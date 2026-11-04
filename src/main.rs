#![no_std]
#![no_main]
#![cfg_attr(test, feature(custom_test_frameworks))]
#![cfg_attr(test, test_runner(proto_os::test_runner))]
#![cfg_attr(test, reexport_test_harness_main = "test_main")]

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
#[cfg(not(test))]
use proto_os::{interrupts, keyboard, panic, println, serial_println, shell, vga_buffer};

#[cfg(not(test))]
entry_point!(kernel_main);

#[cfg(not(test))]
fn kernel_main(_boot_info: &'static BootInfo) -> ! {
    proto_os::init();

    vga_buffer::clear_screen();
    println!("proto-os - sem sistema operacional embaixo");
    println!("Este texto foi escrito direto no buffer de video VGA,");
    println!("por este mesmo binario Rust, sem nenhum SO por baixo.");

    shell::print_prompt();
    serial_println!("[boot] prompt pronto");

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

#[cfg(not(test))]
#[panic_handler]
fn on_panic(info: &PanicInfo) -> ! {
    panic::handle(info)
}

// Segundo ponto de entrada, usado só quando este binário é compilado em
// modo de teste (`cargo test` também compila e roda `main.rs`, ainda que
// sem nenhum #[test_case] próprio — ver `research.md`, seção 8): apenas
// inicializa o kernel e roda a suíte (vazia, neste binário) gerada pelo
// framework de testes customizado.
#[cfg(test)]
entry_point!(test_kernel_main);

#[cfg(test)]
fn test_kernel_main(_boot_info: &'static BootInfo) -> ! {
    proto_os::init();
    test_main();
    proto_os::panic::halt_loop();
}

#[cfg(test)]
#[panic_handler]
fn test_on_panic(info: &PanicInfo) -> ! {
    proto_os::test_panic_handler(info)
}
