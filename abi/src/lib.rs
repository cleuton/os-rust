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

/// `open(path_ptr, path_len)`: abre um arquivo ou diretório, somente para
/// leitura, e devolve um descritor (`0..MAX_OPEN_FILES`).
pub const SYS_OPEN: u64 = 6;
/// `read(fd, ptr, len)`: lê bytes do arquivo aberto, em sequência. Devolve
/// `0` no fim do arquivo.
pub const SYS_READ: u64 = 7;
/// `close(fd)`: fecha o descritor.
pub const SYS_CLOSE: u64 = 8;
/// `read_dir(fd, ptr, len)`: escreve em `ptr` a próxima entrada (um
/// `DirEntryRaw`) do diretório aberto. Devolve `1`, ou `0` no fim.
pub const SYS_READ_DIR: u64 = 9;

/// `time(ptr, len)`: escreve em `ptr` a data e a hora atuais, em UTC, como um
/// `DateTime` (`len` tem de ser `TIME_SIZE`). Devolve `0`.
pub const SYS_TIME: u64 = 10;

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

/// O caminho (ou um componente dele) não existe.
pub const ERR_NOENT: i64 = -4;
/// Volume desconhecido, ou conhecido mas indisponível (sem disco, volume
/// inválido, erro ao montar).
pub const ERR_NODEV: i64 = -5;
/// Tipo errado: ler como arquivo um diretório, listar como diretório um
/// arquivo, ou usar um arquivo como componente intermediário do caminho.
pub const ERR_TYPE: i64 = -6;
/// Descritor inválido: fora de `0..MAX_OPEN_FILES`, não aberto ou já fechado.
pub const ERR_BADF: i64 = -7;
/// O programa já tem `MAX_OPEN_FILES` arquivos abertos.
pub const ERR_MFILE: i64 = -8;
/// Caminho com mais de `MAX_PATH_LEN` bytes (ou profundidade demais).
pub const ERR_NAMETOOLONG: i64 = -9;
/// Erro ao ler o volume: disco que não responde, erro do disco ou estrutura
/// FAT corrompida.
pub const ERR_IO: i64 = -10;
/// O relógio devolveu valores impossíveis, não se estabilizou ou não
/// respondeu (`SYS_TIME`).
pub const ERR_CLOCK: i64 = -11;

/// Maior caminho aceito por `SYS_OPEN`, em bytes.
pub const MAX_PATH_LEN: usize = 64;
/// Arquivos abertos ao mesmo tempo por programa.
pub const MAX_OPEN_FILES: usize = 4;
/// Maior executável que `run` carrega de um arquivo, em bytes (64 KiB). O
/// arquivo é lido inteiro para o heap do kernel, então o limite protege o heap.
pub const MAX_EXEC_SIZE: usize = 65536;

/// Tamanho, em bytes, de um `DirEntryRaw`.
pub const DIR_ENTRY_SIZE: usize = 20;
/// `DirEntryRaw::kind` de um arquivo.
pub const KIND_FILE: u8 = 1;
/// `DirEntryRaw::kind` de um diretório.
pub const KIND_DIR: u8 = 2;

/// Uma entrada de diretório como `SYS_READ_DIR` a entrega ao programa:
/// 20 bytes, `size` em little-endian.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirEntryRaw {
    /// Nome em minúsculas (`nome.ext`), terminado e preenchido com zeros.
    pub name: [u8; 12],
    /// `KIND_FILE` ou `KIND_DIR`.
    pub kind: u8,
    /// Zeros (alinhamento de `size`).
    pub _pad: [u8; 3],
    /// Tamanho em bytes; `0` para diretório.
    pub size: u32,
}

// O layout é parte do contrato: kernel e runtime compartilham este tipo.
const _: () = assert!(core::mem::size_of::<DirEntryRaw>() == DIR_ENTRY_SIZE);

/// Tamanho, em bytes, de um `DateTime`.
pub const TIME_SIZE: usize = 8;

/// A data e a hora como `SYS_TIME` as entrega ao programa: 8 bytes, `year` em
/// little-endian. Sempre UTC.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    /// Ano completo (por exemplo, 2026).
    pub year: u16,
    /// Mês, de 1 a 12.
    pub month: u8,
    /// Dia, de 1 até o último dia do mês.
    pub day: u8,
    /// Hora, de 0 a 23.
    pub hour: u8,
    /// Minuto, de 0 a 59.
    pub minute: u8,
    /// Segundo, de 0 a 59.
    pub second: u8,
    /// Zero (alinhamento).
    pub _pad: u8,
}

// O layout é parte do contrato: kernel e runtime compartilham este tipo.
const _: () = assert!(core::mem::size_of::<DateTime>() == TIME_SIZE);

/// `AAAA-MM-DD HH:MM:SS`: o formato único do `data`, do `hora` e dos testes.
impl core::fmt::Display for DateTime {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}
