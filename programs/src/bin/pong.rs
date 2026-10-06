//! `pong`: o segundo de dois programas que se alternam na tela.
//!
//! É igual ao `ping`, com outro texto. Rode `run ping pong` e as linhas dos
//! dois aparecem intercaladas, na ordem: cada um escreve uma linha e **cede a
//! CPU** (`yield_now`) ao outro. Cada programa tem a memória só dele: o
//! contador `i` de um não é visto pelo outro.

#![no_std]
#![no_main]

use runtime::{entry, println, yield_now};

entry!(main);

fn main() -> i32 {
    for i in 1..=4 {
        // Pede ao kernel (syscall `write`) que escreva a linha na tela. A
        // linha chega inteira, mesmo com outro programa rodando.
        println!("pong {}", i);

        // Pede ao kernel (syscall `yield`) que passe a CPU ao próximo
        // programa pronto. Só volta aqui quando chegar a vez deste de novo.
        yield_now();
    }
    0
}
