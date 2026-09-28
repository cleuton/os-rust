//! Chamadas de sistema (syscalls): o único caminho pelo qual um programa em
//! modo usuário (ring 3) pede serviços ao kernel.
//!
//! O programa executa a instrução `syscall`; a CPU sobe para ring 0 e salta
//! para `syscall_entry` (registrada em `LSTAR`). O stub troca de pilha,
//! chama `syscall_dispatch` e volta ao programa com `sysretq`. Números,
//! registradores, erros e semântica de cada chamada estão em `SYSCALLS.md`,
//! a única fonte da interface (Princípio VII da constitution): nenhuma
//! syscall existe sem estar naquele arquivo.

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

use x86_64::registers::model_specific::{Efer, EferFlags, LStar, SFMask, Star};
use x86_64::registers::rflags::RFlags;
use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::user::{self, Termination, USER_REGION_END, USER_REGION_START};
use crate::{gdt, keyboard, memory, vga_buffer};

// As constantes do contrato (números das syscalls, códigos de erro, limite de
// `len`) vivem na crate `abi`, compartilhada com a biblioteca de runtime dos
// programas: os números nunca divergem entre kernel e programas. O texto do
// contrato é o `SYSCALLS.md`.
pub use abi::{
    ERR_FAULT, ERR_INVAL, ERR_NOMEM, IO_MAX_LEN, SYS_ALLOC, SYS_EXIT, SYS_READ_LINE, SYS_WRITE,
};

/// `rsp` do kernel no momento em que `enter_user` (em `user.rs`) desceu para
/// ring 3. `leave_user` o restaura para "retornar" de `enter_user` quando o
/// programa termina. Lido e escrito só pelos trechos de assembly.
pub(crate) static SAVED_KERNEL_RSP: AtomicU64 = AtomicU64::new(0);

/// `rsp` do programa de usuário, guardado enquanto o kernel roda a syscall
/// na própria pilha e devolvido no `sysretq`. Uma CPU só e interrupções
/// desligadas durante a syscall (`SFMASK`), então uma única variável basta:
/// não há como duas syscalls estarem em andamento ao mesmo tempo.
static USER_RSP_SCRATCH: AtomicU64 = AtomicU64::new(0);

/// Topo da pilha do kernel usada nas syscalls (a mesma de `TSS.rsp0`),
/// gravado por `init`.
static KERNEL_ENTRY_STACK_TOP: AtomicU64 = AtomicU64::new(0);

extern "C" {
    /// Rótulo do stub de entrada de `syscall` (assembly, logo abaixo).
    fn syscall_entry();
}

// Stub de entrada da instrução `syscall`. A CPU já fez, antes de chegar
// aqui: `rcx` = `rip` da instrução seguinte ao `syscall`, `r11` = `rflags`
// do programa, `CS`/`SS` do kernel, `IF` desligado (`SFMASK`). Ela **não**
// trocou `rsp`: ele ainda aponta para a pilha do programa, que o kernel
// nunca usa nem confia. O contrato (`SYSCALLS.md`, seção 4) diz que tudo é
// preservado, menos `rax` (resultado), `rcx` e `r11` (usados pela própria
// instrução): por isso o stub salva e restaura `rdi`, `rsi`, `rdx`, `r8`,
// `r9` e `r10`, que o código Rust chamado pode destruir.
//
// A syscall roda com `IF` desligado, com uma única exceção: `SYS_READ_LINE`
// liga as interrupções enquanto espera uma tecla (`sti; hlt`, em
// `keyboard::read_line`) e as desliga de novo antes de voltar aqui. É
// obrigatório voltar com `IF` desligado: uma interrupção depois de
// `mov rsp, [user_rsp]` e antes do `sysretq` rodaria no kernel em cima da
// pilha do programa.
global_asm!(
    ".global syscall_entry",
    "syscall_entry:",
    // Guarda o rsp do programa e passa para a pilha do kernel.
    "mov [rip + {user_rsp}], rsp",
    "mov rsp, [rip + {kernel_stack}]",
    // Salva o que o retorno precisa (rcx = rip, r11 = rflags) e os
    // registradores que o contrato promete preservar. 8 pushes = 64 bytes:
    // o topo da pilha é múltiplo de 16, então rsp continua alinhado em 16
    // no `call`, como a convenção C exige.
    "push rcx",
    "push r11",
    "push rdi",
    "push rsi",
    "push rdx",
    "push r8",
    "push r9",
    "push r10",
    // Monta os argumentos de `syscall_dispatch(nr, a1, a2, a3)` na
    // convenção C (rdi, rsi, rdx, rcx) a partir da convenção da syscall
    // (nr em rax; a1, a2, a3 em rdi, rsi, rdx). A ordem dos `mov` evita
    // sobrescrever um valor antes de copiá-lo.
    "mov rcx, rdx",
    "mov rdx, rsi",
    "mov rsi, rdi",
    "mov rdi, rax",
    "call {dispatch}",
    // O resultado já está em rax; restaura o resto na ordem inversa.
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rdx",
    "pop rsi",
    "pop rdi",
    "pop r11",
    "pop rcx",
    // Devolve a pilha do programa e volta a ring 3 (rip = rcx, rflags = r11).
    "mov rsp, [rip + {user_rsp}]",
    "sysretq",
    user_rsp = sym USER_RSP_SCRATCH,
    kernel_stack = sym KERNEL_ENTRY_STACK_TOP,
    dispatch = sym syscall_dispatch,
);

/// Liga o mecanismo de `syscall`/`sysret`: `EFER.SCE` (habilita a
/// instrução), `STAR` (seletores de segmento de kernel e de usuário),
/// `LSTAR` (endereço do stub) e `SFMASK` (flags que a CPU desliga ao
/// entrar). Chamada uma única vez por `os_rust::init`, depois de
/// `gdt::init` (os seletores vêm da GDT já carregada).
pub fn init() {
    KERNEL_ENTRY_STACK_TOP.store(gdt::kernel_entry_stack_top().as_u64(), Ordering::Relaxed);

    // SAFETY: só liga o bit `SCE` em `EFER`, preservando os demais (o
    // bootloader já ligou `NXE`, que W^X usa); é feito uma única vez no
    // boot, antes de qualquer programa de usuário existir.
    unsafe {
        Efer::update(|flags| *flags |= EferFlags::SYSTEM_CALL_EXTENSIONS);
    }

    // `Star::write` confere o layout da GDT (dados do usuário antes do
    // código do usuário, dados do kernel logo depois do código do kernel,
    // ver `gdt.rs`). Um erro aqui é um bug de programação na GDT, nunca
    // uma condição de execução: parar o boot com a mensagem é o certo.
    Star::write(
        gdt::user_code_selector(),
        gdt::user_data_selector(),
        gdt::kernel_code_selector(),
        gdt::kernel_data_selector(),
    )
    .expect("GDT com layout incompativel com STAR (syscall/sysret)");

    // `LStar::write` e `SFMask::write` são funções seguras da crate
    // `x86_64`, mas o efeito é o mesmo de uma escrita em MSR: a partir daqui
    // toda instrução `syscall` salta para `syscall_entry`, o stub de
    // assembly acima, que só assume o que a CPU garante na entrada de
    // `syscall` e termina em `sysretq`; os seletores de `STAR` já foram
    // validados.
    LStar::write(VirtAddr::new(syscall_entry as *const () as usize as u64));
    // O kernel entra com interrupções desligadas (a syscall roda na pilha
    // de entrada, sem reentrância; a espera de `SYS_READ_LINE` é a única
    // exceção, e devolve `IF` desligado), direção de string limpa e sem
    // trace, seja qual for o estado do programa.
    SFMask::write(RFlags::INTERRUPT_FLAG | RFlags::DIRECTION_FLAG | RFlags::TRAP_FLAG);
}

/// Despacha uma syscall: `nr` é o número (`rax`), `a1`..`a3` os argumentos
/// (`rdi`, `rsi`, `rdx`). Devolve o resultado (`rax`). Um número que não
/// está no contrato encerra o programa, com mensagem legível, e nunca
/// derruba o kernel.
extern "C" fn syscall_dispatch(nr: u64, a1: u64, a2: u64, _a3: u64) -> u64 {
    match nr {
        SYS_WRITE => sys_write(a1, a2) as u64,
        SYS_EXIT => user::terminate(Termination::Exit { code: a1 }),
        SYS_READ_LINE => sys_read_line(a1, a2) as u64,
        SYS_ALLOC => match user::grow_heap(a1) {
            Ok(start) => start,
            Err(code) => code as u64,
        },
        _ => user::terminate(Termination::BadSyscall { number: nr }),
    }
}

/// Confere que todo o intervalo `[ptr, ptr + len)` é memória do programa:
/// dentro da região do usuário e, para cada página que ele toca, mapeada e
/// acessível ao usuário, e também **gravável** quando `writable` (o kernel vai
/// escrever nele). Supõe `len > 0`. Qualquer falha devolve `Err(ERR_FAULT)`.
/// É a defesa de toda syscall que recebe um ponteiro: o kernel nunca confia
/// no ponteiro que o programa passa (`write` lê dele, `read_line` escreve
/// nele).
fn validate_user_range(ptr: u64, len: u64, writable: bool) -> Result<(), i64> {
    let Some(end) = ptr.checked_add(len) else {
        return Err(ERR_FAULT);
    };
    if ptr < USER_REGION_START || end > USER_REGION_END {
        return Err(ERR_FAULT);
    }

    // Todas as páginas que o intervalo toca: da página de `ptr` até a do
    // último byte (`end - 1`).
    let mut page = ptr & !0xfff;
    let last_page = (end - 1) & !0xfff;
    while page <= last_page {
        match memory::user_page_flags(VirtAddr::new(page)) {
            Some(flags)
                if flags.contains(PageTableFlags::USER_ACCESSIBLE)
                    && (!writable || flags.contains(PageTableFlags::WRITABLE)) => {}
            _ => return Err(ERR_FAULT),
        }
        page += 4096;
    }
    Ok(())
}

/// `read_line(ptr, len)`: espera uma linha do teclado e a escreve em `ptr`
/// (com o `\n` final). Devolve os bytes escritos (`1 ≤ n ≤ len`), `0` se
/// `len == 0`, ou um erro. Toda a validação acontece **antes** de esperar: um
/// ponteiro ruim devolve `ERR_FAULT` sem consumir nenhuma tecla. Detalhes em
/// `SYSCALLS.md` (`SYS_READ_LINE`).
fn sys_read_line(ptr: u64, len: u64) -> i64 {
    if len == 0 {
        return 0;
    }
    if len > IO_MAX_LEN {
        return ERR_INVAL;
    }
    // O kernel vai **escrever** nesse intervalo: além de mapeado e do usuário,
    // precisa ser gravável (uma página de código ou de dados somente-leitura
    // é recusada).
    if let Err(code) = validate_user_range(ptr, len, true) {
        return code;
    }

    // A linha é montada num buffer do kernel (na pilha de entrada) e só
    // copiada para o programa depois do Enter: durante a espera o kernel não
    // segura o ponteiro do usuário.
    let capacity = (len as usize).min(keyboard::LINE_CAPACITY);
    let mut line = [0u8; keyboard::LINE_CAPACITY];
    let written = keyboard::read_line(&mut line[..capacity]);

    // SAFETY: o intervalo `ptr..ptr + len` foi validado acima (dentro da região
    // do usuário, mapeado, `USER_ACCESSIBLE` e gravável) e nada altera esse
    // mapeamento enquanto o programa está bloqueado aqui: uma CPU só, um único
    // programa, nenhuma syscall que desmapeie. `written <= capacity <= len`,
    // então a cópia cabe no intervalo; o buffer do kernel e a memória do
    // programa nunca se sobrepõem.
    unsafe {
        core::ptr::copy_nonoverlapping(line.as_ptr(), ptr as *mut u8, written);
    }

    // `read_line` já devolve com as interrupções desligadas; repetir aqui deixa
    // a garantia visível no ponto que importa: o `sysretq` só pode acontecer
    // com `IF` desligado.
    x86_64::instructions::interrupts::disable();
    written as i64
}

/// `write(ptr, len)`. O kernel **não confia no ponteiro**: antes de ler,
/// confere que o intervalo inteiro está dentro da região do usuário e que
/// cada página dele está mapeada e acessível ao usuário. Qualquer falha
/// devolve `ERR_FAULT` sem escrever nada e o programa continua rodando.
fn sys_write(ptr: u64, len: u64) -> i64 {
    if len == 0 {
        return 0;
    }
    if len > IO_MAX_LEN {
        return ERR_INVAL;
    }
    if let Err(code) = validate_user_range(ptr, len, false) {
        return code;
    }

    // SAFETY: todas as páginas de `ptr..ptr + len` foram conferidas acima
    // (mapeadas, acessíveis ao usuário e dentro da região do usuário), e o
    // kernel roda no mesmo espaço de endereçamento do programa, então ler
    // esses bytes é seguro; nenhum outro código escreve neles durante a
    // syscall (uma CPU só, interrupções desligadas).
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    // O lock do `WRITER` é solto ao fim desta instrução, antes de voltar
    // ao programa: nenhum lock preso ao sair da syscall.
    vga_buffer::WRITER.lock().write_bytes(bytes);
    len as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::{USER_HEAP_END, USER_HEAP_MAX_PAGES, USER_HEAP_START};
    use alloc::format;

    /// O contrato publicado, embutido no teste: se o texto e o código
    /// divergirem, o teste falha (Princípio VII: `SYSCALLS.md` é a única
    /// fonte da interface).
    const CONTRATO: &str = include_str!("../SYSCALLS.md");

    /// Formata um endereço como o documento o escreve, com `_` no meio por
    /// legibilidade (`0x4000_0000`): o valor da constante do código é
    /// formatado do mesmo jeito para a comparação.
    fn com_separador(valor: u64) -> alloc::string::String {
        format!("{:#06x}_{:04x}", valor >> 16, valor & 0xffff)
    }

    #[test_case]
    fn contrato_cita_os_numeros_das_syscalls() {
        assert!(CONTRATO.contains(&format!("| {} | `SYS_WRITE` |", SYS_WRITE)));
        assert!(CONTRATO.contains(&format!("| {} | `SYS_EXIT` |", SYS_EXIT)));
        assert!(CONTRATO.contains(&format!("| {} | `SYS_READ_LINE` |", SYS_READ_LINE)));
        assert!(CONTRATO.contains(&format!("| {} | `SYS_ALLOC` |", SYS_ALLOC)));
    }

    #[test_case]
    fn contrato_cita_os_codigos_de_erro() {
        assert!(CONTRATO.contains(&format!("| `ERR_FAULT` | `{}` |", ERR_FAULT)));
        assert!(CONTRATO.contains(&format!("| `ERR_INVAL` | `{}` |", ERR_INVAL)));
        assert!(CONTRATO.contains(&format!("| `ERR_NOMEM` | `{}` |", ERR_NOMEM)));
        assert!(CONTRATO.contains(&format!("`len > {}`", IO_MAX_LEN)));
    }

    #[test_case]
    fn contrato_cita_a_regiao_do_usuario() {
        let regiao = format!(
            "[{}, {})",
            com_separador(USER_REGION_START),
            com_separador(USER_REGION_END)
        );
        assert!(CONTRATO.contains(&regiao), "regiao nao encontrada: {}", regiao);
    }

    #[test_case]
    fn contrato_cita_a_janela_do_heap_e_a_faixa_de_codigo() {
        let heap = format!(
            "[{}, {})",
            com_separador(USER_HEAP_START),
            com_separador(USER_HEAP_END)
        );
        assert!(CONTRATO.contains(&heap), "heap nao encontrado: {}", heap);
        // Código e dados: do começo da região até o começo do heap.
        let codigo = format!(
            "[{}, {})",
            com_separador(USER_REGION_START),
            com_separador(USER_HEAP_START)
        );
        assert!(CONTRATO.contains(&codigo), "faixa de codigo nao encontrada: {}", codigo);
    }

    #[test_case]
    fn contrato_cita_os_limites_de_linha_e_de_heap() {
        // O limite de caracteres de uma linha e o de páginas do heap são
        // definidos no código: o documento precisa dizer os mesmos números.
        assert!(CONTRATO.contains(&format!("min(len, {})", keyboard::LINE_CAPACITY)));
        assert!(CONTRATO.contains(&format!("passaria de {} ", USER_HEAP_MAX_PAGES)));
    }

    #[test_case]
    fn contrato_declara_a_versao_2() {
        assert!(CONTRATO.contains("**Versão do contrato**: 2"));
    }
}
