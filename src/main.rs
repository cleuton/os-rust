#![no_std]
#![no_main]
#![cfg_attr(test, feature(custom_test_frameworks))]
#![cfg_attr(test, test_runner(os_rust::test_runner))]
#![cfg_attr(test, reexport_test_harness_main = "test_main")]

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
#[cfg(not(test))]
use os_rust::{panic, serial_println, shell, vga_buffer};

#[cfg(not(test))]
entry_point!(kernel_main);

#[cfg(not(test))]
fn kernel_main(boot_info: &'static BootInfo) -> ! {
    os_rust::init(boot_info);

    vga_buffer::clear_screen();
    os_rust::print_welcome();

    shell::print_prompt();

    // Desenhado por último, direto nas linhas 0-19 (sem passar pelo
    // `Writer` normal). O `Writer` sempre escreve na última linha visível
    // e rola a tela inteira a cada `\n`; se o logo fosse escrito primeiro
    // pelo caminho normal, cada linha nova de `print_welcome`/
    // `shell::print_prompt` rolaria o logo já escrito para fora da tela.
    // Escrevendo por último e direto no buffer, sem passar pelo `Writer`,
    // nenhuma rolagem seguinte pode deslocá-lo.
    vga_buffer::draw_logo();

    serial_println!("[boot] prompt pronto");

    loop {
        shell::poll_keyboard();

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
// modo de teste. `cargo test` compila e roda em QEMU um binário por
// arquivo de teste, mais a própria `lib.rs` e este `main.rs` — mesmo sem
// nenhum #[test_case] próprio, rodar este binário sob teste prova que o
// binário de produção ainda compila e dá boot também em modo de teste;
// por isso ele só inicializa o kernel e roda a suíte (vazia, aqui)
// gerada pelo framework de testes customizado.
#[cfg(test)]
entry_point!(test_kernel_main);

#[cfg(test)]
fn test_kernel_main(boot_info: &'static BootInfo) -> ! {
    os_rust::init(boot_info);
    test_main();
    os_rust::panic::halt_loop();
}

#[cfg(test)]
#[panic_handler]
fn test_on_panic(info: &PanicInfo) -> ! {
    os_rust::test_panic_handler(info)
}
