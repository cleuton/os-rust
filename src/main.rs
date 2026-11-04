#![no_std]
#![no_main]

mod panic;
mod vga_buffer;

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;

entry_point!(kernel_main);

fn kernel_main(_boot_info: &'static BootInfo) -> ! {
    vga_buffer::clear_screen();
    println!("proto-os - sem sistema operacional embaixo");
    println!("Este texto foi escrito direto no buffer de video VGA,");
    println!("por este mesmo binario Rust, sem nenhum SO por baixo.");

    loop {
        // SAFETY: `hlt` apenas pausa a CPU até a próxima interrupção; não
        // acessa memória nem modifica a pilha, então é seguro chamá-la em
        // loop para manter o sistema "rodando" sem gastar CPU à toa depois
        // que a mensagem de boas-vindas já foi escrita na tela.
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

#[panic_handler]
fn on_panic(info: &PanicInfo) -> ! {
    panic::handle(info)
}
