//! `hello`: o primeiro programa de usuário do os-rust.
//!
//! Roda em modo usuário (ring 3): o processador o impede de tocar em
//! hardware ou na memória do kernel. Para escrever na tela, ele precisa
//! pedir ao kernel, e a biblioteca de runtime (`runtime/`) esconde a
//! instrução `syscall`: `println!` chama a syscall `write`, e o retorno de
//! `main` chama `exit`. O contrato completo (números, registradores, erros)
//! está em `SYSCALLS.md`, na raiz do repositório.
//!
//! No Marco 5, este programa tinha o `asm!` da instrução `syscall` escrito à
//! mão, de propósito, para mostrar o que acontece por baixo; esse `asm!`
//! agora vive uma vez só, em `runtime/src/sys.rs`, para todos os programas.
//!
//! Compilado à parte do kernel, para o target `x86_64-os_rust_user.json`,
//! e embutido na imagem de boot em tempo de compilação.

#![no_std]
#![no_main]

use runtime::{entry, println};

// Gera o `_start` e o liga a `main`.
entry!(main);

fn main() -> i32 {
    println!("Ola do ring 3!");
    // 0 = sucesso: `exit(0)` não mostra nada na tela.
    0
}
