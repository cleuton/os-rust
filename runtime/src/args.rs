//! Argumentos de linha de comando. O programa recebe **um texto só**: o que vem
//! depois do nome dele no comando `exec` (o `run` não passa argumentos). Chega
//! cru, sem aspas nem escapes; separar em palavras é do programa ([`words`]).

pub use abi::MAX_ARGS_LEN;

use crate::sys;

/// Lê o texto de argumentos em `buf` e o devolve; vazio se não há. O texto
/// nunca passa de [`MAX_ARGS_LEN`] bytes, então o buffer sempre basta.
pub fn read(buf: &mut [u8; MAX_ARGS_LEN]) -> &str {
    let total = sys::args(&mut buf[..]);
    let Ok(total) = usize::try_from(total) else { return "" };
    // O kernel só entrega ASCII imprimível; qualquer outra coisa (impossível)
    // vira texto vazio em vez de um `unwrap`.
    core::str::from_utf8(&buf[..total.min(MAX_ARGS_LEN)]).unwrap_or("")
}

/// Separa o texto em palavras, por espaços. Não há aspas: `"a b"` vira duas
/// palavras, `"a` e `b"`.
pub fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(' ').filter(|word| !word.is_empty())
}
