//! `eco2`: o irmão do `eco`, para rodar ao lado dele.
//!
//! Faz o mesmo que o `eco` (lê uma linha do teclado e responde), mas põe o
//! prefixo `eco2:` na resposta. Rode `run eco eco2`, digite uma linha e Enter, e
//! depois outra: o teclado vai ao programa que **pediu primeiro** (o `eco`), e a
//! segunda linha vai ao outro. Cada resposta mostra quem recebeu qual linha.
//! Enquanto um programa espera o teclado ele não gasta CPU: o kernel passa a
//! vez ao outro.

#![no_std]
#![no_main]

// O alocador global vem da biblioteca de runtime; `extern crate alloc`
// dá acesso a `Vec`, `Box` e `String`.
extern crate alloc;

use alloc::vec::Vec;
use runtime::{entry, print, println, read_line};

entry!(main);

fn main() -> i32 {
    print!("eco2: digite algo: ");

    // O buffer é do programa. `read_line` escreve nele a linha digitada e
    // devolve o texto, sem o Enter. Cabem até 127 caracteres.
    let mut buffer = [0u8; 128];
    let texto = read_line(&mut buffer);

    // A resposta: exatamente o que foi digitado, com o prefixo do programa.
    println!("eco2: voce digitou: {}", texto);

    // `collect` cria um `Vec`: a primeira alocação faz a biblioteca pedir
    // memória ao kernel (syscall `SYS_ALLOC`).
    let palavras: Vec<&str> = texto.split_whitespace().collect();
    println!("eco2: palavras: {}", palavras.len());

    0
}
