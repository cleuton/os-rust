//! Escrita de texto na tela: `print!` e `println!`.

use core::fmt;

use abi::IO_MAX_LEN;

use crate::sys;

/// Tamanho do buffer de uma chamada `print!`/`println!`, em bytes. Uma linha
/// de até este tamanho sai em **uma** chamada `write`.
const BUFFER_SIZE: usize = 256;

// Uma chamada `write` aceita no máximo `IO_MAX_LEN` bytes: o buffer cabe numa só.
const _: () = assert!(BUFFER_SIZE <= IO_MAX_LEN as usize);

/// A tela, vista como algo em que se pode escrever texto formatado. O
/// `format_args!` produz o texto em vários trechos (`println!("a {}", 1)` gera
/// `"a "`, `"1"` e `"\n"`); aqui eles se juntam num buffer na pilha e saem
/// numa única chamada `write`. Com vários programas rodando ao mesmo tempo,
/// isso importa: cada `write` chega inteiro à tela (`SYSCALLS.md`, seção 8),
/// então uma linha escrita por uma chamada nunca é partida pela saída de
/// outro programa. Se o texto passar do buffer, ele sai em mais de uma chamada.
pub struct Out {
    buffer: [u8; BUFFER_SIZE],
    len: usize,
}

impl Out {
    fn new() -> Out {
        Out { buffer: [0; BUFFER_SIZE], len: 0 }
    }

    /// Entrega ao kernel o que está no buffer.
    fn flush(&mut self) -> fmt::Result {
        if self.len > 0 {
            let result = sys::write(&self.buffer[..self.len]);
            self.len = 0;
            if result < 0 {
                return Err(fmt::Error);
            }
        }
        Ok(())
    }
}

impl fmt::Write for Out {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut bytes = s.as_bytes();
        while !bytes.is_empty() {
            // Buffer cheio: manda o que tem e continua com o resto.
            if self.len == BUFFER_SIZE {
                self.flush()?;
            }
            let room = BUFFER_SIZE - self.len;
            let take = bytes.len().min(room);
            self.buffer[self.len..self.len + take].copy_from_slice(&bytes[..take]);
            self.len += take;
            bytes = &bytes[take..];
        }
        Ok(())
    }
}

/// Função por trás de `print!`/`println!`. Um erro de escrita é ignorado,
/// como no `print!` do kernel: não há o que fazer se a tela não aceita.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let mut out = Out::new();
    let _ = fmt::Write::write_fmt(&mut out, args);
    let _ = out.flush();
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
