//! Biblioteca de runtime dos programas de usuário do os-rust.
//!
//! É o equivalente da libc do projeto: quem escreve um programa não chama a
//! instrução `syscall` na mão. A biblioteca oferece:
//!
//! - [`entry!`]: liga a função `main` do programa ao ponto de entrada
//!   (`_start`) e chama `exit` com o valor que `main` devolve;
//! - [`print!`] e [`println!`]: escrevem texto na tela;
//! - [`read_line`]: espera uma linha digitada e a devolve como texto;
//! - um alocador global, que permite usar `Box`, `Vec` e `String` (com
//!   `extern crate alloc;` no programa);
//! - [`File`], [`Dir`] e [`FsError`]: abrir e ler arquivos e listar diretórios
//!   (somente leitura; volumes `/ram` e `/disco`);
//! - [`time::now`]: a data e a hora atuais, em UTC;
//! - [`exit`]: encerra o programa mais cedo;
//! - [`yield_now`]: cede a CPU a outro programa (multitarefa);
//! - um tratador de `panic!` que escreve `[panic] <mensagem>` e sai com o
//!   código 101.
//!
//! O contrato com o kernel (números das syscalls, erros, limites) está em
//! `SYSCALLS.md`, na raiz do repositório; os números vêm da crate `abi`,
//! compartilhada com o kernel.

#![no_std]

pub mod fs;
pub mod heap;
pub mod io;
pub mod sys;
pub mod time;

pub use fs::{Dir, DirEntry, File, FsError};
pub use io::read_line;
pub use sys::{exit, yield_now};
pub use time::{DateTime, TimeError};

/// Gera o ponto de entrada (`_start`) do programa e o liga à função `main`.
///
/// ```ignore
/// use runtime::{entry, println};
///
/// entry!(main);
///
/// fn main() -> i32 {
///     println!("ola");
///     0
/// }
/// ```
///
/// O `_start` nasce **dentro do programa**, chamando `main` com a assinatura
/// conferida pelo compilador (`fn() -> i32`): um `main` com outro tipo não
/// compila. O valor devolvido vira o código de saída (`0` é sucesso). O
/// contrato (`SYSCALLS.md`, seção 3) diz que `_start` nunca retorna, e não
/// retorna: termina em `exit`.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[no_mangle]
        pub extern "C" fn _start() -> ! {
            let code: i32 = $main();
            $crate::exit(code)
        }
    };
}

/// O que fazer num `panic!`: escrever `[panic] <mensagem>` na tela e sair com
/// o código 101 (o mesmo que o Rust usa no Linux). O kernel mostra
/// `[run] <nome> terminou com codigo 101` e volta ao prompt. Uma alocação que
/// falha também cai aqui (o `alloc` do Rust a transforma em `panic`).
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    crate::println!("[panic] {}", info);
    exit(101)
}
