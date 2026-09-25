//! IDT, exceções de CPU, PIC 8259 e o handler de teclado (IRQ1).

use core::fmt::Write as _;
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::instructions::port::Port;
use x86_64::registers::control::Cr2;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use crate::panic::halt_loop;
use crate::vga_buffer::WRITER;
use crate::{gdt, println, serial_println, VERSION};

/// Offset de vetor do PIC mestre: logo após as 32 exceções reservadas da
/// CPU (vetores 0–31), para que nenhuma IRQ de hardware colida com elas.
const PIC_1_OFFSET: u8 = 32;
/// Offset de vetor do PIC escravo, encadeado logo depois do mestre.
const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

/// Vetor de interrupção da IRQ1 (teclado): offset do PIC mestre + linha 1.
const KEYBOARD_INTERRUPT_VECTOR: u8 = PIC_1_OFFSET + 1;

/// Máscara do PIC mestre: todas as linhas desabilitadas (bit 1), exceto a
/// IRQ1 (bit 0 em zero = habilitada) — mantém o timer (IRQ0) e as demais
/// linhas caladas (só IRQ1 fica habilitada).
const MASTER_PIC_MASK: u8 = 0b1111_1101;
/// Máscara do PIC escravo: todas as linhas desabilitadas.
const SLAVE_PIC_MASK: u8 = 0b1111_1111;

/// Capacidade fixa da fila de scancodes pendentes de processamento.
const SCANCODE_QUEUE_CAPACITY: usize = 16;

static PICS: Mutex<ChainedPics> = Mutex::new(unsafe {
    // SAFETY: 32/40 são os offsets padrão que colocam as IRQs de hardware
    // logo após as 32 exceções reservadas da CPU; nenhum outro código do
    // kernel cria uma segunda instância de `ChainedPics` para os mesmos
    // PICs físicos, então não há dono duplicado dessas portas de I/O.
    ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET)
});

// Invariante do projeto, desde o Marco 3 (`research.md` do Marco 3,
// seção 9; ampliado no Marco 4, `research.md` seção 5): nenhum handler
// registrado nesta IDT aloca ou libera memória do heap.
// `breakpoint_handler`, `invalid_opcode_handler`,
// `general_protection_fault_handler`, `page_fault_handler` e
// `double_fault_handler` só usam `println!`/`serial_println!` (os
// quatro últimos, através de `fatal_exception`, que só formata em
// variáveis de pilha); `keyboard_interrupt_handler` só empilha um byte
// numa fila de tamanho fixo (`ScancodeQueue`, array `[u8; 16]`). É isso
// que garante que o `spin::Mutex` interno do alocador global
// (`src/allocator.rs`) nunca pode ser disputado entre o fluxo principal
// e uma interrupção — um handler novo que precise alocar violaria este
// invariante e exige revisão explícita antes de ser aceito.
lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.general_protection_fault
            .set_handler_fn(general_protection_fault_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        // SAFETY: o índice aponta para a única pilha que `gdt::init()`
        // (já chamado antes de `interrupts::init()`, ver `lib.rs`)
        // reserva na Interrupt Stack Table da TSS para este propósito —
        // nenhum outro handler ou código do kernel usa essa mesma pilha.
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt[KEYBOARD_INTERRUPT_VECTOR].set_handler_fn(keyboard_interrupt_handler);
        idt
    };
}

/// Handler de breakpoint (`int3`): relata a exceção e retorna normalmente,
/// demonstrando que uma exceção de CPU pode ser tratada sem interromper a
/// execução do sistema. Não é fatal: a tela ganha só uma linha curta (a
/// serial continua com a mesma linha de sempre, desde o Marco 1).
extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    serial_println!("[EXCEPTION] breakpoint (int3)");
    println!(
        "[EXCEPTION] breakpoint (#BP) em {:#x}",
        stack_frame.instruction_pointer.as_u64()
    );
}

/// Handler de instrução inválida (`#UD`): fatal, mostra a tela de
/// exceção e para o kernel.
extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    fatal_exception("Invalid Opcode", "#UD", &stack_frame, None, None);
}

/// Handler de proteção geral (`#GP`): fatal, mostra a tela de exceção
/// (com o código de erro) e para o kernel.
extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    fatal_exception(
        "General Protection Fault",
        "#GP",
        &stack_frame,
        Some(error_code),
        None,
    );
}

/// Handler de page fault (`#PF`): fatal, mostra a tela de exceção com o
/// endereço de falha (lido de CR2) e a interpretação do código de erro
/// em palavras, e para o kernel.
extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    let fault_address = Cr2::read_raw();
    fatal_exception(
        "Page Fault",
        "#PF",
        &stack_frame,
        Some(error_code.bits()),
        Some((fault_address, error_code)),
    );
}

/// Handler de double fault: fatal, roda na pilha dedicada da IST
/// (registrada em `IDT`, acima), mostra a tela de exceção e para o
/// kernel — nunca mais um reinício silencioso do QEMU por estouro de
/// pilha, independentemente do estado da pilha que estava em uso.
extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    fatal_exception("Double Fault", "#DF", &stack_frame, None, None);
}

/// Mostra a tela de exceção fatal (tela + serial, FR-009) compartilhada
/// pelas quatro exceções fatais (`#UD`, `#GP`, `#PF`, `#DF`), no mesmo
/// estilo visual da tela de panic, com o cabeçalho `[EXCEPTION]` para
/// distinguir as duas. Contém sempre nome/sigla,
/// endereço da instrução e a versão do os-rust (FR-007); `error_code`
/// aparece quando a exceção tem um (`#GP`, `#PF`); `page_fault_info`
/// aparece só para `#PF`, com o endereço de falha e a interpretação em
/// palavras do código de erro (FR-008). Nunca aloca memória do heap
/// (FR-011) — só formata em variáveis de pilha. Nunca retorna: termina
/// sempre parando a CPU de forma controlada (FR-007).
fn fatal_exception(
    name: &str,
    mnemonic: &str,
    stack_frame: &InterruptStackFrame,
    error_code: Option<u64>,
    page_fault_info: Option<(u64, PageFaultErrorCode)>,
) -> ! {
    if crate::panic::enter_fatal_handler() {
        // Um panic ou outra exceção fatal já estava em andamento (por
        // exemplo, um bug nesta própria função provocando uma nova
        // falha) — o Mutex do WRITER pode já estar travado pelo evento
        // anterior, então não tentamos travá-lo de novo; só a serial
        // (Edge Case "exceção com locks ocupados").
        serial_println!(
            "[EXCEPTION] {} ({}) reentrante — {} parou",
            name,
            mnemonic,
            VERSION
        );
        halt_loop();
    }

    let address = stack_frame.instruction_pointer.as_u64();

    // Interpretação em palavras do código de erro de page fault (FR-008),
    // calculada uma única vez e reaproveitada na tela e na serial.
    let page_fault_words = page_fault_info.map(|(fault_address, code)| {
        let acesso = if code.contains(PageFaultErrorCode::CAUSED_BY_WRITE) {
            "escrita"
        } else {
            "leitura"
        };
        let causa = if code.contains(PageFaultErrorCode::PROTECTION_VIOLATION) {
            "violacao de protecao"
        } else {
            "pagina ausente"
        };
        (fault_address, acesso, causa)
    });

    serial_println!("[EXCEPTION] {} ({})", name, mnemonic);
    serial_println!("endereco da instrucao: {:#x}", address);
    if let Some(code) = error_code {
        serial_println!("codigo de erro: {:#x}", code);
    }
    if let Some((fault_address, acesso, causa)) = page_fault_words {
        serial_println!("endereco de falha: {:#x}", fault_address);
        serial_println!("acesso: {}", acesso);
        serial_println!("causa: {}", causa);
    }
    serial_println!("{}", VERSION);

    let mut writer = WRITER.lock();
    // `write!` sobre `Writer` nunca falha de verdade (ver
    // `vga_buffer.rs`), então ignorar o `Result` aqui não esconde nenhum
    // erro real possível.
    let _ = write!(
        writer,
        "\n[EXCEPTION] {} ({}) - {} parou\n",
        name, mnemonic, VERSION
    );
    let _ = write!(writer, "endereco da instrucao: {:#x}\n", address);
    if let Some(code) = error_code {
        let _ = write!(writer, "codigo de erro: {:#x}\n", code);
    }
    if let Some((fault_address, acesso, causa)) = page_fault_words {
        let _ = write!(writer, "endereco de falha: {:#x}\n", fault_address);
        let _ = write!(writer, "acesso: {}\n", acesso);
        let _ = write!(writer, "causa: {}\n", causa);
    }
    drop(writer);

    halt_loop();
}

/// Handler de IRQ1 (teclado). Só lê o scancode bruto da porta `0x60`, o
/// enfileira e sinaliza o fim da interrupção ao PIC — nenhuma tradução,
/// eco ou interpretação de comando acontece aqui, para manter o handler
/// curto e para que ele nunca tente travar o `Writer` VGA global enquanto
/// o fluxo principal já o estiver usando.
extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    let mut port: Port<u8> = Port::new(0x60);

    // SAFETY: 0x60 é a porta de dados fixa do controlador de teclado 8042;
    // esta interrupção só dispara quando há um byte de scancode pronto
    // para ser lido nela.
    let scancode: u8 = unsafe { port.read() };

    SCANCODE_QUEUE.lock().push(scancode);

    // SAFETY: sem este EOI, o PIC nunca libera a linha IRQ1 e o teclado
    // para de gerar novas interrupções depois da primeira tecla.
    unsafe {
        PICS.lock().notify_end_of_interrupt(KEYBOARD_INTERRUPT_VECTOR);
    }
}

/// Fila circular de tamanho fixo que separa a captura do scancode (handler
/// de IRQ1, produtor) do processamento no fluxo principal do kernel
/// (consumidor) — ver `research.md`, seção 4.
struct ScancodeQueue {
    buffer: [u8; SCANCODE_QUEUE_CAPACITY],
    head: usize,
    tail: usize,
    len: usize,
}

impl ScancodeQueue {
    const fn new() -> Self {
        ScancodeQueue {
            buffer: [0; SCANCODE_QUEUE_CAPACITY],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    /// Adiciona um scancode à fila. Se ela já estiver cheia, descarta o
    /// scancode mais antigo em vez de travar: perder um byte sob digitação
    /// anormalmente rápida é aceitável, travar o sistema não é.
    fn push(&mut self, scancode: u8) {
        if self.len == SCANCODE_QUEUE_CAPACITY {
            self.head = (self.head + 1) % SCANCODE_QUEUE_CAPACITY;
            self.len -= 1;
        }
        self.buffer[self.tail] = scancode;
        self.tail = (self.tail + 1) % SCANCODE_QUEUE_CAPACITY;
        self.len += 1;
    }

    fn pop(&mut self) -> Option<u8> {
        if self.len == 0 {
            return None;
        }
        let scancode = self.buffer[self.head];
        self.head = (self.head + 1) % SCANCODE_QUEUE_CAPACITY;
        self.len -= 1;
        Some(scancode)
    }
}

static SCANCODE_QUEUE: Mutex<ScancodeQueue> = Mutex::new(ScancodeQueue::new());

/// Remove e retorna o próximo scancode pendente, se houver.
///
/// Só deve ser chamada fora do contexto de interrupção, dentro de
/// `x86_64::instructions::interrupts::without_interrupts` (ver o laço
/// ocioso em `main.rs`): isso garante que a IRQ1 nunca dispare durante o
/// acesso à fila pelo fluxo principal, então o `Mutex` acima nunca fica
/// de fato disputado com o handler.
pub fn next_scancode() -> Option<u8> {
    SCANCODE_QUEUE.lock().pop()
}

/// Carrega a IDT e inicializa o PIC 8259, deixando apenas a IRQ1 (teclado)
/// habilitada. Deve ser chamada uma única vez, antes de habilitar
/// interrupções globalmente.
pub fn init() {
    IDT.load();

    // SAFETY: chamada uma única vez, antes de qualquer interrupção ser
    // habilitada (a primeira habilitação só acontece no laço ocioso de
    // `kernel_main`, via `enable_and_hlt`), então não há disputa com um
    // handler já em execução; os offsets configurados em `PICS` não
    // colidem com as exceções da CPU.
    unsafe {
        PICS.lock().initialize();
        PICS.lock().write_masks(MASTER_PIC_MASK, SLAVE_PIC_MASK);
    }
}

#[cfg(test)]
mod tests {
    #[test_case]
    fn breakpoint_e_tratado_sem_interromper_a_execucao() {
        // `os_rust::init()` (chamado antes da suíte) já carregou a IDT.
        // Se o handler de breakpoint não retornasse normalmente, esta
        // instrução nunca seria alcançada e o teste travaria até o
        // tempo máximo de execução esgotar.
        x86_64::instructions::interrupts::int3();
    }
}
