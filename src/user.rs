//! Programas de usuário (ring 3): layout de memória, carregador e execução.
//!
//! Este módulo reúne o que o kernel sabe sobre onde um programa vive: a
//! região de endereços do usuário, a pilha inicial e o estado dos
//! registradores na primeira instrução. Tudo aqui está documentado em
//! `SYSCALLS.md` (Princípio VII da constitution): um programa só pode
//! depender do que está naquele arquivo.

/// Primeiro byte da região de memória do usuário: 1 GiB. Escolhida por três
/// motivos: fica abaixo de 2 GiB, então o code model padrão do Rust
/// (relocações `R_X86_64_32S`) funciona sem gambiarra nos programas; ocupa
/// a entrada 1 do nível P3 dentro da entrada 0 do P4, que o kernel e o
/// bootloader não usam (o kernel vive na entrada 0 do P3); e é
/// um número fácil de reconhecer em aula.
pub const USER_REGION_START: u64 = 0x4000_0000;

/// Primeiro byte depois da região do usuário (exclusivo): 2 GiB. Toda a
/// região é exatamente a entrada 1 do P3 (1 GiB de endereços).
pub const USER_REGION_END: u64 = 0x8000_0000;

/// Número de páginas de 4 KiB da pilha do usuário: 4 páginas, 16 KiB, o
/// bastante para os programas deste marco (nenhum usa recursão nem
/// `alloca`). A página logo abaixo da pilha fica sem mapeamento: um
/// estouro de pilha vira `#PF` em vez de corromper outro segmento.
pub const USER_STACK_PAGES: u64 = 4;

/// Fim (exclusivo) da pilha do usuário: o último byte da região. A pilha
/// ocupa as `USER_STACK_PAGES` últimas páginas dela.
pub const USER_STACK_TOP: u64 = USER_REGION_END;

/// Início da pilha do usuário (a página mais baixa mapeada).
pub const USER_STACK_BOTTOM: u64 = USER_STACK_TOP - USER_STACK_PAGES * 4096;

/// Valor de `rsp` na primeira instrução do programa: o topo da pilha menos
/// 8. Um `_start` compilado por Rust é uma função `extern "C"` e espera
/// `rsp ≡ 8 (mod 16)` na entrada, como se tivesse sido chamada por um
/// `call` (que empilha o endereço de retorno). Aqui não há endereço de
/// retorno válido em `[rsp]`: o programa nunca retorna de `_start`, deve
/// chamar `exit`.
pub const USER_INITIAL_RSP: u64 = USER_STACK_TOP - 8;

/// Valor de `rflags` na primeira instrução do programa: bit reservado 1
/// sempre ligado e `IF` (interrupções habilitadas) ligado, o resto zero.
/// `sysretq` carrega `rflags` de `r11`.
pub const USER_RFLAGS: u64 = 0x202;

use spin::Mutex;
use x86_64::structures::paging::{Page, PageTableFlags, Size4KiB};
use x86_64::VirtAddr;

use crate::elf::{self, ElfImage, LoadError, MAX_SEGMENTS};
use crate::memory;
use crate::programs::PROGRAMS;
use crate::syscall::SAVED_KERNEL_RSP;

/// Por que o programa deixou de rodar. Guardado por quem detecta o fim (o
/// despachante de syscall) e lido por `run_image` depois que `enter_user`
/// retorna. Não contém nada alocado: é escrito dentro de handlers, onde o
/// heap nunca é usado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Termination {
    /// O programa chamou `exit(code)`.
    Exit { code: u64 },
    /// O programa chamou uma syscall que não está no contrato.
    BadSyscall { number: u64 },
    /// O programa causou uma exceção de CPU (em ring 3): `mnemonic` é a
    /// sigla (`#UD`), `name` o nome legível, `rip` o endereço da instrução
    /// que falhou, `error_code` o código de erro da exceção (quando ela
    /// tem um) e `fault_address` o endereço acessado (só `#PF`).
    Fault {
        mnemonic: &'static str,
        name: &'static str,
        rip: u64,
        error_code: Option<u64>,
        fault_address: Option<u64>,
    },
}

/// Por que `run` não conseguiu executar o programa pedido.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunError {
    /// O nome não corresponde a nenhum programa embutido.
    UnknownProgram,
    /// O programa existe, mas não pôde ser carregado.
    Load(LoadError),
}

/// Motivo do término do programa em execução; `None` enquanto ele roda e
/// depois que `run_image` o leu.
static TERMINATION: Mutex<Option<Termination>> = Mutex::new(None);

/// Um intervalo de páginas mapeado por `load`, para `unload` desfazer.
#[derive(Clone, Copy)]
struct MappedRange {
    start: u64,
    pages: u64,
}

/// Tudo que `load` mapeou. Array de tamanho fixo (os segmentos mais a pilha),
/// sem heap, porque precisa existir também no caminho de falha da carga.
struct Loaded {
    ranges: [MappedRange; MAX_SEGMENTS + 1],
    count: usize,
}

impl Loaded {
    fn new() -> Self {
        Loaded {
            ranges: [MappedRange { start: 0, pages: 0 }; MAX_SEGMENTS + 1],
            count: 0,
        }
    }

    /// Abre um intervalo novo (ainda com zero páginas mapeadas); as páginas
    /// são contadas uma a uma em `page_mapped`, para que uma falha no meio
    /// desfaça exatamente o que foi mapeado.
    fn begin(&mut self, start: u64) {
        self.ranges[self.count] = MappedRange { start, pages: 0 };
        self.count += 1;
    }

    fn page_mapped(&mut self) {
        self.ranges[self.count - 1].pages += 1;
    }
}

/// Desmapeia tudo que `load` mapeou e devolve os frames ao alocador.
fn unload(loaded: &Loaded) {
    for range in &loaded.ranges[..loaded.count] {
        memory::unmap_user_range(VirtAddr::new(range.start), range.pages);
    }
}

/// Traduz o erro de mapeamento da crate `x86_64` para o do carregador.
fn map_error(error: x86_64::structures::paging::mapper::MapToError<Size4KiB>) -> LoadError {
    use x86_64::structures::paging::mapper::MapToError;
    match error {
        MapToError::FrameAllocationFailed => LoadError::OutOfFrames,
        MapToError::ParentEntryHugePage | MapToError::PageAlreadyMapped(_) => LoadError::RegionBusy,
    }
}

/// Mapeia uma página do usuário em `vaddr`, já zerada, e devolve o frame
/// para o chamador preencher.
fn map_zeroed_page(
    vaddr: u64,
    flags: PageTableFlags,
) -> Result<x86_64::structures::paging::PhysFrame<Size4KiB>, LoadError> {
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(vaddr));
    let frame = memory::map_user_page(page, flags).map_err(map_error)?;
    memory::zero_frame(frame);
    Ok(frame)
}

/// Carrega os segmentos do programa e a pilha na região do usuário: para
/// cada página, aloca um frame, zera, copia os bytes do arquivo e mapeia com
/// as permissões do segmento (W^X). Em qualquer falha, desfaz o que já
/// mapeou antes de devolver o erro: nunca sobra mapeamento pela metade.
fn load(image: &ElfImage) -> Result<Loaded, LoadError> {
    if !memory::claim_user_region() {
        return Err(LoadError::RegionBusy);
    }
    let mut loaded = Loaded::new();
    let result = load_into(image, &mut loaded);
    if result.is_err() {
        unload(&loaded);
    }
    result.map(|()| loaded)
}

fn load_into(image: &ElfImage, loaded: &mut Loaded) -> Result<(), LoadError> {
    for segment in image.segments() {
        // Leitura é sempre permitida; escrita só com PF_W; execução só com
        // PF_X (sem NO_EXECUTE). O bootloader já ligou EFER.NXE.
        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if segment.flags.write {
            flags |= PageTableFlags::WRITABLE;
        }
        if !segment.flags.execute {
            flags |= PageTableFlags::NO_EXECUTE;
        }

        let pages = (segment.end_page_aligned() - segment.vaddr) / 4096;
        loaded.begin(segment.vaddr);
        for index in 0..pages {
            let frame = map_zeroed_page(segment.vaddr + index * 4096, flags)?;
            loaded.page_mapped();
            // Os bytes do arquivo que caem nesta página; o resto da página
            // (e as páginas além do arquivo, a `.bss`) ficam zerados.
            let from = (index * 4096) as usize;
            if from < segment.file_bytes.len() {
                let to = (from + 4096).min(segment.file_bytes.len());
                memory::fill_frame(frame, 0, &segment.file_bytes[from..to]);
            }
        }
    }

    // Pilha: gravável e nunca executável. A página logo abaixo dela não é
    // mapeada (guarda): um estouro de pilha vira #PF.
    let stack_flags = PageTableFlags::PRESENT
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_EXECUTE;
    loaded.begin(USER_STACK_BOTTOM);
    for index in 0..USER_STACK_PAGES {
        map_zeroed_page(USER_STACK_BOTTOM + index * 4096, stack_flags)?;
        loaded.page_mapped();
    }
    Ok(())
}

extern "C" {
    /// Desce para ring 3 no ponto `entry` com a pilha em `user_rsp` e só
    /// "retorna" quando o programa termina (via `leave_user`). Assembly logo
    /// abaixo.
    fn enter_user(entry: u64, user_rsp: u64);
    /// Volta ao chamador de `enter_user`, abandonando a pilha em uso.
    fn leave_user() -> !;
}

// `enter_user(entry: rdi, user_rsp: rsi)` e `leave_user()`: o mesmo padrão
// de `setjmp`/`longjmp` do C, com uma única volta. `enter_user` guarda o
// estado do kernel (registradores callee-saved e `rsp`) e desce para ring 3;
// `leave_user`, chamada de dentro de uma syscall ou de um handler de
// exceção (na pilha de entrada do kernel), restaura esse estado e executa
// `ret`, que retorna a quem chamou `enter_user`, como se ele tivesse
// terminado normalmente.
core::arch::global_asm!(
    ".global enter_user",
    "enter_user:",
    // Registradores que a convenção C exige que o kernel preserve: quem
    // chamou `enter_user` os espera intactos quando ele "retornar".
    "push rbx",
    "push rbp",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov [rip + {saved_rsp}], rsp",
    // Nenhuma interrupção pode chegar entre carregar o rsp do usuário e o
    // `sysretq`: ela rodaria no kernel em cima da pilha do programa.
    "cli",
    // `sysretq` volta para rip = rcx com rflags = r11.
    "mov rcx, rdi",
    "mov r11, {rflags}",
    // O programa começa com todos os outros registradores zerados: nada do
    // kernel pode vazar para ele (`SYSCALLS.md`, seção 3). Só `rcx` e `r11`
    // não são zero: `sysretq` os deixa como estavam (rip e flags iniciais).
    "xor eax, eax",
    "xor ebx, ebx",
    "xor edx, edx",
    "xor edi, edi",
    "xor ebp, ebp",
    "xor r8d, r8d",
    "xor r9d, r9d",
    "xor r10d, r10d",
    "xor r12d, r12d",
    "xor r13d, r13d",
    "xor r14d, r14d",
    "xor r15d, r15d",
    // rsp do usuário: penúltima instrução, e rsi é zerado logo depois.
    "mov rsp, rsi",
    "xor esi, esi",
    "sysretq",
    ".global leave_user",
    "leave_user:",
    // Uma exceção em ring 3 não limpa o flag de direção; o código Rust do
    // kernel a que vamos voltar pressupõe DF = 0.
    "cld",
    "mov rsp, [rip + {saved_rsp}]",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    "ret",
    saved_rsp = sym SAVED_KERNEL_RSP,
    rflags = const USER_RFLAGS,
);

/// Encerra o programa em execução: guarda o motivo e volta ao chamador de
/// `enter_user`. Chamada de dentro de uma syscall ou de um handler de
/// exceção, ambos na pilha de entrada do kernel. Nunca retorna.
///
/// Não passa por `panic::enter_fatal_handler`: aquela guarda de reentrância
/// é permanente, e travaria o kernel para sempre no segundo erro de programa.
pub(crate) fn terminate(reason: Termination) -> ! {
    // O lock é solto ao fim desta instrução, antes de abandonar a pilha.
    *TERMINATION.lock() = Some(reason);
    // SAFETY: só é chamada quando um programa de usuário está em execução
    // (`enter_user` guardou `SAVED_KERNEL_RSP` e ainda não retornou). Abandonar
    // a pilha atual é seguro: nenhum lock está preso (o de `TERMINATION` foi
    // solto acima, e `sys_write` solta o do `WRITER` antes de voltar) e
    // nenhum objeto com destrutor está vivo nesses quadros de pilha.
    unsafe { leave_user() }
}

/// Carrega e executa uma imagem ELF em ring 3 e devolve o motivo do término.
/// Antes de devolver, **toda** página do usuário foi desmapeada e seus
/// frames reciclados (também quando o programa falha). Não escreve nada na
/// tela nem na serial: quem mostra o resultado é o shell.
pub fn run_image(image: &[u8]) -> Result<Termination, LoadError> {
    let elf = elf::parse(image)?;
    let loaded = load(&elf)?;
    // SAFETY: `load` acabou de mapear os segmentos e a pilha, e `elf.entry`
    // foi validado por `elf::parse` como um endereço dentro de um segmento
    // executável; `USER_INITIAL_RSP` é o topo da pilha mapeada. `enter_user`
    // só retorna depois de `terminate`, que restaura o estado do kernel.
    unsafe { enter_user(elf.entry, USER_INITIAL_RSP) };
    unload(&loaded);
    // `enter_user` só retorna via `terminate`, que sempre grava o motivo
    // antes; um `None` aqui seria um bug do kernel, não do programa.
    let reason = TERMINATION
        .lock()
        .take()
        .expect("programa terminou sem registrar o motivo");
    Ok(reason)
}

/// Procura o programa embutido `name` e o executa.
pub fn run(name: &str) -> Result<Termination, RunError> {
    let program = PROGRAMS
        .iter()
        .find(|program| program.name == name)
        .ok_or(RunError::UnknownProgram)?;
    run_image(program.image).map_err(RunError::Load)
}

/// Nomes dos programas embutidos, em ordem alfabética.
pub fn program_names() -> impl Iterator<Item = &'static str> {
    PROGRAMS.iter().map(|program| program.name)
}
