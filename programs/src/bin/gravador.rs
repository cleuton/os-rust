//! `gravador`: cria um arquivo no ramdisk e, na vez seguinte, o amplia.
//!
//! Um programa nunca fala com o disco: ele pede ao kernel, por syscalls, e a
//! biblioteca de runtime esconde as syscalls atrás de `FileWriter`, que fecha o
//! descritor sozinho quando sai de escopo. Escrever é sempre **pelo fim**: um
//! arquivo nasce vazio e só cresce acrescentando bytes. Por isso o programa
//! tenta criar primeiro; se o nome já existe (`Exists`), abre o arquivo para
//! acrescentar em vez de sobrescrevê-lo.
//!
//! Só o `/ram` aceita escrita (o `/disco` é somente leitura), e reiniciar o
//! sistema devolve o `/ram` ao conteúdo original. O caminho é fixo porque
//! programas não recebem argumentos de linha de comando.
//!
//! Rode com `run gravador` (cria), de novo (amplia) e `run apagador` (remove).

#![no_std]
#![no_main]

use runtime::{entry, println, FileWriter, FsError};

entry!(main);

/// O arquivo que o programa cria e amplia.
const CAMINHO: &str = "/ram/gravado.txt";

fn main() -> i32 {
    match FileWriter::create(CAMINHO) {
        Ok(mut arquivo) => {
            if let Err(erro) = grava_linhas(&mut arquivo) {
                println!("gravador: nao gravou {}: {}", CAMINHO, erro);
                return 1;
            }
            println!("gravador: criei {} (680 bytes)", CAMINHO);
            0
        }
        // O nome já existe: acrescenta ao fim, sem tocar no que já está lá.
        Err(FsError::Exists) => {
            let resultado = FileWriter::append(CAMINHO)
                .and_then(|mut arquivo| arquivo.write_all(b"mais uma linha do gravador\n"));
            match resultado {
                Ok(()) => {
                    println!("gravador: ampliei {} (+27 bytes)", CAMINHO);
                    0
                }
                Err(erro) => {
                    println!("gravador: nao gravou {}: {}", CAMINHO, erro);
                    1
                }
            }
        }
        Err(erro) => {
            println!("gravador: nao gravou {}: {}", CAMINHO, erro);
            1
        }
    }
}

/// Grava as 20 linhas `linha NN: gravada por um programa`, cada uma com 34
/// bytes (33 de texto e a quebra de linha), 680 no total. A linha é montada
/// num buffer da pilha: o programa não precisa de heap.
fn grava_linhas(arquivo: &mut FileWriter) -> Result<(), FsError> {
    for numero in 1..=20u8 {
        let mut linha = *b"linha NN: gravada por um programa\n";
        linha[6] = b'0' + numero / 10;
        linha[7] = b'0' + numero % 10;
        arquivo.write_all(&linha)?;
    }
    Ok(())
}
