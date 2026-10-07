//! Chamadas de sistema (syscalls): o único caminho pelo qual um programa em
//! modo usuário (ring 3) pede serviços ao kernel.
//!
//! O programa executa a instrução `syscall`; a CPU sobe para ring 0 e salta
//! para `syscall_entry` (registrada em `LSTAR`). O stub troca de pilha, monta
//! o estado do programa (`TaskContext`) na pilha do kernel, chama
//! `syscall_dispatch` e volta ao programa com `iretq`. Números, registradores,
//! erros e semântica de cada chamada estão em `SYSCALLS.md`, a única fonte da
//! interface: nenhuma syscall existe sem estar naquele arquivo.

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

use x86_64::registers::model_specific::{Efer, EferFlags, LStar, SFMask, Star};
use x86_64::registers::rflags::RFlags;
use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::gdt::{USER_CODE_SELECTOR_BITS, USER_DATA_SELECTOR_BITS};
use crate::scheduler;
use crate::task::TaskContext;
use crate::user::{self, Termination, USER_REGION_END, USER_REGION_START};
use crate::{gdt, memory, vga_buffer};

// As constantes do contrato (números das syscalls, códigos de erro, limite de
// `len`) vivem na crate `abi`, compartilhada com a biblioteca de runtime dos
// programas: os números nunca divergem entre kernel e programas. O texto do
// contrato é o `SYSCALLS.md`.
pub use abi::{
    DirEntryRaw, ERR_BADF, ERR_FAULT, ERR_INVAL, ERR_IO, ERR_MFILE, ERR_NAMETOOLONG, ERR_NODEV,
    ERR_NOENT, ERR_NOMEM, ERR_TYPE, IO_MAX_LEN, SYS_ALLOC, SYS_CLOSE, SYS_EXIT, SYS_OPEN,
    SYS_READ, SYS_READ_DIR, SYS_READ_LINE, SYS_WRITE, SYS_YIELD,
};
use abi::{DIR_ENTRY_SIZE, KIND_DIR, KIND_FILE, MAX_PATH_LEN};

use crate::fat::Kind;
use crate::fs::FsError;

/// `rsp` do kernel no momento em que `enter_user` (em `user.rs`) entrou no
/// escalonador. `leave_user` o restaura para "retornar" de `enter_user` quando
/// o último programa termina. Lido e escrito só pelos trechos de assembly.
pub(crate) static SAVED_KERNEL_RSP: AtomicU64 = AtomicU64::new(0);

/// `rsp` do programa de usuário, guardado só durante os primeiros instantes
/// do stub (entre a instrução `syscall` e o empilhamento do contexto). Uma CPU
/// só e interrupções desligadas (`SFMASK`) nesse trecho, então uma única
/// variável basta: não há como duas syscalls estarem ali ao mesmo tempo.
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
// nunca usa nem confia.
//
// O stub monta, na pilha do kernel, um `TaskContext` completo (ver
// `task.rs`): primeiro as cinco palavras do frame de `iretq` (`ss`, `rsp` do
// programa, `rflags`, `cs`, `rip`), depois os 15 registradores gerais, com
// `r15` por último (menor endereço). Com o estado inteiro do programa guardado,
// uma syscall pode devolver a CPU ao mesmo programa ou a outro, e a troca é
// só copiar esse bloco. A volta é por `iretq`: o contrato (`SYSCALLS.md`,
// seção 4) continua dizendo que `rcx` e `r11` são destruídos, mas esta
// implementação os devolve intactos.
//
// A syscall roda com `IF` desligado de ponta a ponta: uma interrupção depois de
// `mov rsp, [user_rsp]` e antes do retorno rodaria no kernel em cima da pilha
// do programa. A única espera longa (teclado) é feita pelo escalonador, fora
// da syscall, e não pelo stub.
global_asm!(
    ".global syscall_entry",
    "syscall_entry:",
    // Guarda o rsp do programa e passa para a pilha do kernel.
    "mov [rip + {user_rsp}], rsp",
    "mov rsp, [rip + {kernel_stack}]",
    // Frame de `iretq`, do endereço mais alto para o mais baixo.
    "push {user_ss}",
    "push qword ptr [rip + {user_rsp}]",
    "push r11",
    "push {user_cs}",
    "push rcx",
    // Os 15 registradores gerais; o último `push` (r15) fica no menor
    // endereço, que é onde `TaskContext` começa.
    "push rax",
    "push rbx",
    "push rcx",
    "push rdx",
    "push rsi",
    "push rdi",
    "push rbp",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    // O topo da pilha é múltiplo de 16 e foram empilhadas 20 palavras
    // (160 bytes), então `rsp` continua alinhado em 16 no `call`, como a
    // convenção C exige. O argumento é o ponteiro para o `TaskContext`.
    "mov rdi, rsp",
    "call {dispatch}",
    // O resultado já foi gravado no campo `rax` do contexto; desempilha tudo
    // na ordem inversa e volta ao programa.
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rbp",
    "pop rdi",
    "pop rsi",
    "pop rdx",
    "pop rcx",
    "pop rbx",
    "pop rax",
    "iretq",
    user_rsp = sym USER_RSP_SCRATCH,
    kernel_stack = sym KERNEL_ENTRY_STACK_TOP,
    dispatch = sym syscall_dispatch,
    user_ss = const USER_DATA_SELECTOR_BITS,
    user_cs = const USER_CODE_SELECTOR_BITS,
);

/// Liga o mecanismo de `syscall`: `EFER.SCE` (habilita a instrução), `STAR`
/// (seletores de segmento de kernel e de usuário), `LSTAR` (endereço do stub) e
/// `SFMASK` (flags que a CPU desliga ao entrar). Chamada uma única vez por
/// `os_rust::init`, depois de `gdt::init` (os seletores vêm da GDT já
/// carregada).
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
    // `syscall` e termina em `iretq`; os seletores de `STAR` já foram
    // validados.
    LStar::write(VirtAddr::new(syscall_entry as *const () as usize as u64));
    // O kernel entra com interrupções desligadas (a syscall roda na pilha
    // de entrada, sem reentrância), direção de string limpa e sem trace,
    // seja qual for o estado do programa.
    SFMask::write(RFlags::INTERRUPT_FLAG | RFlags::DIRECTION_FLAG | RFlags::TRAP_FLAG);
}

/// Despacha uma syscall: o número está em `ctx.rax` e os argumentos em
/// `ctx.rdi`, `ctx.rsi` e `ctx.rdx`; o resultado volta em `ctx.rax`. Um número que não
/// está no contrato encerra o programa, com mensagem legível, e nunca derruba
/// o kernel. `SYS_EXIT`, `SYS_YIELD` (quando há outro programa pronto) e um
/// número inexistente **não retornam** aqui: o escalonador retoma outra tarefa.
extern "C" fn syscall_dispatch(ctx: &mut TaskContext) {
    let (nr, a1, a2, a3) = (ctx.rax, ctx.rdi, ctx.rsi, ctx.rdx);
    match nr {
        SYS_WRITE => ctx.rax = sys_write(a1, a2) as u64,
        SYS_EXIT => scheduler::terminate_current(Termination::Exit { code: a1 }),
        SYS_READ_LINE => sys_read_line(ctx, a1, a2),
        SYS_ALLOC => {
            ctx.rax = match user::grow_heap(a1) {
                Ok(start) => start,
                Err(code) => code as u64,
            }
        }
        SYS_YIELD => scheduler::yield_now(ctx),
        SYS_OPEN => ctx.rax = sys_open(a1, a2) as u64,
        SYS_READ => ctx.rax = sys_read(a1, a2, a3) as u64,
        SYS_CLOSE => ctx.rax = sys_close(a1) as u64,
        SYS_READ_DIR => ctx.rax = sys_read_dir(a1, a2, a3) as u64,
        _ => scheduler::terminate_current(Termination::BadSyscall { number: nr }),
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
/// ponteiro ruim devolve `ERR_FAULT` sem consumir nenhuma tecla. Se tudo está
/// certo, a tarefa fica **bloqueada** e o escalonador passa a CPU adiante: quem
/// espera não gasta CPU, e a linha só é copiada para o programa quando ele volta
/// a rodar, depois do Enter (`scheduler::block_on_keyboard`). Detalhes em
/// `SYSCALLS.md` (`SYS_READ_LINE`).
fn sys_read_line(ctx: &mut TaskContext, ptr: u64, len: u64) {
    if len == 0 {
        ctx.rax = 0;
        return;
    }
    if len > IO_MAX_LEN {
        ctx.rax = ERR_INVAL as u64;
        return;
    }
    // O kernel vai **escrever** nesse intervalo: além de mapeado e do usuário,
    // precisa ser gravável (uma página de código ou de dados somente-leitura
    // é recusada).
    if let Err(code) = validate_user_range(ptr, len, true) {
        ctx.rax = code as u64;
        return;
    }
    scheduler::block_on_keyboard(ctx, ptr, len)
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

/// Traduz um erro de arquivo no código que o programa recebe (`SYSCALLS.md`,
/// seção "Sistema de arquivos"). Volume desconhecido e volume indisponível são
/// o mesmo código para o programa: ele só precisa saber que não há volume.
fn errno(error: FsError) -> i64 {
    match error {
        FsError::UnknownVolume | FsError::Unavailable(_) => ERR_NODEV,
        FsError::NotFound => ERR_NOENT,
        FsError::NotADirectory | FsError::IsADirectory => ERR_TYPE,
        FsError::PathTooLong => ERR_NAMETOOLONG,
        FsError::InvalidPath => ERR_INVAL,
        FsError::BadDescriptor => ERR_BADF,
        FsError::TooManyOpen => ERR_MFILE,
        // `TooBig` só existe em `run`; se algum dia chegasse aqui, é falha de leitura.
        FsError::Io | FsError::TooBig { .. } => ERR_IO,
    }
}

/// O descritor que o programa passou, como índice. Um valor que nem cabe em
/// `usize` (ou que o programa gerou de um `-1`) nunca é um descritor válido.
fn descriptor(fd: u64) -> Result<usize, i64> {
    usize::try_from(fd).map_err(|_| ERR_BADF)
}

/// `open(path_ptr, path_len)`: abre um arquivo ou diretório, somente para
/// leitura, e devolve o descritor. O caminho é copiado para um array na pilha
/// do kernel (sem heap) **antes** de ser interpretado: o programa não pode
/// mudá-lo no meio da conversa.
fn sys_open(ptr: u64, len: u64) -> i64 {
    if len == 0 {
        return ERR_INVAL;
    }
    if len > MAX_PATH_LEN as u64 {
        return ERR_NAMETOOLONG;
    }
    if let Err(code) = validate_user_range(ptr, len, false) {
        return code;
    }
    let mut path = [0u8; MAX_PATH_LEN];
    let len = len as usize;
    // SAFETY: `validate_user_range` conferiu que todas as páginas de
    // `ptr..ptr + len` estão mapeadas e acessíveis ao usuário, dentro da região
    // do usuário; o kernel roda no espaço de endereçamento do programa, uma
    // CPU só e interrupções desligadas, então nada muda esses bytes durante a
    // cópia. `len <= MAX_PATH_LEN` cabe em `path`.
    path[..len].copy_from_slice(unsafe { core::slice::from_raw_parts(ptr as *const u8, len) });
    match scheduler::with_current_files(|files| files.open(&path[..len])) {
        Some(Ok(fd)) => fd as i64,
        Some(Err(error)) => errno(error),
        None => ERR_BADF,
    }
}

/// `read(fd, ptr, len)`: lê até `len` bytes do arquivo aberto, na posição
/// dele, e a avança. Devolve os bytes lidos; `0` só no fim do arquivo. O buffer
/// do programa é validado (e precisa ser gravável) **antes** de qualquer leitura
/// de disco. A leitura é feita em pedaços de um setor, com um buffer na pilha
/// do kernel; se um pedaço falha depois de outros terem sido entregues, devolve
/// o que já foi lido, e a próxima chamada recebe o erro.
fn sys_read(fd: u64, ptr: u64, len: u64) -> i64 {
    if len == 0 {
        return 0;
    }
    if len > IO_MAX_LEN {
        return ERR_INVAL;
    }
    let fd = match descriptor(fd) {
        Ok(fd) => fd,
        Err(code) => return code,
    };
    if let Err(code) = validate_user_range(ptr, len, true) {
        return code;
    }
    let len = len as usize;
    let result = scheduler::with_current_files(|files| -> Result<usize, FsError> {
        let file = files.get_mut(fd)?;
        let mut chunk = [0u8; 512];
        let mut total = 0usize;
        while total < len {
            let want = (len - total).min(chunk.len());
            let n = match file.read(&mut chunk[..want]) {
                Ok(n) => n,
                Err(error) if total == 0 => return Err(error),
                Err(_) => break,
            };
            if n == 0 {
                break;
            }
            // SAFETY: `ptr..ptr + len` foi validado acima (mapeado, do usuário
            // e gravável) e `total + n <= len`; o kernel roda no espaço de
            // endereçamento do programa, uma CPU só, interrupções desligadas.
            unsafe {
                core::ptr::copy_nonoverlapping(chunk.as_ptr(), (ptr as *mut u8).add(total), n);
            }
            total += n;
        }
        Ok(total)
    });
    match result {
        Some(Ok(total)) => total as i64,
        Some(Err(error)) => errno(error),
        None => ERR_BADF,
    }
}

/// `close(fd)`: fecha o descritor.
fn sys_close(fd: u64) -> i64 {
    let fd = match descriptor(fd) {
        Ok(fd) => fd,
        Err(code) => return code,
    };
    match scheduler::with_current_files(|files| files.close(fd)) {
        Some(Ok(())) => 0,
        Some(Err(error)) => errno(error),
        None => ERR_BADF,
    }
}

/// `read_dir(fd, ptr, len)`: escreve em `ptr` a próxima entrada do diretório
/// aberto (um `DirEntryRaw`, `DIR_ENTRY_SIZE` bytes) e devolve `1`; `0` quando
/// as entradas acabaram, sem escrever nada.
fn sys_read_dir(fd: u64, ptr: u64, len: u64) -> i64 {
    if len > IO_MAX_LEN || len < DIR_ENTRY_SIZE as u64 {
        return ERR_INVAL;
    }
    let fd = match descriptor(fd) {
        Ok(fd) => fd,
        Err(code) => return code,
    };
    if let Err(code) = validate_user_range(ptr, DIR_ENTRY_SIZE as u64, true) {
        return code;
    }
    let result = scheduler::with_current_files(|files| files.get_mut(fd)?.next_entry());
    let entry = match result {
        Some(Ok(Some(entry))) => entry,
        Some(Ok(None)) => return 0,
        Some(Err(error)) => return errno(error),
        None => return ERR_BADF,
    };
    let mut raw = DirEntryRaw {
        name: [0; 12],
        kind: if entry.kind == Kind::Dir { KIND_DIR } else { KIND_FILE },
        _pad: [0; 3],
        size: entry.size,
    };
    raw.name[..entry.name_len as usize].copy_from_slice(&entry.name[..entry.name_len as usize]);
    // SAFETY: `ptr..ptr + DIR_ENTRY_SIZE` foi validado acima (mapeado, do
    // usuário e gravável); `DirEntryRaw` é `repr(C)` de `DIR_ENTRY_SIZE` bytes
    // (conferido por uma asserção na crate `abi`); a escrita não exige
    // alinhamento (`write_unaligned`).
    unsafe { core::ptr::write_unaligned(ptr as *mut DirEntryRaw, raw) };
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::{USER_HEAP_END, USER_HEAP_MAX_PAGES, USER_HEAP_START};
    use alloc::format;

    /// O contrato publicado, embutido no teste: se o texto e o código
    /// divergirem, o teste falha (`SYSCALLS.md` é a única fonte da
    /// interface).
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
        assert!(CONTRATO.contains(&format!("| {} | `SYS_YIELD` |", SYS_YIELD)));
        assert!(CONTRATO.contains(&format!("| {} | `SYS_OPEN` |", SYS_OPEN)));
        assert!(CONTRATO.contains(&format!("| {} | `SYS_READ` |", SYS_READ)));
        assert!(CONTRATO.contains(&format!("| {} | `SYS_CLOSE` |", SYS_CLOSE)));
        assert!(CONTRATO.contains(&format!("| {} | `SYS_READ_DIR` |", SYS_READ_DIR)));
    }

    #[test_case]
    fn cada_erro_de_arquivo_vira_o_codigo_do_contrato() {
        use crate::fs::Reason;
        assert_eq!(errno(FsError::UnknownVolume), ERR_NODEV);
        assert_eq!(errno(FsError::Unavailable(Reason::NoDisk)), ERR_NODEV);
        assert_eq!(errno(FsError::Unavailable(Reason::IoError)), ERR_NODEV);
        assert_eq!(errno(FsError::NotFound), ERR_NOENT);
        assert_eq!(errno(FsError::NotADirectory), ERR_TYPE);
        assert_eq!(errno(FsError::IsADirectory), ERR_TYPE);
        assert_eq!(errno(FsError::PathTooLong), ERR_NAMETOOLONG);
        assert_eq!(errno(FsError::InvalidPath), ERR_INVAL);
        assert_eq!(errno(FsError::BadDescriptor), ERR_BADF);
        assert_eq!(errno(FsError::TooManyOpen), ERR_MFILE);
        assert_eq!(errno(FsError::Io), ERR_IO);
        assert_eq!(errno(FsError::TooBig { size: 2, max: 1 }), ERR_IO);
    }

    #[test_case]
    fn descritor_que_nao_cabe_em_usize_ou_e_enorme_e_invalido() {
        assert_eq!(descriptor(0), Ok(0));
        assert_eq!(descriptor(3), Ok(3));
        // `-1` visto como `u64`: cabe em `usize` (64 bits) e a tabela o recusa.
        assert!(descriptor(u64::MAX).map_or(true, |fd| fd >= abi::MAX_OPEN_FILES));
    }

    #[test_case]
    fn contrato_cita_os_codigos_de_erro() {
        assert!(CONTRATO.contains(&format!("| `ERR_FAULT` | `{}` |", ERR_FAULT)));
        assert!(CONTRATO.contains(&format!("| `ERR_INVAL` | `{}` |", ERR_INVAL)));
        assert!(CONTRATO.contains(&format!("| `ERR_NOMEM` | `{}` |", ERR_NOMEM)));
        assert!(CONTRATO.contains(&format!("| `ERR_NOENT` | `{}` |", ERR_NOENT)));
        assert!(CONTRATO.contains(&format!("| `ERR_NODEV` | `{}` |", ERR_NODEV)));
        assert!(CONTRATO.contains(&format!("| `ERR_TYPE` | `{}` |", ERR_TYPE)));
        assert!(CONTRATO.contains(&format!("| `ERR_BADF` | `{}` |", ERR_BADF)));
        assert!(CONTRATO.contains(&format!("| `ERR_MFILE` | `{}` |", ERR_MFILE)));
        assert!(CONTRATO.contains(&format!("| `ERR_NAMETOOLONG` | `{}` |", ERR_NAMETOOLONG)));
        assert!(CONTRATO.contains(&format!("| `ERR_IO` | `{}` |", ERR_IO)));
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
        assert!(CONTRATO.contains(&format!("min(len, {})", crate::keyboard::LINE_CAPACITY)));
        assert!(CONTRATO.contains(&format!("passaria de {} ", USER_HEAP_MAX_PAGES)));
    }

    #[test_case]
    fn contrato_declara_a_versao_4() {
        assert!(CONTRATO.contains("**Versão do contrato**: 4"));
        assert!(CONTRATO.contains("os-rust 0.8.0 (Marco 8)"));
    }

    #[test_case]
    fn contrato_declara_o_sistema_de_arquivos_somente_leitura() {
        assert!(CONTRATO.contains("**somente leitura**"));
        assert!(CONTRATO.contains("| `/ram` |"));
        assert!(CONTRATO.contains("| `/disco` |"));
    }

    #[test_case]
    fn contrato_cita_os_limites_do_sistema_de_arquivos() {
        // Os limites vêm de `abi`, as mesmas constantes que o kernel e a
        // biblioteca de runtime usam: o documento precisa dizê-los.
        assert!(CONTRATO.contains(&format!("| Tamanho do caminho | **{} bytes** |", abi::MAX_PATH_LEN)));
        assert!(CONTRATO.contains(&format!("| Arquivos abertos por programa | **{}** |", abi::MAX_OPEN_FILES)));
        assert!(CONTRATO.contains(&format!("| Executável lido de arquivo | **{} bytes** |", abi::MAX_EXEC_SIZE)));
        assert!(CONTRATO.contains(&format!("| Componentes por caminho | **{}** |", crate::fs::MAX_DEPTH)));
        assert!(CONTRATO.contains(&format!("| Entradas percorridas por diretório | {} ", crate::fat::MAX_DIR_ENTRIES)));
        assert!(CONTRATO.contains(&format!("`SYS_READ_DIR` escreve {} bytes", abi::DIR_ENTRY_SIZE)));
    }

    #[test_case]
    fn contrato_cita_o_maximo_de_programas_e_a_fatia() {
        // O número máximo de programas e a fatia de tempo vêm de `abi`, as
        // mesmas constantes que o kernel usa: o documento precisa dizê-los.
        assert!(CONTRATO.contains(&format!("no máximo **{}** programas", abi::MAX_TASKS)));
        assert!(CONTRATO.contains(&format!("**{} ticks**", abi::SLICE_TICKS)));
        assert!(CONTRATO.contains(&format!("**{} Hz**", abi::TIMER_HZ)));
    }
}
