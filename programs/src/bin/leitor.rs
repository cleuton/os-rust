//! `leitor`: abre um arquivo do disco, lê o conteúdo e o escreve na tela.
//!
//! O programa nunca fala com o disco: ele pede ao kernel, por syscalls, e a
//! biblioteca de runtime esconde as syscalls atrás de `File`. O arquivo é
//! lido em pedaços de 128 bytes (um buffer na pilha) até `read` devolver `0`,
//! que quer dizer "o arquivo acabou". O caminho é fixo porque programas não
//! recebem argumentos de linha de comando neste marco.
//!
//! Rode com `run leitor`. Para ler outro arquivo, troque `CAMINHO`.

#![no_std]
#![no_main]

use runtime::sys::write;
use runtime::{entry, println, File};

entry!(main);

/// O arquivo que o programa lê: um texto de dois clusters, no disco.
const CAMINHO: &str = "/disco/docs/longo.txt";

fn main() -> i32 {
    // `File::open` devolve um erro de Rust se o arquivo não existe, se o disco
    // não está lá, etc. O arquivo é fechado sozinho quando `arquivo` sai de escopo.
    let mut arquivo = match File::open(CAMINHO) {
        Ok(arquivo) => arquivo,
        Err(erro) => {
            println!("leitor: nao abriu {}: {}", CAMINHO, erro);
            return 1;
        }
    };

    let mut pedaco = [0u8; 128];
    loop {
        match arquivo.read(&mut pedaco) {
            // 0 bytes: o arquivo acabou.
            Ok(0) => return 0,
            // `write` entrega os bytes à tela exatamente como estão.
            Ok(n) => {
                write(&pedaco[..n]);
            }
            Err(erro) => {
                println!("leitor: erro de leitura: {}", erro);
                return 2;
            }
        }
    }
}
