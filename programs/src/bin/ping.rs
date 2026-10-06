//! `ping`: o primeiro de dois programas que se alternam na tela.
//!
//! Escreve uma linha e **cede a CPU** (`yield_now`) ao outro programa; quando
//! chega a vez dele de novo, escreve a próxima. Rode `run ping pong` e as
//! linhas de `ping` e de `pong` aparecem intercaladas, na ordem. Cada
//! programa tem a memória só dele: o contador `i` de um não é visto pelo outro.

#![no_std]
#![no_main]

use runtime::{entry, println, yield_now};

entry!(main);

fn main() -> i32 {
    for i in 1..=4 {
        // Pede ao kernel (syscall `write`) que escreva a linha na tela. A
        // linha chega inteira, mesmo com outro programa rodando.
        println!("ping {}", i);

        // Pede ao kernel (syscall `yield`) que passe a CPU ao próximo
        // programa pronto. Só volta aqui quando chegar a vez deste de novo.
        yield_now();
    }
    0
}
