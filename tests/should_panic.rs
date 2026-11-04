//! Teste de integração cujo resultado esperado é um panic (FR-016).
//!
//! Diferente dos demais binários de teste, este NÃO usa
//! `custom_test_frameworks`: o alvo usa `panic-strategy = "abort"` (sem
//! *unwinding*), então `#[should_panic]` do harness padrão não
//! funcionaria aqui de qualquer forma — ver `research.md`, seção 6. Em
//! vez disso, o panic é tratado diretamente como sucesso pelo
//! `#[panic_handler]` deste arquivo.

#![no_std]
#![no_main]

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use proto_os::{exit_qemu, serial_println, QemuExitCode};

entry_point!(main);

fn main(_boot_info: &'static BootInfo) -> ! {
    proto_os::init();

    serial_println!("should_panic::verificacao_que_deve_falhar...\t");
    verificacao_que_deve_falhar();

    // Se chegamos até aqui, a verificação abaixo não entrou em panic
    // como esperado — o teste falhou.
    serial_println!("[test did not panic]");
    exit_qemu(QemuExitCode::Failed);
}

/// A verificação sob teste: deliberadamente falsa por construção, para
/// que `assert_eq!` dispare um panic.
fn verificacao_que_deve_falhar() {
    assert_eq!(1, 2, "verificacao deliberadamente falsa");
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // O panic era exatamente o resultado esperado: sucesso.
    serial_println!("[ok]");
    exit_qemu(QemuExitCode::Success);
}
