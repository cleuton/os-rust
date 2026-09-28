//! `falha_memoria`: um programa que erra de propósito.
//!
//! Escreve num endereço de memória que não é dele. O processador levanta
//! uma exceção de falha de página (`#PF`); o kernel a trata, encerra só
//! este programa, mostra uma mensagem legível e devolve o controle ao
//! prompt. Uma falha em programa de usuário nunca derruba o kernel
//! (`SYSCALLS.md`, seção 7).

#![no_std]
#![no_main]

use runtime::{entry, println};

entry!(main);

fn main() -> i32 {
    println!("prestes a acessar memoria invalida...");

    // SAFETY: isto é **intencionalmente** inseguro. `0xdead_beef` está fora
    // da memória deste programa, então a escrita vira uma exceção de CPU
    // (`#PF`), tratada pelo kernel: é exatamente o comportamento que este
    // programa existe para mostrar.
    unsafe {
        core::ptr::write_volatile(0xdead_beef as *mut u8, 42);
    }

    // Esta linha nunca é alcançada.
    println!("se voce esta vendo isso, o isolamento falhou");
    0
}
