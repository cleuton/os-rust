//! O escalonador: quem decide qual programa de usuário usa a CPU e faz a troca.
//!
//! O desenho inteiro cabe em uma ideia: **toda troca de tarefa acontece numa
//! fronteira ring 3 → ring 0** (uma syscall, uma exceção ou, mais tarde, o
//! timer). Nesse momento a pilha de entrada do kernel está vazia e o estado do
//! programa está inteiro num `TaskContext` no topo dela. Trocar é copiar esse
//! contexto para a `Task` que sai e carregar o de outra, com `iretq`. Por isso
//! não existe pilha de kernel por tarefa: o kernel nunca precisa guardar nada
//! dele mesmo entre duas trocas.
//!
//! A ordem de escolha é um rodízio (round-robin): o próximo a rodar é o
//! primeiro `Ready` depois do que rodou por último, em ordem circular.
//!
//! Regra de ouro deste arquivo: **nunca retomar uma tarefa (`resume_task`) nem
//! sair do escalonador (`leave_user`) com o guarda do `SCHED` vivo.** Quem
//! retoma não volta para soltar o lock, e o próximo `lock()` travaria para
//! sempre. Por isso as funções copiam o que precisam, soltam o guarda e só
//! então retomam.
//!
//! Sobre alocar aqui: as funções deste módulo só rodam depois de uma entrada
//! vinda de ring 3 (ou do laço de partida, com as interrupções desligadas). Em
//! ring 3 nenhum código do kernel está no meio de uma operação, então nenhum
//! lock (heap, alocador de frames, tela) está preso, e liberar um `Box<Task>`
//! ou devolver frames é seguro. Handlers disparados em ring 0 continuam
//! proibidos de alocar.

use alloc::boxed::Box;
use alloc::vec::Vec;
use spin::Mutex;

use abi::MAX_TASKS;

use x86_64::instructions::interrupts;

use crate::keyboard::{LineEditor, LINE_CAPACITY};
use crate::memory;
use crate::task::{PendingRead, Task, TaskContext, TaskState};
use crate::timer;
use crate::user::{self, Finished, Termination};

/// Slot vazio, para inicializar o vetor de tarefas em `const`.
const NENHUMA: Option<Box<Task>> = None;
/// Término ainda não ocorrido, para inicializar o vetor de resultados.
const NENHUM: Option<Finished> = None;

/// O estado global do escalonador.
struct Scheduler {
    /// A tabela de tarefas; o índice é o identificador da tarefa. Um slot vazio
    /// é uma tarefa que terminou (ou que nunca existiu).
    tasks: [Option<Box<Task>>; MAX_TASKS],
    /// A tarefa em execução agora, se há alguma.
    current: Option<usize>,
    /// Valor do contador de ticks quando a tarefa atual foi colocada na CPU: a
    /// fatia dela conta a partir daqui (`SLICE_TICKS`).
    slice_start: u64,
    /// Índice da última tarefa colocada na CPU: o rodízio continua dali.
    last: usize,
    /// Os términos, na ordem em que aconteceram. Vetor fixo: é escrito em
    /// contexto de exceção e não pode depender de crescer.
    finished: [Option<Finished>; MAX_TASKS],
    finished_len: usize,
    /// Quem é avisado a cada término (o shell mostra a mensagem na hora).
    on_end: Option<fn(&Finished)>,
    /// Número de ordem do próximo pedido de teclado: o menor número entre os
    /// que esperam é o que pediu primeiro.
    next_ticket: u64,
    /// A linha em montagem para o programa que pediu primeiro.
    editor: LineEditor,
    /// De quem é a linha que o editor está montando agora.
    editor_owner: Option<usize>,
    /// Quantas trocas foram causadas pelo timer.
    preemptions: u32,
    /// Quantas voltas o laço ocioso deu.
    idle_loops: u32,
}

impl Scheduler {
    const fn new() -> Scheduler {
        Scheduler {
            tasks: [NENHUMA; MAX_TASKS],
            current: None,
            slice_start: 0,
            // Com `last` no fim da tabela, a primeira varredura começa no
            // índice 0: a primeira tarefa a rodar é a do primeiro nome dado.
            last: MAX_TASKS - 1,
            finished: [NENHUM; MAX_TASKS],
            finished_len: 0,
            on_end: None,
            next_ticket: 0,
            editor: LineEditor::new(),
            editor_owner: None,
            preemptions: 0,
            idle_loops: 0,
        }
    }

    /// O primeiro `Ready` depois de `last`, em ordem circular (a própria
    /// tarefa que rodou por último entra por último na varredura).
    fn next_ready(&self) -> Option<usize> {
        (1..=MAX_TASKS)
            .map(|passo| (self.last + passo) % MAX_TASKS)
            .find(|&i| {
                matches!(
                    self.tasks[i].as_ref().map(|t| t.state),
                    Some(TaskState::Ready)
                )
            })
    }

    /// Verdadeiro se ainda existe alguma tarefa, em qualquer estado.
    fn any_alive(&self) -> bool {
        self.tasks.iter().any(|t| t.is_some())
    }

    /// A tarefa que pediu primeiro uma linha e ainda não a recebeu: a de menor
    /// número de ordem entre as que esperam o teclado.
    fn first_waiter(&self) -> Option<usize> {
        (0..MAX_TASKS)
            .filter_map(|i| {
                let task = self.tasks[i].as_ref()?;
                let read = task.read.as_ref()?;
                (task.state == TaskState::WaitingKeyboard).then_some((read.ticket, i))
            })
            .min()
            .map(|(_, index)| index)
    }

    /// Verdadeiro se alguma tarefa espera uma linha do teclado.
    fn any_waiting(&self) -> bool {
        self.tasks.iter().flatten().any(|t| t.state == TaskState::WaitingKeyboard)
    }
}

static SCHED: Mutex<Scheduler> = Mutex::new(Scheduler::new());

/// O gancho de teste (`set_poll_hook`). Mora fora do `Scheduler` para
/// sobreviver a `start`, que reinicia o escalonador a cada `run`.
static POLL_HOOK: Mutex<Option<fn()>> = Mutex::new(None);

/// Registra (ou, com `None`, remove) uma função chamada no começo de **toda**
/// `poll_keyboard()` (bloqueio, `yield`, tick vindo de ring 3, volta do laço
/// ocioso), sempre com as interrupções desligadas. Existe para os testes
/// "digitarem" no meio da execução, exatamente quando o kernel olha o teclado:
/// depois que o kernel já dormiu (para provar que não há laço ocupado) e
/// enquanto um programa computa (para provar que o tick acorda quem espera).
/// O gancho costuma usar `interrupts::push_scancode`, que exige `IF = 0`.
pub fn set_poll_hook(hook: Option<fn()>) {
    *POLL_HOOK.lock() = hook;
}

/// Instala as tarefas (todas prontas) e as executa até a última terminar.
/// Volta a quem chamou quando `leave_user` devolve o controle; todos os
/// espaços de endereçamento já foram destruídos e o `CR3` é o do kernel.
pub fn start(tasks: Vec<Task>, on_end: Option<fn(&Finished)>) {
    {
        let mut sched = SCHED.lock();
        *sched = Scheduler::new();
        sched.on_end = on_end;
        for (slot, task) in sched.tasks.iter_mut().zip(tasks) {
            *slot = Some(Box::new(task));
        }
    }
    // SAFETY: as tarefas já estão instaladas e cada uma tem um espaço de
    // endereçamento válido e um contexto inicial com seletores de usuário.
    // `enter_user` só retorna depois que `run_next` chamou `leave_user`, com
    // o estado do kernel restaurado.
    unsafe { user::enter_user() };
}

/// Entrega o que aconteceu na rodada que acabou de terminar (os términos em
/// ordem, as trocas causadas pelo timer e as voltas ociosas).
pub fn take_report() -> (Vec<Finished>, u32, u32) {
    let sched = SCHED.lock();
    let finished = sched.finished[..sched.finished_len]
        .iter()
        .filter_map(|f| *f)
        .collect();
    (finished, sched.preemptions, sched.idle_loops)
}

/// Quantas páginas de heap a tarefa atual já recebeu.
pub(crate) fn current_heap_pages() -> u64 {
    let sched = SCHED.lock();
    sched
        .current
        .and_then(|i| sched.tasks[i].as_ref())
        .map_or(0, |task| task.heap_pages)
}

/// Roda `f` com a tabela de arquivos abertos da tarefa atual. `None` se não há
/// tarefa atual (só aconteceria fora de uma syscall). A trava do escalonador
/// fica presa durante `f`; é seguro porque a syscall roda com as interrupções
/// desligadas, e nada que interrompa ring 0 usa o escalonador.
pub(crate) fn with_current_files<R>(f: impl FnOnce(&mut crate::fs::FileTable) -> R) -> Option<R> {
    let mut sched = SCHED.lock();
    let index = sched.current?;
    sched.tasks[index].as_mut().map(|task| f(&mut task.files))
}

/// Grava quantas páginas de heap a tarefa atual tem.
pub(crate) fn set_current_heap_pages(pages: u64) {
    let mut sched = SCHED.lock();
    if let Some(task) = sched.current.and_then(|i| sched.tasks[i].as_mut()) {
        task.heap_pages = pages;
    }
}

/// Olha o teclado em nome de quem espera uma linha. Chamada sempre com as
/// interrupções desligadas (a fila de scancodes só é tocada assim).
///
/// Só esvazia a fila **enquanto há um programa esperando**: o que ninguém pediu
/// fica na fila para o prompt, como no Marco 6. A linha é montada para o
/// programa que pediu primeiro (o de menor número de ordem); quando o Enter a
/// completa, ela fica guardada na tarefa, que passa a `Ready`, e as teclas
/// seguintes valem para o próximo da espera. Uma tecla nunca vai a dois
/// programas, e nenhuma se perde.
fn poll_keyboard() {
    // Copia o gancho antes de chamá-lo: ele pode empurrar scancodes, e não
    // pode ser chamado com o lock do escalonador preso.
    let hook = *POLL_HOOK.lock();
    if let Some(hook) = hook {
        hook();
    }

    let mut sched = SCHED.lock();
    loop {
        let Some(owner) = sched.first_waiter() else { return };

        // Linha nova para quem agora é o primeiro da espera.
        if sched.editor_owner != Some(owner) {
            let max_chars = sched.tasks[owner]
                .as_ref()
                .and_then(|task| task.read.as_ref())
                .map_or(0, |read| (read.len as usize).min(LINE_CAPACITY).saturating_sub(1));
            sched.editor.reset(max_chars);
            sched.editor_owner = Some(owner);
        }

        let Some(scancode) = crate::interrupts::next_scancode() else { return };
        let finished_line = sched.editor.feed(scancode);
        if let Some(written) = finished_line {
            let Scheduler { editor, tasks, editor_owner, .. } = &mut *sched;
            if let Some(task) = tasks[owner].as_mut() {
                if let Some(read) = task.read.as_mut() {
                    read.line[..written].copy_from_slice(editor.line(written));
                    read.written = written;
                }
                task.state = TaskState::Ready;
            }
            *editor_owner = None;
        }
    }
}

/// O que `run_next` decidiu fazer, depois de olhar a tabela de tarefas.
enum Decision {
    /// Colocar esta tarefa na CPU.
    Run(*const TaskContext),
    /// Ninguém pronto, mas alguém espera o teclado: dormir até uma interrupção.
    Idle,
    /// Não resta nenhuma tarefa.
    Done,
}

/// Escolhe a próxima tarefa e a coloca na CPU; nunca retorna. É o ponto único
/// por onde toda tarefa começa ou volta a rodar, e por isso é aqui (e só aqui)
/// que o espaço de endereçamento dela é ativado e que a linha que ela esperava
/// do teclado é entregue. É `extern "C"` porque o assembly de `enter_user`
/// salta para ela.
///
/// Se nenhuma tarefa está pronta mas alguma espera o teclado, o kernel **dorme**
/// (`sti; hlt`) até a próxima interrupção, em vez de girar num laço ocupado: cada
/// volta do laço é provocada por uma interrupção (um tick ou uma tecla).
pub(crate) extern "C" fn run_next() -> ! {
    loop {
        // As interrupções ficam desligadas de ponta a ponta, menos durante o
        // `hlt` do ocioso.
        interrupts::disable();
        poll_keyboard();

        // Decide com o lock preso; só guarda o ponteiro do contexto. O
        // `Box<Task>` não sai do lugar, e ninguém o libera enquanto ele roda
        // (uma CPU só, sem preempção do kernel).
        let decision = {
            let mut sched = SCHED.lock();
            match sched.next_ready() {
                Some(index) => {
                    sched.current = Some(index);
                    sched.last = index;
                    sched.slice_start = timer::ticks();
                    match sched.tasks[index].as_mut() {
                        Some(task) => {
                            task.state = TaskState::Running;
                            task.slices += 1;
                            task.space.activate();
                            deliver_line(task);
                            Decision::Run(&task.context as *const TaskContext)
                        }
                        None => Decision::Idle,
                    }
                }
                None if sched.any_waiting() => {
                    sched.idle_loops += 1;
                    Decision::Idle
                }
                None => Decision::Done,
            }
        };

        match decision {
            // SAFETY: o ponteiro é do contexto de uma tarefa viva (ver acima), com
            // `cs`/`ss` de usuário e `IF` ligado nas flags; o guarda do `SCHED` já
            // foi solto, como a regra de ouro do módulo exige.
            Decision::Run(context) => unsafe { user::resume_task(context) },
            // `sti; hlt` como uma instrução atômica: uma tecla ou um tick que
            // chegue entre o teste e o `hlt` acorda a CPU em vez de ficar
            // esquecido. O `hlt` não segura nenhum lock. Ao acordar, o topo do
            // laço desliga as interrupções de novo.
            Decision::Idle => interrupts::enable_and_hlt(),
            Decision::Done => finish(),
        }
    }
}

/// Entrega à tarefa que vai voltar a rodar a linha que ela esperava, se a linha
/// já ficou pronta: copia para o buffer dela (que está mapeado agora, porque o
/// espaço dela acabou de ser ativado) e grava o resultado de `SYS_READ_LINE`
/// em `rax`.
fn deliver_line(task: &mut Task) {
    let Some(read) = task.read.take() else { return };
    if read.written == 0 {
        return;
    }
    // SAFETY: `sys_read_line` validou o intervalo `ptr..ptr + len` (dentro da
    // região do usuário, mapeado, `USER_ACCESSIBLE` e gravável) antes de
    // bloquear a tarefa, e nada desmapeia páginas de uma tarefa que espera
    // (uma CPU só, nenhuma syscall desmapeia). `written <= min(len, 128)`, então
    // a cópia cabe no intervalo; o buffer do kernel (dentro da `Task`) e a
    // memória do programa nunca se sobrepõem. O espaço da tarefa está ativo.
    unsafe {
        core::ptr::copy_nonoverlapping(read.line.as_ptr(), read.ptr as *mut u8, read.written);
    }
    task.context.rax = read.written as u64;
}

/// Não resta nenhuma tarefa: volta ao kernel.
fn finish() -> ! {
    debug_assert!(!SCHED.lock().any_alive());
    memory::activate_kernel();
    // SAFETY: `enter_user` guardou o estado do kernel e ainda não retornou (é
    // quem chamou o escalonador); nenhum guarda do `SCHED` está vivo aqui.
    unsafe { user::leave_user() }
}

/// `SYS_READ_LINE`, depois de validado: bloqueia a tarefa atual até o Enter e
/// passa a CPU adiante. O resultado (os bytes escritos) só é gravado quando a
/// linha ficar pronta, por `deliver_line`. Nunca retorna.
pub(crate) fn block_on_keyboard(ctx: &mut TaskContext, ptr: u64, len: u64) -> ! {
    {
        let mut sched = SCHED.lock();
        let ticket = sched.next_ticket;
        sched.next_ticket += 1;
        if let Some(task) = sched.current.and_then(|i| sched.tasks[i].as_mut()) {
            task.context = *ctx;
            task.state = TaskState::WaitingKeyboard;
            task.read = Some(PendingRead {
                ptr,
                len,
                ticket,
                line: [0; LINE_CAPACITY],
                written: 0,
            });
        }
    }
    run_next()
}

/// Encerra a tarefa atual, por `exit`, por falha ou por syscall inexistente:
/// registra o motivo, avisa quem pediu, devolve todos os recursos dela e passa
/// a CPU adiante. Nunca retorna.
///
/// O isolamento de falhas (`SYSCALLS.md`, seção 7) depende de três coisas que
/// acontecem aqui: (a) o aviso de término, que o shell transforma em mensagem
/// com o **nome** do programa, sai na hora, antes de os outros acabarem; (b)
/// ele vai à tela e à serial (o shell escreve nos dois); (c) o espaço de
/// endereçamento e os frames só desta tarefa voltam ao alocador agora, enquanto
/// as outras tarefas continuam vivas e intactas.
pub(crate) fn terminate_current(reason: Termination) -> ! {
    // Sai do espaço da tarefa antes de destruí-lo.
    memory::activate_kernel();

    let (task, finished, on_end) = {
        let mut sched = SCHED.lock();
        let index = sched.current.take();
        let task = index.and_then(|i| sched.tasks[i].take());
        let finished = task.as_ref().map(|task| Finished {
            name: task.name,
            termination: reason,
            slices: task.slices,
        });
        if let Some(finished) = finished {
            let len = sched.finished_len;
            sched.finished[len] = Some(finished);
            sched.finished_len = len + 1;
        }
        (task, finished, sched.on_end)
    };

    // O aviso sai antes de os recursos voltarem: quem o recebe vê o estado
    // com a tarefa ainda dona dos frames dela.
    if let (Some(on_end), Some(finished)) = (on_end, finished) {
        on_end(&finished);
    }
    if let Some(task) = task {
        let Task { space, .. } = *task;
        space.destroy();
    }
    run_next()
}

/// O tick do timer interrompeu a tarefa atual (em ring 3). Antes de decidir,
/// olha o teclado: uma tecla digitada enquanto um programa computa precisa
/// acordar quem espera uma linha, mesmo que ninguém mais troque de tarefa (sem
/// isto, um programa esperando teclado só receberia a linha depois que o outro
/// terminasse). Depois, só tira a CPU da tarefa se ela já usou a fatia
/// (`SLICE_TICKS` ticks desde que foi colocada na CPU, ver `abi`): um programa
/// que cede a CPU logo, como `ping`, nunca acumula uma fatia só com os ticks
/// que o retorno de cada syscall entrega, e a ordem dos programas cooperativos
/// é a do rodízio. Se a fatia acabou e há outra tarefa pronta, passa a CPU a
/// ela; senão a mesma continua.
pub(crate) fn preempt(ctx: &mut TaskContext) {
    poll_keyboard();
    let slice_start = SCHED.lock().slice_start;
    if timer::ticks().saturating_sub(slice_start) < abi::SLICE_TICKS {
        return;
    }
    switch_if_other_ready(ctx, true);
}

/// `SYS_YIELD`: cede a CPU. Se não há outra tarefa pronta, a mesma continua.
pub(crate) fn yield_now(ctx: &mut TaskContext) {
    ctx.rax = 0;
    poll_keyboard();
    switch_if_other_ready(ctx, false);
}

/// Se há outra tarefa pronta, guarda `ctx` na tarefa atual e passa a CPU
/// adiante (não retorna); senão volta, e o stub retoma a mesma tarefa.
fn switch_if_other_ready(ctx: &mut TaskContext, by_timer: bool) {
    {
        let mut sched = SCHED.lock();
        let Some(current) = sched.current else { return };
        if sched.next_ready().is_none() {
            return;
        }
        if by_timer {
            sched.preemptions += 1;
        }
        if let Some(task) = sched.tasks[current].as_mut() {
            task.context = *ctx;
            task.state = TaskState::Ready;
        }
    }
    run_next()
}
