//! O contrato de syscalls do os-rust em constantes.
//!
//! O texto do contrato (números, registradores, semântica de cada chamada,
//! erros, limites) é o `SYSCALLS.md`, na raiz do repositório: ele é a única
//! fonte da interface. Esta crate só guarda
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

/// `yield()`: cede a CPU ao próximo programa pronto. Não recebe argumentos,
/// devolve `0` e nunca falha.
pub const SYS_YIELD: u64 = 5;

/// Máximo de programas que `run` carrega ao mesmo tempo. Pedir mais que isso
/// é recusado antes de iniciar qualquer programa.
pub const MAX_TASKS: usize = 4;

/// Frequência do timer (PIT), em interrupções por segundo.
pub const TIMER_HZ: u64 = 100;

/// Fatia de tempo de cada programa, em ticks do timer: 5 ticks, ou seja, 50 ms
/// a `TIMER_HZ` = 100. O timer só tira a CPU de um programa depois que o
/// contador do kernel avançou `SLICE_TICKS` ticks desde que ele foi colocado na
/// CPU.
///
/// Por que 5 e não 1: o kernel roda com as interrupções desligadas, então um
/// tick que chega durante uma syscall fica pendente e é entregue no instante
/// em que o programa volta a rodar, e o PIC guarda **no máximo um** tick
/// pendente. Logo, cada volta de uma syscall pode somar um tick à fatia sem que
/// o programa tenha computado nada. Um programa que escreve e cede a CPU (como
/// `ping`) faz poucas syscalls por vez (menos de 5), então nunca acumula uma
/// fatia só com isso: só é interrompido se calcular, em ring 3, por 40 ms ou
/// mais. A ordem de um par cooperativo é, portanto, a do rodízio de
/// `SYS_YIELD`.
pub const SLICE_TICKS: u64 = 5;

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
