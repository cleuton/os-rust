//! Teste de integração cujo resultado esperado é um panic.
//!
//! Diferente dos demais binários de teste, este NÃO usa
//! `custom_test_frameworks`: o alvo usa `panic-strategy = "abort"` (sem
//! *unwinding*), então `#[should_panic]` do harness padrão não
//! funcionaria aqui de qualquer forma, já que ele depende de capturar o
//! panic via *unwinding* e continuar a execução dos testes seguintes. Em
//! vez disso, o panic é tratado diretamente como sucesso pelo
//! `#[panic_handler]` deste arquivo.

#![no_std]
#![no_main]

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use os_rust::{exit_qemu, serial_println, QemuExitCode};

entry_point!(main);

fn main(boot_info: &'static BootInfo) -> ! {
    os_rust::init(boot_info);

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
