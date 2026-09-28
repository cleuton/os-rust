//! Teste de integração de page fault.
//!
//! Como `should_panic.rs`, este teste NÃO usa `custom_test_frameworks`:
//! o cenário é um único evento dirigido (provocar um `#PF` esperado),
//! não uma suíte de `#[test_case]`s independentes — um page fault não
//! tratado pelo handler de teste levaria à tela de exceção de produção,
//! que nunca retorna, então não haveria como continuar rodando outros
//! testes na mesma suíte. `os_rust::init(boot_info)` já carrega a IDT de
//! produção;
//! este arquivo sobrepõe só a entrada `page_fault` com um handler de
//! teste próprio, que confere o endereço de falha (CR2) contra o valor
//! esperado antes de sinalizar o resultado ao host — a tela de exceção
//! de produção (`interrupts::fatal_exception`) nunca retorna, então não
//! serviria para reportar sucesso.

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use lazy_static::lazy_static;
use os_rust::{allocator, exit_qemu, serial_println, QemuExitCode};
use x86_64::registers::control::Cr2;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

/// Endereço logo após a última página mapeada do heap — nunca mapeado
/// (mesma fórmula usada pelo comando `falha pagina` do shell).
fn endereco_esperado() -> u64 {
    allocator::HEAP_START as u64 + allocator::HEAP_SIZE as u64
}

lazy_static! {
    static ref TEST_IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.page_fault.set_handler_fn(test_page_fault_handler);
        idt
    };
}

extern "x86-interrupt" fn test_page_fault_handler(
    _stack_frame: InterruptStackFrame,
    _error_code: PageFaultErrorCode,
) {
    let fault_address = Cr2::read_raw();
    if fault_address == endereco_esperado() {
        exit_qemu(QemuExitCode::Success);
    } else {
        serial_println!(
            "endereco de falha inesperado: esperava {:#x}, recebeu {:#x}",
            endereco_esperado(),
            fault_address
        );
        exit_qemu(QemuExitCode::Failed);
    }
}

entry_point!(main);

fn main(boot_info: &'static BootInfo) -> ! {
    os_rust::init(boot_info);

    // A partir daqui, page fault é tratado pelo handler de teste acima,
    // por cima da IDT de produção.
    TEST_IDT.load();

    // SAFETY: este endereço fica deliberadamente logo após a última
    // página mapeada do heap (nunca mapeado — Marco 3); o page fault
    // resultante é o comportamento esperado e intencional deste teste.
    unsafe {
        (endereco_esperado() as *const u8).read_volatile();
    }

    // O handler de teste sempre chama exit_qemu antes de este ponto ser
    // alcançado; se chegamos até aqui, o page fault esperado não
    // aconteceu.
    serial_println!("[test did not fault]");
    exit_qemu(QemuExitCode::Failed);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    os_rust::test_panic_handler(info)
}
