//! `hora`: escreve a data e a hora atuais (UTC).
//!
//! O programa não toca no relógio: pede a hora ao kernel (syscall `time`, pela
//! biblioteca de runtime) e escreve o que recebeu, no mesmo formato do comando
//! `data` do prompt. Rode `run hora` e compare com `data`.

#![no_std]
#![no_main]

use runtime::{entry, println, time};

entry!(main);

fn main() -> i32 {
    // Pede ao kernel (syscall `time`) a data e a hora. Só falha se o relógio
    // do computador devolver valores impossíveis.
    match time::now() {
        Ok(agora) => {
            // `{}` escreve `AAAA-MM-DD HH:MM:SS`; o relógio é sempre UTC.
            println!("{} UTC", agora);
            0
        }
        Err(erro) => {
            // Um código de saída diferente de zero faz o kernel avisar no
            // prompt (`[run] hora terminou com codigo 1`).
            println!("hora: {}", erro);
            1
        }
    }
}
