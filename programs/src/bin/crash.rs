//! `crash`: um programa de usuário que erra de propósito.
//!
//! Existe para mostrar, na aula, a diferença entre um erro **no kernel**
//! (o comando `falha opcode` do prompt para tudo e mostra a tela de
//! exceção) e um erro **num programa** (aqui, o kernel encerra só o
//! programa, mostra uma mensagem legível e o prompt continua funcionando).
//! Uma falha em programa de usuário nunca derruba o kernel: é a regra do
//! contrato (`SYSCALLS.md`, seção 7).

#![no_std]
#![no_main]

use core::arch::asm;

/// Ponto de entrada (mesmo esqueleto de `hello`): executa `ud2`, o opcode
/// que a arquitetura x86 reserva como sempre inválido. A CPU levanta uma
/// exceção de instrução inválida (`#UD`) em ring 3; o kernel a trata,
/// encerra o programa e devolve o controle ao prompt.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // SAFETY: `ud2` não lê nem escreve memória; a exceção que ele provoca é
    // exatamente o comportamento intencional deste programa.
    unsafe { asm!("ud2", options(noreturn)) }
}

/// Um programa sem biblioteca padrão precisa dizer o que fazer num `panic!`.
/// Este não tem nada que possa entrar em pânico; se acontecer, fica parado.
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
