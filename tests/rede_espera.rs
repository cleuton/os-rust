//! O único teste do Marco 12 que gasta 30 s de relógio de verdade: o `servidor`
//! sozinho, sem ninguém se conectar, desiste depois do tempo limite. Fica num
//! binário à parte para caber folgado nos 60 s do `test-timeout`.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os_rust::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::string::String;
use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use os_rust::user::{self, Termination};
use os_rust::{memory, net, timer, vga_buffer};

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

fn tela_contem(texto: &str) -> bool {
    (0..25).any(|row| {
        let linha: String = vga_buffer::screen_row_bytes(row).iter().map(|&b| b as char).collect();
        linha.contains(texto)
    })
}

#[test_case]
fn servidor_sozinho_desiste_depois_de_30_segundos() {
    vga_buffer::clear_screen();
    let frames = memory::frames_outstanding();
    let antes = timer::ticks();
    let relatorio = user::run_all(&["servidor"], None).expect("run servidor");
    let ticks = timer::ticks() - antes;
    let fim = &relatorio.finished[0];
    assert_eq!(fim.termination, Termination::Exit { code: 1 }, "{:?}", relatorio.finished);
    assert!(tela_contem("servidor: ouvindo na porta 8000"));
    assert!(tela_contem("servidor: ninguem se conectou em 30 s"));
    // 30 s = 3000 ticks, com uma pequena folga para cima.
    assert!((3000..3100).contains(&ticks), "demorou {} ticks", ticks);
    // O kernel dormiu, em vez de girar: poucas voltas ociosas por tick.
    assert!(relatorio.idle_loops as u64 <= ticks + 5, "{} voltas", relatorio.idle_loops);
    x86_64::instructions::interrupts::disable();
    net::poll();
    assert_eq!(net::tcp_in_use(), 0);
    x86_64::instructions::interrupts::enable();
    assert_eq!(memory::frames_outstanding(), frames);
}
