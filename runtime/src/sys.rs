//! A instrução `syscall` e um wrapper fino por chamada do contrato.
//!
//! É o único lugar da biblioteca (e, portanto, de todos os programas) com
//! `asm!` de syscall. O contrato completo (números, registradores, erros)
//! está em `SYSCALLS.md`, na raiz do repositório; os números vêm da crate
//! `abi`, a mesma que o kernel usa.

use core::arch::asm;

use abi::{SYS_ALLOC, SYS_EXIT, SYS_READ_LINE, SYS_WRITE, SYS_YIELD};

/// Executa `syscall` com o número `nr` e dois argumentos e devolve o
/// resultado (`rax`). Nenhuma das syscalls deste contrato usa mais de dois
/// argumentos.
///
/// # Safety
///
/// Os argumentos precisam obedecer ao contrato da syscall `nr`: por exemplo,
/// um ponteiro precisa apontar para memória do programa com o tamanho
/// declarado. Um número de syscall inexistente encerra o programa.
#[inline(always)]
unsafe fn syscall2(nr: u64, a1: u64, a2: u64) -> i64 {
    let ret: i64;
    // SAFETY: a instrução segue a convenção de `SYSCALLS.md`, seção 4.
    //   rax = número da syscall (na volta, o resultado)
    //   rdi = argumento 1, rsi = argumento 2 (preservados pelo kernel)
    //   rcx e r11 = destruídos pela própria instrução `syscall` (a CPU guarda
    //   neles o endereço de retorno e as flags); por isso são declarados
    //   como saídas descartadas.
    // O kernel preserva todos os outros registradores e não mexe na pilha do
    // programa (usa uma pilha própria), então `nostack` é verdade. A validade
    // dos argumentos é obrigação de quem chama (ver `# Safety` acima).
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") nr as i64 => ret,
            in("rdi") a1,
            in("rsi") a2,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    ret
}

/// Encerra o programa com o código de saída `code` e devolve o controle ao
/// prompt. `0` significa sucesso (nada aparece na tela); qualquer outro valor
/// faz o kernel escrever `[run] <nome> terminou com codigo <n>`. O código
/// segue para o kernel como `u32`: `-1` aparece como `4294967295`.
pub fn exit(code: i32) -> ! {
    // SAFETY: `SYS_EXIT` não usa ponteiros e nunca retorna: o kernel abandona
    // o programa e volta ao prompt, então `noreturn` é verdade e nenhum
    // registrador precisa ser preservado.
    unsafe {
        asm!(
            "syscall",
            in("rax") SYS_EXIT,
            in("rdi") code as u32 as u64,
            options(noreturn, nostack),
        );
    }
}

/// Escreve `bytes` na tela. Devolve os bytes escritos (`≥ 0`) ou um código de
/// erro (`< 0`, ver `abi`). `bytes` deve ter no máximo `abi::IO_MAX_LEN`.
pub fn write(bytes: &[u8]) -> i64 {
    // SAFETY: o ponteiro e o tamanho vêm de uma fatia viva do programa (memória
    // mapeada para ele), que é exatamente o que `SYS_WRITE` exige.
    unsafe { syscall2(SYS_WRITE, bytes.as_ptr() as u64, bytes.len() as u64) }
}

/// Espera uma linha digitada e a escreve em `buf`, com o `\n` final. Devolve os
/// bytes escritos (`≥ 1`) ou um código de erro (`< 0`). O programa fica
/// bloqueado até o Enter. `buf.len()` deve ser no máximo `abi::IO_MAX_LEN`.
pub fn read_line_raw(buf: &mut [u8]) -> i64 {
    // SAFETY: o ponteiro e o tamanho vêm de uma fatia mutável viva do programa
    // (memória mapeada e gravável), que é o que `SYS_READ_LINE` exige do
    // intervalo em que o kernel vai escrever.
    unsafe { syscall2(SYS_READ_LINE, buf.as_mut_ptr() as u64, buf.len() as u64) }
}

/// Pede ao kernel `size` bytes de memória (arredondados para cima até
/// páginas de 4 KiB). Devolve o endereço do início da área nova, ou um código
/// de erro (`< 0`, por exemplo `abi::ERR_NOMEM`). As áreas são contíguas.
pub fn alloc(size: usize) -> i64 {
    // SAFETY: `SYS_ALLOC` não recebe ponteiros; qualquer `size` é válido (o
    // kernel devolve erro se não puder atender).
    unsafe { syscall2(SYS_ALLOC, size as u64, 0) }
}

/// Cede a CPU: o kernel passa a vez ao próximo programa pronto e só volta a
/// este quando chegar a vez dele, na instrução seguinte. Se nenhum outro
/// programa está pronto, volta na hora. Nunca falha.
///
/// Um programa não precisa chamar isto: o timer do kernel o interrompe de
/// qualquer jeito depois de uma fatia de tempo. Chamar é só uma gentileza
/// (e a forma de dois programas se alternarem numa ordem previsível).
pub fn yield_now() {
    // SAFETY: `SYS_YIELD` não recebe ponteiros nem argumentos e nunca falha.
    // O kernel preserva todos os registradores, menos `rax` (resultado, que
    // vale 0 e é ignorado aqui) e `rcx`/`r11` (que o contrato declara
    // destruídos; por isso são saídas descartadas), e não mexe na pilha do
    // programa, então `nostack` é verdade.
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") SYS_YIELD as i64 => _,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
}
