//! O contrato de syscalls do os-rust em constantes.
//!
//! O texto do contrato (números, registradores, semântica de cada chamada,
//! erros, limites) é o `SYSCALLS.md`, na raiz do repositório: ele é a única
//! fonte da interface (Princípio VII da constitution). Esta crate só guarda
//! os valores que kernel e biblioteca de runtime precisam **concordar**.
//! Ambos dependem daqui, então os números nunca divergem; e o teste do
//! kernel que compara `SYSCALLS.md` com estas constantes protege os dois
//! lados.

#![no_std]

/// `write(ptr, len)`: escreve `len` bytes, a partir de `ptr`, na tela.
pub const SYS_WRITE: u64 = 1;
/// `exit(code)`: encerra o programa e devolve o controle ao prompt.
pub const SYS_EXIT: u64 = 2;
/// `read_line(ptr, len)`: espera uma linha do teclado e a entrega em `ptr`.
pub const SYS_READ_LINE: u64 = 3;
/// `alloc(size)`: amplia o heap do programa em `size` bytes.
pub const SYS_ALLOC: u64 = 4;

/// Ponteiro ou intervalo inválido (fora da região do usuário, não mapeado,
/// ou não gravável quando o kernel precisa escrever nele).
pub const ERR_FAULT: i64 = -1;
/// Argumento fora dos limites permitidos (`len` acima de `IO_MAX_LEN`, ou
/// `size == 0` em `SYS_ALLOC`).
pub const ERR_INVAL: i64 = -2;
/// `SYS_ALLOC` não pôde dar a memória pedida (limite do heap do programa
/// ou falta de memória física).
pub const ERR_NOMEM: i64 = -3;

/// Maior `len` aceito por `SYS_WRITE` e por `SYS_READ_LINE`.
pub const IO_MAX_LEN: u64 = 4096;
