//! `eco`: lê uma linha do teclado e responde.
//!
//! Mostra o ciclo completo de um programa de usuário: escrever (`print!`),
//! ler do teclado (`read_line`) e usar memória dinâmica (`Vec`), tudo por
//! syscalls do kernel, através da biblioteca de runtime. O programa nunca
//! toca no hardware do teclado nem na tela: só pede ao kernel.

#![no_std]
#![no_main]

// O alocador global vem da biblioteca de runtime; `extern crate alloc`
// dá acesso a `Vec`, `Box` e `String`.
extern crate alloc;

use alloc::vec::Vec;
use runtime::{entry, print, println, read_line};

entry!(main);

fn main() -> i32 {
    print!("digite algo: ");

    // O buffer é do programa. `read_line` escreve nele a linha digitada e
    // devolve o texto, sem o Enter. Cabem até 127 caracteres.
    let mut buffer = [0u8; 128];
    let texto = read_line(&mut buffer);

    // A resposta: exatamente o que foi digitado.
    println!("voce digitou: {}", texto);

    // `collect` cria um `Vec`: a primeira alocação faz a biblioteca pedir
    // memória ao kernel (syscall `SYS_ALLOC`).
    let palavras: Vec<&str> = texto.split_whitespace().collect();
    println!("palavras: {}", palavras.len());

    0
}
