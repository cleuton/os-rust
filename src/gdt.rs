//! GDT (segmento de código do kernel + descritor da TSS) e a TSS (com a
//! pilha dedicada de double fault na Interrupt Stack Table).

use lazy_static::lazy_static;
use x86_64::instructions::segmentation::{Segment, CS};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

/// Índice de software (0-based) da pilha dedicada ao double fault dentro
/// da Interrupt Stack Table da TSS. Reaproveitado por `interrupts.rs`
/// (produção) e pelos testes de integração de double fault.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// Tamanho da pilha dedicada ao double fault: 5 páginas de 4 KiB (20 KiB),
/// com folga generosa sobre o que o handler realmente usa (formatar e
/// escrever uma mensagem de texto curta, sem alocar — FR-011).
const DOUBLE_FAULT_STACK_SIZE: usize = 5 * 4096;

/// Região estática reservada só para a pilha de double fault. Nunca
/// acessada por código Rust diretamente — só o processador a usa, através
/// do ponteiro guardado na TSS (`interrupt_stack_table`, abaixo). Sem
/// página de guarda abaixo dela (fora de escopo deste marco).
static mut DOUBLE_FAULT_STACK: [u8; DOUBLE_FAULT_STACK_SIZE] = [0; DOUBLE_FAULT_STACK_SIZE];

lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            // SAFETY: só o endereço de `DOUBLE_FAULT_STACK` é lido aqui
            // (um ponteiro bruto, nunca uma referência Rust viva) para
            // calcular o valor que a TSS vai guardar; nenhum código Rust
            // lê ou escreve nela — só o processador a acessa depois,
            // através desse endereço.
            let stack_start = VirtAddr::from_ptr(&raw const DOUBLE_FAULT_STACK);
            // A pilha cresce para baixo, então o endereço guardado na IST
            // é o fim do array, não o início.
            stack_start + DOUBLE_FAULT_STACK_SIZE as u64
        };
        tss
    };
}

/// Os dois seletores de segmento que `gdt::init` precisa depois de montar
/// a `GlobalDescriptorTable`: o do código do kernel (para recarregar
/// `CS`) e o da TSS (para `load_tss`).
struct Selectors {
    code_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.append(Descriptor::kernel_code_segment());
        let tss_selector = gdt.append(Descriptor::tss_segment(&TSS));
        (
            gdt,
            Selectors {
                code_selector,
                tss_selector,
            },
        )
    };
}

/// Carrega a GDT do kernel (segmento de código + descritor da TSS),
/// recarrega o registrador `CS` e carrega a TSS. Deve ser chamada uma
/// única vez, durante o boot, antes de `interrupts::init()` (FR-001) e
/// antes de qualquer interrupção ser habilitada.
pub fn init() {
    GDT.0.load();

    // SAFETY: `code_selector`/`tss_selector` vêm da própria `GDT` que
    // acabou de ser carregada na linha acima, então apontam para
    // descritores válidos e ativos; esta função é chamada uma única vez,
    // antes de qualquer interrupção ser habilitada, então não há disputa
    // com nenhum handler já em execução.
    unsafe {
        CS::set_reg(GDT.1.code_selector);
        load_tss(GDT.1.tss_selector);
    }
}
