//! `contador_a`: conta em laço longo, **sem nunca ceder a CPU**.
//!
//! Este programa não pede a vez a ninguém: ele só conta, como se fosse o único
//! programa do sistema. Mesmo assim, rodando ao lado do `contador_b`
//! (`run contador_a contador_b`), as linhas dos dois aparecem intercaladas na
//! tela, porque o timer do kernel interrompe cada programa depois de uma
//! fatia de tempo e passa a CPU ao outro. Quem manda na CPU é o kernel, não o
//! programa.

#![no_std]
#![no_main]

use core::hint::black_box;
use runtime::{entry, println};

entry!(main);

/// Quantas iterações o programa conta entre uma linha e a seguinte. É grande
/// de propósito: leva várias fatias de tempo do timer, então as linhas dos
/// dois programas só se alternam se o kernel os interromper.
const PASSO: u64 = 20_000_000;

/// Quantas linhas o programa escreve. São poucas: a tela tem 25 linhas e os
/// testes leem a tela, então a saída dos dois programas tem de caber nela.
const LINHAS: u64 = 8;

fn main() -> i32 {
    let mut contador: u64 = 0;
    for linha in 1..=LINHAS {
        for _ in 0..PASSO {
            // `black_box` esconde o valor do compilador: sem ele, o laço seria
            // eliminado e o programa terminaria na hora.
            contador = black_box(contador).wrapping_add(1);
        }
        // Pede ao kernel (syscall `write`) que escreva a linha na tela.
        println!("A: {}", linha);
    }
    0
}
