//! `hello`: o primeiro programa de usuário do os-rust.
//!
//! Roda em modo usuário (ring 3): o processador o impede de tocar em
//! hardware ou na memória do kernel. Para escrever na tela, ele precisa
//! pedir ao kernel, com a instrução `syscall`. O contrato completo (números,
//! registradores, erros) está em `SYSCALLS.md`, na raiz do repositório; este
//! programa usa exatamente duas chamadas: `write` e `exit`.
//!
//! Compilado à parte do kernel, para o target `x86_64-os_rust_user.json`,
//! e embutido na imagem de boot em tempo de compilação. Sem biblioteca de
//! runtime (ela é assunto do Marco 6): a instrução `syscall` aparece crua,
//! de propósito, para mostrar o que acontece por baixo.

#![no_std]
#![no_main]

use core::arch::asm;

/// O texto que o programa escreve. Vai para a seção somente-leitura do
/// executável (`.rodata`), que o kernel carrega numa página não gravável.
static MENSAGEM: &[u8] = b"Ola do ring 3!\n";

/// Ponto de entrada: o kernel salta para `_start` com todos os registradores
/// zerados, `rsp` no topo da pilha do usuário (alinhado como numa chamada de
/// função) e sem argumentos (`SYSCALLS.md`, seção 3). O programa nunca
/// retorna de `_start`: não há para onde voltar, deve chamar `exit`.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // SAFETY: as duas instruções `syscall` seguem o contrato de
    // `SYSCALLS.md`: os argumentos estão nos registradores certos, o
    // ponteiro e o tamanho descrevem um buffer que é deste programa
    // (`MENSAGEM`, em memória mapeada para ele), e o kernel preserva tudo
    // menos `rax`, `rcx` e `r11`.
    unsafe {
        // write(ptr, len): syscall número 1 (`SYS_WRITE`, `SYSCALLS.md` §5).
        //   rax = 1 (número da syscall; na volta, o resultado)
        //   rdi = endereço do texto
        //   rsi = quantidade de bytes
        //   rcx e r11 = destruídos pela própria instrução `syscall` (a CPU
        //   guarda neles o endereço de retorno e as flags)
        asm!(
            "syscall",
            inlateout("rax") 1u64 => _,
            in("rdi") MENSAGEM.as_ptr(),
            in("rsi") MENSAGEM.len(),
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );

        // exit(code): syscall número 2 (`SYS_EXIT`, `SYSCALLS.md` §5).
        //   rax = 2
        //   rdi = 0 (código de saída: 0 significa sucesso)
        // Não retorna: o kernel abandona o programa e volta ao prompt.
        asm!(
            "syscall",
            in("rax") 2u64,
            in("rdi") 0u64,
            options(noreturn, nostack),
        );
    }
}

/// Um programa sem biblioteca padrão precisa dizer o que fazer num `panic!`.
/// Este programa não tem nada que possa entrar em pânico; se acontecer, fica
/// parado (o kernel não interfere: fora do contrato, ver `SYSCALLS.md` §7).
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
