//! `apagador`: mostra o arquivo que o `gravador` criou e o apaga.
//!
//! Primeiro lê o conteúdo (em pedaços de 128 bytes, como o `leitor`) e o
//! escreve na tela, o que também prova que um programa lê o que outro gravou.
//! Depois remove o arquivo com `remove`.
//!
//! Se o arquivo não existe, o programa avisa e termina com código 1.
//!
//! Rode com `run gravador` antes, e depois `run apagador`.

#![no_std]
#![no_main]

use runtime::sys::write;
use runtime::{entry, println, remove, File, FsError};

entry!(main);

/// O arquivo que o `gravador` cria.
const CAMINHO: &str = "/ram/gravado.txt";

fn main() -> i32 {
    let mut arquivo = match File::open(CAMINHO) {
        Ok(arquivo) => arquivo,
        Err(FsError::NotFound) => {
            println!("apagador: nao ha {} (rode gravador antes)", CAMINHO);
            return 1;
        }
        Err(erro) => {
            println!("apagador: nao abriu {}: {}", CAMINHO, erro);
            return 2;
        }
    };

    let mut pedaco = [0u8; 128];
    loop {
        match arquivo.read(&mut pedaco) {
            // 0 bytes: o arquivo acabou.
            Ok(0) => break,
            Ok(n) => {
                write(&pedaco[..n]);
            }
            Err(erro) => {
                println!("apagador: erro de leitura: {}", erro);
                return 2;
            }
        }
    }

    // O kernel não apaga um arquivo aberto (um leitor poderia passar a ler
    // clusters reaproveitados): `remove` daria `Busy`. Por isso o arquivo é
    // fechado antes, soltando-o de escopo.
    drop(arquivo);

    match remove(CAMINHO) {
        Ok(()) => {
            println!("apagador: removi {}", CAMINHO);
            0
        }
        Err(erro) => {
            println!("apagador: nao removeu {}: {}", CAMINHO, erro);
            3
        }
    }
}
