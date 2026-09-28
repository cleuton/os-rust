//! Teste de integração de double fault.
//!
//! Como `page_fault.rs`, este teste NÃO usa `custom_test_frameworks`: é
//! um único evento dirigido (estourar a pilha do kernel e provocar um
//! `#DF`), não uma suíte de `#[test_case]`s — o próprio double fault, se
//! não for tratado na pilha certa, derruba o processador antes que
//! qualquer suíte normal pudesse continuar rodando outros testes.
//! `os_rust::init(boot_info)` já carrega a GDT/TSS/IST e a IDT de
//! produção; este arquivo sobrepõe só a entrada `double_fault` com um
//! handler de teste que sinaliza sucesso ao host assim que é alcançado
//! — a prova de sucesso é justamente ele ter sido alcançado (rodando na
//! pilha dedicada da IST) em vez de um triple fault reiniciando o QEMU.

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use lazy_static::lazy_static;
use os_rust::{exit_qemu, gdt, serial_println, QemuExitCode};
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

lazy_static! {
    static ref TEST_IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        // SAFETY: o índice aponta para a mesma pilha dedicada que
        // `gdt::init()` (já chamado por `os_rust::init`, acima) reserva
        // na TSS para double fault — é exatamente essa pilha que este
        // teste quer provar que está em uso.
        unsafe {
            idt.double_fault
                .set_handler_fn(test_double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt
    };
}

extern "x86-interrupt" fn test_double_fault_handler(
    _stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    // Ter chegado até aqui já é o resultado esperado: o processador só
    // consegue entregar este handler, numa pilha válida, porque a IST
    // trocou para a pilha dedicada antes de entrar nele.
    exit_qemu(QemuExitCode::Success);
}

entry_point!(main);

fn main(boot_info: &'static BootInfo) -> ! {
    os_rust::init(boot_info);

    // A partir daqui, double fault é tratado pelo handler de teste
    // acima, por cima da IDT de produção — mas na mesma pilha dedicada
    // da TSS/IST que a produção já configurou.
    TEST_IDT.load();

    stack_overflow();

    // O handler de teste sempre chama exit_qemu antes de este ponto ser
    // alcançado; se chegamos até aqui, o estouro de pilha não provocou
    // o double fault esperado.
    serial_println!("[test did not fault]");
    exit_qemu(QemuExitCode::Failed);
}

/// Recursão sem caso base, deliberada: cada chamada empilha um novo
/// quadro, até estourar a pilha do kernel. A leitura volátil depois da
/// chamada recursiva impede o compilador de aplicar otimização de *tail
/// call* (que transformaria a recursão num laço sem crescer a pilha, e
/// este teste nunca estouraria nada) — mesma técnica de
/// `shell::cmd_falha_pilha`.
#[allow(unconditional_recursion)]
fn stack_overflow() {
    stack_overflow();
    volatile::Volatile::new(0u8).read();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    os_rust::test_panic_handler(info)
}
