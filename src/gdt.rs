//! GDT (segmentos de código e dados do kernel, segmentos de código e dados
//! do usuário, descritor da TSS) e a TSS (com a pilha dedicada de double
//! fault na Interrupt Stack Table e a pilha do kernel para entradas vindas
//! de ring 3).

use lazy_static::lazy_static;
use x86_64::instructions::segmentation::{Segment, CS, SS};
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
/// escrever uma mensagem de texto curta, sem alocar).
const DOUBLE_FAULT_STACK_SIZE: usize = 5 * 4096;

/// Uma pilha de `N` bytes alinhada a 16. Um `static` de `[u8; N]` tem
/// alinhamento 1, então seu endereço (e o do topo, que é o que o processador e o
/// stub de `syscall` usam) pode cair em qualquer byte; a convenção de chamada
/// (System V) e o próprio stub de `syscall` (`syscall.rs`) pressupõem que o topo
/// da pilha é múltiplo de 16. Com o alinhamento declarado aqui, o topo
/// (`início + N`, com `N` múltiplo de 16) também é múltiplo de 16.
#[repr(align(16))]
struct Stack<const N: usize>([u8; N]);

/// Região estática reservada só para a pilha de double fault. Nunca
/// acessada por código Rust diretamente — só o processador a usa, através
/// do ponteiro guardado na TSS (`interrupt_stack_table`, abaixo). Sem
/// página de guarda abaixo dela (fora de escopo deste marco).
static mut DOUBLE_FAULT_STACK: Stack<DOUBLE_FAULT_STACK_SIZE> =
    Stack([0; DOUBLE_FAULT_STACK_SIZE]);

/// Tamanho da pilha do kernel usada quando o processador entra no kernel
/// vindo de ring 3: 5 páginas de 4 KiB (20 KiB), o mesmo tamanho da pilha
/// de double fault, com folga sobre o que os handlers e o despachante de
/// syscall usam (formatar e escrever uma mensagem curta, sem alocar).
const KERNEL_ENTRY_STACK_SIZE: usize = 5 * 4096;

/// Pilha do kernel para entradas vindas de ring 3. O processador só troca
/// de pilha sozinho para exceções e interrupções (usando `rsp0` da TSS,
/// abaixo); a instrução `syscall` **não** troca `rsp`, então o stub de
/// entrada de `syscall` (`syscall.rs`) troca para esta mesma pilha à mão.
/// O `rsp` de um programa de usuário nunca é usado nem confiado pelo
/// kernel. Nunca acessada por código Rust diretamente, só pelo
/// processador e pelos trechos de assembly, através do endereço do topo
/// (`kernel_entry_stack_top`).
static mut KERNEL_ENTRY_STACK: Stack<KERNEL_ENTRY_STACK_SIZE> =
    Stack([0; KERNEL_ENTRY_STACK_SIZE]);

/// Endereço do topo (fim, exclusivo) da pilha do kernel para entradas
/// vindas de ring 3. A pilha cresce para baixo, então o endereço usado por
/// `rsp0` e pelo stub de `syscall` é o fim do array, não o início.
pub fn kernel_entry_stack_top() -> VirtAddr {
    // SAFETY: só o endereço de `KERNEL_ENTRY_STACK` é lido aqui (um
    // ponteiro bruto, nunca uma referência Rust viva) para calcular o topo;
    // nenhum código Rust lê ou escreve nela, só o processador e o assembly
    // de `syscall.rs`/`user.rs` a usam depois, através deste endereço.
    let stack_start = VirtAddr::from_ptr(&raw const KERNEL_ENTRY_STACK);
    stack_start + KERNEL_ENTRY_STACK_SIZE as u64
}

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
        // `rsp0`: pilha que o processador carrega sozinho quando uma
        // exceção ou interrupção chega enquanto o programa está em ring 3
        // (mudança de privilégio 3 -> 0).
        tss.privilege_stack_table[0] = kernel_entry_stack_top();
        tss
    };
}

/// Os seletores de segmento que `gdt::init` e `syscall::init` precisam
/// depois de montar a `GlobalDescriptorTable`.
///
/// A ordem dos descritores na GDT é imposta pelo hardware: `sysret` de 64
/// bits carrega `SS = STAR[63:48] + 8` e `CS = STAR[63:48] + 16`, então os
/// dados do usuário precisam vir **antes** do código do usuário; e
/// `syscall` carrega `CS = STAR[47:32]` e `SS = STAR[47:32] + 8`, então os
/// dados do kernel vêm logo **depois** do código do kernel.
struct Selectors {
    kernel_code: SegmentSelector,
    kernel_data: SegmentSelector,
    user_data: SegmentSelector,
    user_code: SegmentSelector,
    tss: SegmentSelector,
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let kernel_code = gdt.append(Descriptor::kernel_code_segment());
        let kernel_data = gdt.append(Descriptor::kernel_data_segment());
        let user_data = gdt.append(Descriptor::user_data_segment());
        let user_code = gdt.append(Descriptor::user_code_segment());
        let tss = gdt.append(Descriptor::tss_segment(&TSS));
        (
            gdt,
            Selectors {
                kernel_code,
                kernel_data,
                user_data,
                user_code,
                tss,
            },
        )
    };
}

/// Seletor do segmento de código do kernel (`0x08`).
pub fn kernel_code_selector() -> SegmentSelector {
    GDT.1.kernel_code
}

/// Seletor do segmento de dados do kernel (`0x10`).
pub fn kernel_data_selector() -> SegmentSelector {
    GDT.1.kernel_data
}

/// Seletor do segmento de dados/pilha do usuário (`0x18`, com RPL 3: `0x1B`).
pub fn user_data_selector() -> SegmentSelector {
    GDT.1.user_data
}

/// Seletor do segmento de código do usuário (`0x20`, com RPL 3: `0x23`).
pub fn user_code_selector() -> SegmentSelector {
    GDT.1.user_code
}

/// Carrega a GDT do kernel (segmentos do kernel e do usuário + descritor
/// da TSS), recarrega os registradores `CS` e `SS` e carrega a TSS. Deve ser chamada uma
/// única vez, durante o boot, antes de `interrupts::init()` — a IDT
/// referencia o índice de pilha que só esta TSS reserva — e antes de
/// qualquer interrupção ser habilitada.
pub fn init() {
    GDT.0.load();

    // SAFETY: os seletores vêm da própria `GDT` que acabou de ser carregada
    // na linha acima, então apontam para descritores válidos e ativos;
    // `SS` recebe o segmento de dados do kernel (antes ficava com o valor
    // herdado do bootloader, um índice da GDT antiga); esta função é
    // chamada uma única vez, antes de qualquer interrupção ser habilitada,
    // então não há disputa com nenhum handler já em execução.
    unsafe {
        CS::set_reg(GDT.1.kernel_code);
        SS::set_reg(GDT.1.kernel_data);
        load_tss(GDT.1.tss);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn o_topo_da_pilha_de_entrada_e_multiplo_de_16() {
        // O stub de `syscall` e a convenção de chamada pressupõem um topo
        // alinhado a 16; sem `#[repr(align(16))]`, o endereço do `static`
        // podia cair em qualquer byte (o que quebrava a espera de tecla numa
        // `SYS_READ_LINE` de verdade).
        assert_eq!(kernel_entry_stack_top().as_u64() % 16, 0);
    }
}
