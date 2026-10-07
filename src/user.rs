//! Programas de usuário (ring 3): layout de memória, carregador e execução.
//!
//! Este módulo reúne o que o kernel sabe sobre onde um programa vive: a
//! região de endereços do usuário (código e dados, heap e pilha), a pilha
//! inicial e o estado dos registradores na primeira instrução, além de
//! carregar um ou vários programas (cada um num espaço de endereçamento
//! próprio) e entregá-los ao escalonador. Tudo aqui está documentado em
//! `SYSCALLS.md`: um programa só pode depender do que está naquele arquivo.

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

/// Primeiro byte do heap do programa: 1,5 GiB. Fica dentro da mesma entrada
/// `P3[1]` que já contém o resto da região do usuário (nenhuma tabela de
/// página nova de nível alto) e deixa `[USER_REGION_START, USER_HEAP_START)`
/// (512 MiB) para código e dados, muito além do que qualquer programa daqui
/// precisa. O carregador recusa segmentos que passem daí (`elf.rs`).
pub const USER_HEAP_START: u64 = 0x6000_0000;

/// Máximo de páginas do heap de um programa: 256, ou seja, 1 MiB. Um limite
/// fixo torna testável "pedir mais memória do que resta" sem depender do
/// tamanho da RAM da máquina.
pub const USER_HEAP_MAX_PAGES: u64 = 256;

/// Fim (exclusivo) da janela do heap. A página que começa aqui nunca é
/// mapeada: uma escrita além do fim do heap vira `#PF`, não corrupção.
pub const USER_HEAP_END: u64 = USER_HEAP_START + USER_HEAP_MAX_PAGES * 4096;

/// Valor de `rflags` na primeira instrução do programa: bit reservado 1
/// sempre ligado e `IF` (interrupções habilitadas) ligado, o resto zero. O
/// `iretq` que coloca o programa em ring 3 carrega `rflags` do contexto.
pub const USER_RFLAGS: u64 = 0x202;

use alloc::vec::Vec;

use abi::{ERR_INVAL, ERR_NOMEM, MAX_TASKS};
use x86_64::structures::paging::{Page, PageTableFlags, Size4KiB};
use x86_64::VirtAddr;

use crate::elf::{self, ElfImage, LoadError};
use crate::memory::{self, AddressSpace};
use crate::programs::PROGRAMS;
use crate::scheduler;
use crate::syscall::SAVED_KERNEL_RSP;
use crate::task::{ProgramName, Task, TaskContext};

/// Por que o programa deixou de rodar. Guardado pelo escalonador quando a
/// tarefa termina e entregue ao shell, que o transforma em mensagem. Não
/// contém nada alocado: é escrito dentro de handlers, onde o heap nunca é usado.
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

/// O registro de um programa que terminou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finished {
    /// Nome do programa, como foi dado a `run`.
    pub name: ProgramName,
    /// Por que terminou.
    pub termination: Termination,
    /// Quantas vezes foi colocado na CPU antes de terminar.
    pub slices: u32,
}

/// O que aconteceu em uma rodada de `run`: os términos, na ordem em que
/// ocorreram, e contadores que os testes usam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    pub finished: Vec<Finished>,
    /// Trocas de tarefa causadas pelo timer (nenhuma até a preempção existir).
    pub preemptions: u32,
    /// Voltas do laço ocioso do kernel (todos os programas esperando teclado).
    pub idle_loops: u32,
}

/// Por que `run` não conseguiu executar o que foi pedido.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunError {
    /// O nome não corresponde a nenhum programa embutido.
    UnknownProgram,
    /// Mais programas que `MAX_TASKS`; nenhum foi iniciado.
    TooManyPrograms { max: usize },
    /// O programa existe, mas não pôde ser carregado (execução de um só).
    Load(LoadError),
    /// O programa na posição `index` do pedido não pôde ser carregado; nenhum
    /// programa foi iniciado.
    LoadFailed { index: usize, error: LoadError },
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
/// para o chamador preencher. Mapeia no espaço de endereçamento **ativo**.
fn map_zeroed_page(
    vaddr: u64,
    flags: PageTableFlags,
) -> Result<x86_64::structures::paging::PhysFrame<Size4KiB>, LoadError> {
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(vaddr));
    let frame = memory::map_user_page(page, flags).map_err(map_error)?;
    memory::zero_frame(frame);
    Ok(frame)
}

/// Cria um espaço de endereçamento novo e carrega nele os segmentos do
/// programa e a pilha: para cada página, aloca um frame, zera, copia os bytes
/// do arquivo e mapeia com as permissões do segmento (W^X). Volta sempre ao
/// espaço do kernel antes de devolver. Em qualquer falha, o espaço inteiro é
/// destruído (os frames voltam ao alocador): nunca sobra nada pela metade.
fn load(image: &ElfImage) -> Result<AddressSpace, LoadError> {
    if !memory::claim_user_region() {
        return Err(LoadError::RegionBusy);
    }
    let space = AddressSpace::new().map_err(map_error)?;
    space.activate();
    let result = load_into(image);
    memory::activate_kernel();
    match result {
        Ok(()) => Ok(space),
        Err(error) => {
            space.destroy();
            Err(error)
        }
    }
}

fn load_into(image: &ElfImage) -> Result<(), LoadError> {
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
        for index in 0..pages {
            let frame = map_zeroed_page(segment.vaddr + index * 4096, flags)?;
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
    for index in 0..USER_STACK_PAGES {
        map_zeroed_page(USER_STACK_BOTTOM + index * 4096, stack_flags)?;
    }
    Ok(())
}

/// `SYS_ALLOC`: amplia o heap do programa em `size` bytes (arredondados para
/// cima até um múltiplo de 4 KiB) e devolve o endereço do início da área
/// nova, que é contígua às anteriores. A área nasce zerada e é gravável,
/// nunca executável. Tudo ou nada: se o total passaria de
/// `USER_HEAP_MAX_PAGES`, ou se faltarem frames, devolve `ERR_NOMEM` e o heap
/// fica como estava. Mapeia no espaço ativo, que durante uma syscall é o da
/// tarefa que a chamou.
pub(crate) fn grow_heap(size: u64) -> Result<u64, i64> {
    if size == 0 {
        return Err(ERR_INVAL);
    }
    // `div_ceil` não estoura, mesmo com um `size` enorme.
    let pages = size.div_ceil(4096);
    let used = scheduler::current_heap_pages();
    if pages > USER_HEAP_MAX_PAGES - used {
        return Err(ERR_NOMEM);
    }

    let start = USER_HEAP_START + used * 4096;
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_EXECUTE;
    for index in 0..pages {
        if map_zeroed_page(start + index * 4096, flags).is_err() {
            // Faltou frame: desfaz só as páginas mapeadas **nesta chamada**.
            // Devolver frames (`recycle`) pode alocar no heap do kernel, o que
            // é permitido aqui: uma syscall roda com as interrupções
            // desligadas e nenhum lock do heap está preso por quem a
            // interrompeu. O que não pode alocar é um handler de interrupção.
            memory::unmap_user_range(VirtAddr::new(start), index);
            return Err(ERR_NOMEM);
        }
    }
    scheduler::set_current_heap_pages(used + pages);
    Ok(start)
}

extern "C" {
    /// Entra no escalonador pela primeira vez e só "retorna" quando a última
    /// tarefa termina (via `leave_user`). Assembly logo abaixo.
    pub(crate) fn enter_user();
    /// Coloca a tarefa de contexto `ctx` em ring 3 (com `iretq`); nunca
    /// retorna. Assembly logo abaixo.
    pub(crate) fn resume_task(ctx: *const TaskContext) -> !;
    /// Volta ao chamador de `enter_user`, abandonando a pilha em uso.
    pub(crate) fn leave_user() -> !;
}

// `enter_user()` e `leave_user()`: o mesmo padrão de `setjmp`/`longjmp` do C,
// com uma única volta. `enter_user` guarda o estado do kernel (registradores
// callee-saved e `rsp`) e salta para o escalonador, que escolhe a primeira
// tarefa e a coloca em ring 3; `leave_user`, chamada pelo escalonador quando
// não resta nenhuma tarefa, restaura esse estado e executa `ret`, que retorna
// a quem chamou `enter_user`, como se ele tivesse terminado normalmente.
//
// `resume_task(ctx)` é o único jeito de uma tarefa entrar em ring 3: aponta
// `rsp` para o `TaskContext`, desempilha os 15 registradores e deixa o frame
// de `rip, cs, rflags, rsp, ss` para o `iretq` consumir.
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
    // Nenhuma interrupção pode chegar enquanto o kernel ainda usa a pilha do
    // chamador: o escalonador as mantém desligadas até o `iretq`.
    "cli",
    // Na entrada, `rsp` é 8 mod 16 (endereço de retorno); os 6 `push` somam 48
    // e mantêm esse resto. A convenção C quer `rsp` múltiplo de 16 **antes**
    // do `call`, então o ajuste de 8 bytes é necessário.
    "sub rsp, 8",
    "call {run_next}",
    // `run_next` nunca retorna.
    "ud2",
    ".global resume_task",
    "resume_task:",
    "mov rsp, rdi",
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
    run_next = sym scheduler::run_next,
);

/// Carrega e executa, ao mesmo tempo, os programas dados (nome e imagem ELF),
/// cada um num espaço de endereçamento próprio, e devolve o que aconteceu
/// quando o **último** terminar. Tudo ou nada: mais programas que `MAX_TASKS`
/// ou qualquer falha de carga recusam o pedido inteiro, sem iniciar nenhum.
/// `on_end`, se dado, é chamado no momento em que cada programa termina
/// (antes de os recursos dele voltarem). Antes de devolver, **toda** página do
/// usuário foi desmapeada e seus frames reciclados, inclusive os dos que
/// falharam. Não escreve nada na tela nem na serial: quem mostra o resultado
/// é o shell.
pub fn run_images(
    images: &[(&str, &[u8])],
    on_end: Option<fn(&Finished)>,
) -> Result<RunReport, RunError> {
    if images.len() > MAX_TASKS {
        return Err(RunError::TooManyPrograms { max: MAX_TASKS });
    }
    let mut tasks: Vec<Task> = Vec::new();
    for (index, (name, image)) in images.iter().enumerate() {
        let loaded = elf::parse(image).and_then(|elf| load(&elf).map(|space| (elf.entry, space)));
        match loaded {
            Ok((entry, space)) => {
                let context = TaskContext::initial(entry, USER_INITIAL_RSP);
                tasks.push(Task::new(ProgramName::new(name), context, space));
            }
            Err(error) => {
                // Nada começou a rodar: devolve o que já foi carregado.
                for task in tasks {
                    task.space.destroy();
                }
                return Err(RunError::LoadFailed { index, error });
            }
        }
    }
    if tasks.is_empty() {
        return Ok(RunReport { finished: Vec::new(), preemptions: 0, idle_loops: 0 });
    }
    scheduler::start(tasks, on_end);
    let (finished, preemptions, idle_loops) = scheduler::take_report();
    Ok(RunReport { finished, preemptions, idle_loops })
}

/// Procura cada nome entre os programas embutidos e executa todos ao mesmo
/// tempo (`run_images`). Se algum nome não existe, devolve `UnknownProgram` e
/// nada é carregado.
pub fn run_all(
    names: &[&str],
    on_end: Option<fn(&Finished)>,
) -> Result<RunReport, RunError> {
    let mut images: Vec<(&str, &[u8])> = Vec::new();
    for name in names {
        let program = PROGRAMS
            .iter()
            .find(|program| program.name == *name)
            .ok_or(RunError::UnknownProgram)?;
        images.push((program.name, program.image));
    }
    run_images(&images, on_end)
}

/// Executa uma imagem só e devolve o motivo do término.
fn run_one(name: &str, image: &[u8]) -> Result<Termination, RunError> {
    let mut report = match run_images(&[(name, image)], None) {
        Ok(report) => report,
        Err(RunError::LoadFailed { error, .. }) => return Err(RunError::Load(error)),
        Err(other) => return Err(other),
    };
    Ok(report
        .finished
        .pop()
        .expect("programa terminou sem registrar o motivo")
        .termination)
}

/// Carrega e executa uma imagem ELF em ring 3 e devolve o motivo do término.
/// É a execução de **um** programa só (como no Marco 6); os testes dos marcos
/// anteriores usam esta função.
pub fn run_image(image: &[u8]) -> Result<Termination, LoadError> {
    match run_one("programa", image) {
        Ok(termination) => Ok(termination),
        Err(RunError::Load(error)) => Err(error),
        // Com uma imagem só, as demais recusas não podem acontecer.
        Err(_) => Err(LoadError::RegionBusy),
    }
}

/// Procura o programa embutido `name` e o executa (um programa só).
pub fn run(name: &str) -> Result<Termination, RunError> {
    let program = PROGRAMS
        .iter()
        .find(|program| program.name == name)
        .ok_or(RunError::UnknownProgram)?;
    run_one(program.name, program.image)
}

/// A imagem ELF do programa embutido `name`, se existir.
pub fn builtin_image(name: &str) -> Option<&'static [u8]> {
    PROGRAMS.iter().find(|program| program.name == name).map(|program| program.image)
}

/// Nomes dos programas embutidos, em ordem alfabética.
pub fn program_names() -> impl Iterator<Item = &'static str> {
    PROGRAMS.iter().map(|program| program.name)
}
