//! Teste de integração de boot: inicia o kernel do zero (próprio
//! binário, próprio `entry_point!`) e confirma que ele chega até o
//! prompt ficar pronto, sem entrar em panic (FR-017).

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(proto_os::test_runner)]
#![reexport_test_harness_main = "test_main"]

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;

entry_point!(main);

fn main(_boot_info: &'static BootInfo) -> ! {
    test_main();
    proto_os::panic::halt_loop();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    proto_os::test_panic_handler(info)
}

#[test_case]
fn kernel_inicializa_ate_o_prompt_ficar_pronto() {
    proto_os::init();
    proto_os::shell::print_prompt();
}
