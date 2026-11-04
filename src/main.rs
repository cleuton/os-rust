#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

mod interrupts;
mod keyboard;
mod panic;
mod shell;
mod vga_buffer;

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;

entry_point!(kernel_main);

fn kernel_main(_boot_info: &'static BootInfo) -> ! {
    vga_buffer::clear_screen();
    println!("proto-os - sem sistema operacional embaixo");
    println!("Este texto foi escrito direto no buffer de video VGA,");
    println!("por este mesmo binario Rust, sem nenhum SO por baixo.");

    interrupts::init();
    shell::print_prompt();

    loop {
        x86_64::instructions::interrupts::without_interrupts(|| {
            while let Some(scancode) = interrupts::next_scancode() {
                if let Some(byte) = keyboard::translate(scancode) {
                    shell::feed(byte);
                }
            }
        });

        // `enable_and_hlt` executa `sti; hlt` como uma única instrução
        // atômica: habilita interrupções e pausa a CPU até a próxima, sem
        // a janela de corrida em que uma tecla pressionada entre habilitar
        // e pausar ficaria "perdida" até a tecla seguinte.
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

#[panic_handler]
fn on_panic(info: &PanicInfo) -> ! {
    panic::handle(info)
}
