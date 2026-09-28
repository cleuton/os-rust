//! Escrita de texto na tela: `print!` e `println!`.

use core::fmt;

use abi::IO_MAX_LEN;

use crate::sys;

/// A tela, vista como algo em que se pode escrever texto formatado. Cada
/// trecho de texto que o `format_args!` produz vira uma chamada `write`
/// (`println!("a {}", 1)` faz três): simples de propósito, e o custo de uma
/// syscall é irrelevante aqui.
pub struct Out;

impl fmt::Write for Out {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // `write` aceita no máximo `IO_MAX_LEN` bytes por chamada.
        for chunk in s.as_bytes().chunks(IO_MAX_LEN as usize) {
            if sys::write(chunk) < 0 {
                return Err(fmt::Error);
            }
        }
        Ok(())
    }
}

/// Função por trás de `print!`/`println!`. Um erro de escrita é ignorado,
/// como no `print!` do kernel: não há o que fazer se a tela não aceita.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let _ = fmt::Write::write_fmt(&mut Out, args);
}

/// Escreve na tela, sem quebra de linha no fim. Mesma sintaxe de `format!`.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::io::_print(format_args!($($arg)*))
    };
}

/// Escreve na tela, com quebra de linha no fim. Sem argumentos, só a quebra.
#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n")
    };
    ($($arg:tt)*) => {
        $crate::print!("{}\n", format_args!($($arg)*))
    };
}

/// Espera a pessoa digitar uma linha e apertar Enter, e a devolve **sem** o
/// Enter, como texto que aponta para `buf`. Enquanto espera, o sistema mostra
/// o que é digitado e apaga com Backspace. Cabem `min(buf.len(), 128) - 1`
/// caracteres; o que passar disso é ignorado. Um `buf` vazio devolve `""` sem
/// esperar.
///
/// Devolve `&str` (e não um `Result`) porque, pela API segura, o kernel não
/// tem como falhar: o ponteiro é sempre válido e o tamanho é limitado aqui.
/// O kernel só entrega ASCII imprimível, então o texto é sempre UTF-8 válido.
pub fn read_line(buf: &mut [u8]) -> &str {
    if buf.is_empty() {
        return "";
    }
    // O kernel recusa `len` acima do limite: recorta aqui.
    let limit = buf.len().min(IO_MAX_LEN as usize);
    let written = sys::read_line_raw(&mut buf[..limit]);
    // Um valor menor que 1 seria um erro do kernel, impossível por esta API
    // (ver a doc acima); devolver "" evita um `unwrap` sem rede de segurança.
    let Ok(written) = usize::try_from(written) else {
        return "";
    };
    if written == 0 {
        return "";
    }
    let line = &buf[..written];
    // Tira o `\n` final que o kernel sempre escreve.
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    core::str::from_utf8(line).unwrap_or("")
}
