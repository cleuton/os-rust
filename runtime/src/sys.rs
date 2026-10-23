//! A instrução `syscall` e um wrapper fino por chamada do contrato.
//!
//! É o único lugar da biblioteca (e, portanto, de todos os programas) com
//! `asm!` de syscall. O contrato completo (números, registradores, erros)
//! está em `SYSCALLS.md`, na raiz do repositório; os números vêm da crate
//! `abi`, a mesma que o kernel usa.

use core::arch::asm;

use abi::{
    DateTime, SYS_ALLOC, SYS_CLOSE, SYS_CREATE, SYS_EXIT, SYS_MKDIR, SYS_OPEN, SYS_OPEN_APPEND,
    SYS_READ, SYS_READ_DIR, SYS_READ_LINE, SYS_REMOVE, SYS_RMDIR, SYS_TIME, SYS_WRITE,
    SYS_TCP_ACCEPT, SYS_TCP_CLOSE, SYS_TCP_CONNECT, SYS_TCP_LISTEN, SYS_TCP_RECV, SYS_TCP_SEND,
    SYS_TCP_TIMEOUT, SYS_WRITE_FILE, SYS_YIELD,
    TIME_SIZE,
};

/// Executa `syscall` com o número `nr` e dois argumentos e devolve o
/// resultado (`rax`).
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

/// Como `syscall2`, com um terceiro argumento (`rdx`): `SYS_READ` e
/// `SYS_READ_DIR`.
///
/// # Safety
///
/// Os mesmos requisitos de `syscall2`.
#[inline(always)]
unsafe fn syscall3(nr: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    // SAFETY: igual a `syscall2`, com `rdx` = argumento 3 (preservado pelo
    // kernel, `SYSCALLS.md`, seção 4).
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") nr as i64 => ret,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
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

/// Pede ao kernel a data e a hora atuais (UTC) e as escreve em `buf`. Devolve
/// `0` ou um código de erro (`< 0`, por exemplo `abi::ERR_CLOCK`).
pub fn time(buf: &mut DateTime) -> i64 {
    // SAFETY: `buf` é uma referência exclusiva a um `DateTime` vivo do
    // programa: memória mapeada e gravável de `TIME_SIZE` bytes, que é o que
    // `SYS_TIME` exige do intervalo em que o kernel vai escrever.
    unsafe { syscall2(SYS_TIME, buf as *mut DateTime as u64, TIME_SIZE as u64) }
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

/// Abre o arquivo ou diretório `path` (somente leitura). Devolve o descritor
/// (`≥ 0`) ou um código de erro (`< 0`, ver `abi`). `path` tem no máximo
/// `abi::MAX_PATH_LEN` bytes.
pub fn open(path: &[u8]) -> i64 {
    // SAFETY: o ponteiro e o tamanho vêm de uma fatia viva do programa (memória
    // mapeada para ele), que é o que `SYS_OPEN` exige do caminho.
    unsafe { syscall2(SYS_OPEN, path.as_ptr() as u64, path.len() as u64) }
}

/// Lê bytes do arquivo aberto `fd` para `buf`, a partir da posição dele.
/// Devolve os bytes lidos (`0` no fim do arquivo) ou um código de erro.
/// `buf.len()` deve ser no máximo `abi::IO_MAX_LEN`.
pub fn read(fd: i64, buf: &mut [u8]) -> i64 {
    // SAFETY: o ponteiro e o tamanho vêm de uma fatia mutável viva do programa
    // (memória mapeada e gravável), que é o que `SYS_READ` exige do intervalo
    // em que o kernel vai escrever.
    unsafe { syscall3(SYS_READ, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64) }
}

/// Fecha o descritor `fd`. Devolve `0` ou um código de erro.
pub fn close(fd: i64) -> i64 {
    // SAFETY: `SYS_CLOSE` não recebe ponteiros; qualquer valor de `fd` é
    // válido (o kernel devolve erro para um descritor que não existe).
    unsafe { syscall2(SYS_CLOSE, fd as u64, 0) }
}

/// Lê a próxima entrada do diretório aberto `fd` para `buf` (que precisa ter
/// pelo menos `abi::DIR_ENTRY_SIZE` bytes). Devolve `1` (uma entrada escrita),
/// `0` (fim do diretório) ou um código de erro.
pub fn read_dir(fd: i64, buf: &mut [u8]) -> i64 {
    // SAFETY: o ponteiro e o tamanho vêm de uma fatia mutável viva do programa
    // (memória mapeada e gravável), que é o que `SYS_READ_DIR` exige.
    unsafe { syscall3(SYS_READ_DIR, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64) }
}

/// Cria o arquivo `path` (vazio) e o abre para acrescentar. Devolve o
/// descritor (`≥ 0`) ou um código de erro (`< 0`, por exemplo `abi::ERR_EXIST`).
pub fn create(path: &[u8]) -> i64 {
    // SAFETY: o ponteiro e o tamanho vêm de uma fatia viva do programa (memória
    // mapeada para ele), que é o que `SYS_CREATE` exige do caminho.
    unsafe { syscall2(SYS_CREATE, path.as_ptr() as u64, path.len() as u64) }
}

/// Abre o arquivo existente `path` para acrescentar bytes ao fim. Devolve o
/// descritor ou um código de erro.
pub fn open_append(path: &[u8]) -> i64 {
    // SAFETY: como em `create`: fatia viva do programa.
    unsafe { syscall2(SYS_OPEN_APPEND, path.as_ptr() as u64, path.len() as u64) }
}

/// Grava `buf` no fim do arquivo aberto para acrescentar `fd`. Devolve os
/// bytes gravados (pode ser menos que `buf.len()` se o volume encheu) ou um
/// código de erro. `buf.len()` deve ser no máximo `abi::IO_MAX_LEN`.
pub fn write_file(fd: i64, buf: &[u8]) -> i64 {
    // SAFETY: o ponteiro e o tamanho vêm de uma fatia viva do programa (memória
    // mapeada e legível), que é o que `SYS_WRITE_FILE` exige do intervalo que
    // o kernel vai ler.
    unsafe { syscall3(SYS_WRITE_FILE, fd as u64, buf.as_ptr() as u64, buf.len() as u64) }
}

/// Apaga o arquivo `path`. Devolve `0` ou um código de erro (`abi::ERR_BUSY`
/// se algum programa o tem aberto).
pub fn remove(path: &[u8]) -> i64 {
    // SAFETY: como em `create`: fatia viva do programa.
    unsafe { syscall2(SYS_REMOVE, path.as_ptr() as u64, path.len() as u64) }
}

/// Cria o diretório `path` (vazio). Não abre nada: não usa descritor. Devolve
/// `0` ou um código de erro (`abi::ERR_EXIST`, `abi::ERR_NOSPC`, ...).
pub fn mkdir(path: &[u8]) -> i64 {
    // SAFETY: como em `create`: fatia viva do programa.
    unsafe { syscall2(SYS_MKDIR, path.as_ptr() as u64, path.len() as u64) }
}

/// Remove o diretório vazio `path`. Devolve `0` ou um código de erro
/// (`abi::ERR_NOTEMPTY` se tem entradas, `abi::ERR_BUSY` se está aberto).
pub fn rmdir(path: &[u8]) -> i64 {
    // SAFETY: como em `create`: fatia viva do programa.
    unsafe { syscall2(SYS_RMDIR, path.as_ptr() as u64, path.len() as u64) }
}

/// Abre um socket de escuta TCP na porta `port` (1 a 65535). Devolve o número
/// do socket (`0..abi::MAX_SOCKETS`) ou um código de erro (`abi::ERR_ADDRINUSE`,
/// `abi::ERR_NOBUFS`, `abi::ERR_MFILE`, ...).
pub fn tcp_listen(port: u16) -> i64 {
    // SAFETY: `SYS_TCP_LISTEN` não recebe ponteiros.
    unsafe { syscall2(SYS_TCP_LISTEN, u64::from(port), 0) }
}

/// Espera uma conexão no socket de escuta `sock` e devolve o socket novo dela,
/// ou um código de erro (`abi::ERR_TIMEDOUT` quando o tempo limite do socket
/// acaba). O programa fica bloqueado sem gastar CPU.
pub fn tcp_accept(sock: i64) -> i64 {
    // SAFETY: `SYS_TCP_ACCEPT` não recebe ponteiros; qualquer `sock` é válido
    // (o kernel devolve erro para um número que não existe).
    unsafe { syscall2(SYS_TCP_ACCEPT, sock as u64, 0) }
}

/// Conecta em `ip:port` (`ip` como `a<<24 | b<<16 | c<<8 | d`) e espera o
/// aperto de mão de até 3 s. Devolve o socket ou um código de erro
/// (`abi::ERR_CONNREFUSED`, `abi::ERR_TIMEDOUT`, `abi::ERR_NODEV`, ...).
pub fn tcp_connect(ip: u32, port: u16) -> i64 {
    // SAFETY: `SYS_TCP_CONNECT` não recebe ponteiros.
    unsafe { syscall2(SYS_TCP_CONNECT, u64::from(ip), u64::from(port)) }
}

/// Copia `data` para o buffer de envio do socket. Devolve quantos bytes
/// entraram (`≥ 1`, pode ser menos que `data.len()`) ou um código de erro.
/// `data.len()` deve ser no máximo `abi::IO_MAX_LEN`.
pub fn tcp_send(sock: i64, data: &[u8]) -> i64 {
    // SAFETY: o ponteiro e o tamanho vêm de uma fatia viva do programa (memória
    // mapeada para ele), que é o que `SYS_TCP_SEND` exige do intervalo.
    unsafe { syscall3(SYS_TCP_SEND, sock as u64, data.as_ptr() as u64, data.len() as u64) }
}

/// Lê bytes recebidos do socket para `buf`. Devolve quantos (`0` quando o par
/// fechou a conexão) ou um código de erro. `buf.len()` deve ser no máximo
/// `abi::IO_MAX_LEN`.
pub fn tcp_recv(sock: i64, buf: &mut [u8]) -> i64 {
    // SAFETY: o ponteiro e o tamanho vêm de uma fatia mutável viva do programa
    // (memória mapeada e gravável), que é o que `SYS_TCP_RECV` exige.
    unsafe { syscall3(SYS_TCP_RECV, sock as u64, buf.as_mut_ptr() as u64, buf.len() as u64) }
}

/// Fecha o socket, sem esperar. Devolve `0` ou um código de erro.
pub fn tcp_close(sock: i64) -> i64 {
    // SAFETY: `SYS_TCP_CLOSE` não recebe ponteiros.
    unsafe { syscall2(SYS_TCP_CLOSE, sock as u64, 0) }
}

/// Define o tempo limite, em milissegundos (0 a 60 000), das esperas do socket.
/// Devolve `0` ou um código de erro.
pub fn tcp_timeout(sock: i64, ms: u32) -> i64 {
    // SAFETY: `SYS_TCP_TIMEOUT` não recebe ponteiros.
    unsafe { syscall2(SYS_TCP_TIMEOUT, sock as u64, u64::from(ms)) }
}
