//! `listador`: lista um diretório do disco e escreve o nome de cada entrada.
//!
//! Um diretório é aberto como um arquivo, com `Dir::open`, e lido uma entrada
//! por vez com `next`, até ela devolver `None`. Cada entrada diz o nome, se é
//! um diretório e o tamanho. O caminho é fixo porque programas não recebem
//! argumentos de linha de comando neste marco.
//!
//! Rode com `run listador`. Para listar outro diretório, troque `CAMINHO`.

#![no_std]
#![no_main]

use runtime::{entry, println, Dir};

entry!(main);

/// O diretório que o programa lista: a raiz do disco.
const CAMINHO: &str = "/disco";

fn main() -> i32 {
    let mut diretorio = match Dir::open(CAMINHO) {
        Ok(diretorio) => diretorio,
        Err(erro) => {
            println!("listador: nao abriu {}: {}", CAMINHO, erro);
            return 1;
        }
    };

    loop {
        match diretorio.next() {
            Ok(Some(entrada)) => {
                let tipo = if entrada.is_dir() { "dir" } else { "arquivo" };
                println!("{} {} {}", tipo, entrada.size(), entrada.name());
            }
            // Sem mais entradas.
            Ok(None) => return 0,
            Err(erro) => {
                println!("listador: erro de leitura: {}", erro);
                return 2;
            }
        }
    }
}
