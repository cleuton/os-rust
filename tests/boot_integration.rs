//! Teste de integração de boot: inicia o kernel do zero (próprio
//! binário, próprio `entry_point!`) e confirma que ele chega até o
//! prompt ficar pronto, sem entrar em panic.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os_rust::test_runner)]
#![reexport_test_harness_main = "test_main"]

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;

entry_point!(main);

fn main(boot_info: &'static BootInfo) -> ! {
    os_rust::init(boot_info);
    test_main();
    os_rust::panic::halt_loop();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    os_rust::test_panic_handler(info)
}

#[test_case]
fn kernel_inicializa_ate_o_prompt_ficar_pronto() {
    os_rust::shell::print_prompt();
}

#[test_case]
fn boas_vindas_mostra_a_versao() {
    os_rust::vga_buffer::clear_screen();
    os_rust::print_welcome();
    assert!(os_rust::vga_buffer::screen_contains(os_rust::VERSION));
}

#[test_case]
fn identificacao_bate_com_o_manifesto() {
    // Montada aqui de novo, independentemente, a partir de `env!` — nunca
    // um literal `"os-rust"`/`"0.4.1"` copiado à mão, que ficaria
    // desatualizado no próximo bump de versão sem que este teste notasse.
    assert_eq!(
        os_rust::VERSION,
        concat!(env!("CARGO_PKG_NAME"), " v", env!("CARGO_PKG_VERSION"))
    );
}

/// Traduz uma linha do texto canônico do logo para os bytes esperados na
/// tela: `'█'` -> `0xDB` (bloco cheio da code page 437), ASCII imprimível
/// -> o próprio byte, resto -> `0xfe` — a mesma tradução que
/// `vga_buffer::draw_logo` aplica. Colunas além do fim da linha ficam com
/// espaço, como a tela já está depois de `clear_screen()`.
fn linha_do_logo_traduzida(linha: &str) -> [u8; 80] {
    let mut esperado = [b' '; 80];
    for (col, ch) in linha.chars().enumerate() {
        esperado[col] = match ch {
            '█' => 0xDB,
            ' '..='~' => ch as u8,
            _ => 0xfe,
        };
    }
    esperado
}

/// Confere que as 20 primeiras linhas da tela batem, char a char, com
/// `os_rust::logo::LOGO` traduzido.
fn assert_logo_intacto() {
    for (row, linha) in os_rust::logo::LOGO.lines().enumerate() {
        assert_eq!(
            os_rust::vga_buffer::screen_row_bytes(row),
            linha_do_logo_traduzida(linha),
            "linha {} do logo nao bate",
            row
        );
    }
}

#[test_case]
fn logo_aparece_intacto_nas_20_primeiras_linhas() {
    // Isola `draw_logo()` do resto da sequência de boot, para provar só
    // a tradução de caracteres, sem depender de mais nada ter escrito na
    // tela antes.
    os_rust::vga_buffer::clear_screen();
    os_rust::vga_buffer::draw_logo();
    assert_logo_intacto();
}

#[test_case]
fn logo_sobrevive_a_sequencia_completa_de_boot_sem_rolar() {
    // Repete a ordem exata de `main.rs::kernel_main` e confere de novo
    // que o logo continua intacto depois que a identificação e o prompt
    // já escreveram na tela — prova automaticamente que nenhuma linha do
    // logo rola para fora quando eles aparecem, em vez de depender só de
    // inspeção visual no QEMU.
    os_rust::vga_buffer::clear_screen();
    os_rust::print_welcome();
    os_rust::shell::print_prompt();
    os_rust::vga_buffer::draw_logo();
    assert_logo_intacto();
}
