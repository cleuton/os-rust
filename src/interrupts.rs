//! IDT, exceções de CPU, PIC 8259 e o handler de teclado (IRQ1).

use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::instructions::port::Port;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use crate::panic::halt_loop;
use crate::println;

/// Offset de vetor do PIC mestre: logo após as 32 exceções reservadas da
/// CPU (vetores 0–31), para que nenhuma IRQ de hardware colida com elas.
const PIC_1_OFFSET: u8 = 32;
/// Offset de vetor do PIC escravo, encadeado logo depois do mestre.
const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

/// Vetor de interrupção da IRQ1 (teclado): offset do PIC mestre + linha 1.
const KEYBOARD_INTERRUPT_VECTOR: u8 = PIC_1_OFFSET + 1;

/// Máscara do PIC mestre: todas as linhas desabilitadas (bit 1), exceto a
/// IRQ1 (bit 0 em zero = habilitada) — mantém o timer (IRQ0) e as demais
/// linhas caladas, conforme a constitution (só IRQ1 fica habilitada).
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

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.double_fault.set_handler_fn(double_fault_handler);
        idt[KEYBOARD_INTERRUPT_VECTOR].set_handler_fn(keyboard_interrupt_handler);
        idt
    };
}

/// Handler de breakpoint (`int3`): relata a exceção e retorna normalmente,
/// demonstrando que uma exceção de CPU pode ser tratada sem interromper a
/// execução do sistema.
extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("[EXCEPTION] breakpoint (int3)\n{:#?}", stack_frame);
}

/// Handler de double fault: mostra uma mensagem legível e para a CPU, para
/// que uma falha inesperada nunca vire um reinício silencioso em loop —
/// mesmo espírito do tratamento de panic da v1. Sem pilha dedicada (IST):
/// um double fault causado por estouro da própria pilha do kernel fica
/// fora do que este handler consegue tratar de forma confiável (fora de
/// escopo, conforme a constitution).
extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    println!("[DOUBLE FAULT] proto-os parou\n{:#?}", stack_frame);
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
