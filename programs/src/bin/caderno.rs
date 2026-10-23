//! `caderno`: acrescenta uma linha com a hora a um arquivo do disco e mostra o
//! arquivo inteiro.
//!
//! Cada vez que roda, o programa anota a hora atual (UTC) em
//! `/disco/caderno.txt`: na primeira vez cria o arquivo, nas seguintes
//! acrescenta ao fim. Depois abre o arquivo, escreve o conteúdo na tela e
//! conta as linhas. É o jeito mais curto de ver a persistência do `/disco`:
//! rode `run caderno` duas vezes e o arquivo terá duas linhas (20 bytes cada).
//!
//! Atenção: enquanto o QEMU roda com `snapshot=on` (é o caso de `cargo run` e
//! `cargo test`), o `/disco` volta ao inicial quando o QEMU fecha. Para a
//! anotação sobreviver a um reinício, rode o QEMU à mão sobre uma cópia da
//! imagem do disco (passo a passo no README, "Preservando o disco entre boots").
//!
//! O escritor é fechado **antes** de o arquivo ser aberto para leitura, e o
//! caminho é fixo porque programas não recebem argumentos de linha de comando.
//!
//! Rode com `run caderno`.

#![no_std]
#![no_main]

use core::fmt::{self, Write};

use runtime::sys::write;
use runtime::{entry, println, time, File, FileWriter, FsError, TimeError};

entry!(main);

/// O arquivo que o programa cria e amplia.
const CAMINHO: &str = "/disco/caderno.txt";

/// Uma linha do caderno: `AAAA-MM-DD HH:MM:SS` (19 caracteres) mais `\n`.
const TAMANHO_DA_LINHA: usize = 20;

/// Por que o programa não conseguiu anotar ou mostrar o caderno.
enum Falha {
    Arquivo(FsError),
    Hora(TimeError),
    /// A linha não coube nos 20 bytes (não deveria acontecer).
    Formato,
}

impl From<FsError> for Falha {
    fn from(erro: FsError) -> Falha {
        Falha::Arquivo(erro)
    }
}

impl From<TimeError> for Falha {
    fn from(erro: TimeError) -> Falha {
        Falha::Hora(erro)
    }
}

impl fmt::Display for Falha {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Falha::Arquivo(erro) => write!(f, "{}", erro),
            Falha::Hora(erro) => write!(f, "{}", erro),
            Falha::Formato => f.write_str("linha com formato inesperado"),
        }
    }
}

/// Um buffer na pilha que recebe a linha formatada: o programa não precisa de heap.
struct Linha {
    bytes: [u8; TAMANHO_DA_LINHA],
    usados: usize,
}

impl Write for Linha {
    fn write_str(&mut self, texto: &str) -> fmt::Result {
        let fim = self.usados + texto.len();
        if fim > TAMANHO_DA_LINHA {
            return Err(fmt::Error);
        }
        self.bytes[self.usados..fim].copy_from_slice(texto.as_bytes());
        self.usados = fim;
        Ok(())
    }
}

fn main() -> i32 {
    match anota().and_then(|_| mostra()) {
        Ok(linhas) => {
            // "1 linha", mas "2 linhas".
            let palavra = if linhas == 1 { "linha" } else { "linhas" };
            println!("caderno: {} {} em {}", linhas, palavra, CAMINHO);
            0
        }
        Err(erro) => {
            println!("caderno: nao gravou {}: {}", CAMINHO, erro);
            1
        }
    }
}

/// Acrescenta a linha com a hora ao caderno, criando o arquivo se ele não existe.
fn anota() -> Result<(), Falha> {
    // O nome já existe (`Exists`): acrescenta ao fim, sem tocar no que já está lá.
    let mut escritor = match FileWriter::create(CAMINHO) {
        Ok(escritor) => escritor,
        Err(FsError::Exists) => FileWriter::append(CAMINHO)?,
        Err(erro) => return Err(erro.into()),
    };
    let agora = time::now()?;
    let mut linha = Linha { bytes: [0; TAMANHO_DA_LINHA], usados: 0 };
    // `{}` escreve `AAAA-MM-DD HH:MM:SS`; o relógio é sempre UTC.
    writeln!(linha, "{}", agora).map_err(|_| Falha::Formato)?;
    escritor.write_all(&linha.bytes[..linha.usados])?;
    // `escritor` sai de escopo aqui e o descritor é fechado, antes de o
    // arquivo ser aberto para leitura.
    Ok(())
}

/// Escreve o caderno na tela, em pedaços de 128 bytes, e devolve quantas
/// linhas ele tem.
fn mostra() -> Result<usize, Falha> {
    let mut arquivo = File::open(CAMINHO)?;
    let mut pedaco = [0u8; 128];
    let mut linhas = 0;
    loop {
        match arquivo.read(&mut pedaco)? {
            // 0 bytes: o arquivo acabou.
            0 => return Ok(linhas),
            n => {
                // `write` entrega os bytes à tela exatamente como estão.
                write(&pedaco[..n]);
                linhas += pedaco[..n].iter().filter(|&&b| b == b'\n').count();
            }
        }
    }
}
