//! `visita`: um programa que existe **só no disco**.
//!
//! Os outros programas do os-rust viajam dentro do kernel: o `build.rs` os
//! compila e os embute na imagem de boot. Este não. Ele fica fora de
//! `programs/src/bin/`, então o kernel nunca o viu em tempo de compilação: o
//! `build.rs` o compila, mas só o grava dentro da imagem do disco
//! (`/disco/bin/visita`). Rode `run /disco/bin/visita` e o kernel lê o
//! arquivo do disco, valida o ELF, carrega e executa.
//!
//! Por dentro é um programa comum: escreve uma linha com `println!` e sai.

#![no_std]
#![no_main]

use runtime::{entry, println};

entry!(main);

fn main() -> i32 {
    println!("visita: fui carregado do disco!");
    0
}
