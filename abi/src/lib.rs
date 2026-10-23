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

/// `create(path_ptr, path_len)`: cria um arquivo vazio e o abre para
/// acrescentar. Devolve um descritor. O nome não pode existir.
pub const SYS_CREATE: u64 = 11;
/// `open_append(path_ptr, path_len)`: abre um arquivo existente para
/// acrescentar bytes ao fim. Devolve um descritor.
pub const SYS_OPEN_APPEND: u64 = 12;
/// `write_file(fd, ptr, len)`: grava `len` bytes no fim do arquivo aberto para
/// acrescentar. Devolve os bytes gravados.
pub const SYS_WRITE_FILE: u64 = 13;
/// `remove(path_ptr, path_len)`: apaga um arquivo que ninguém tem aberto.
/// Devolve `0`.
pub const SYS_REMOVE: u64 = 14;
/// `mkdir(path_ptr, path_len)`: cria um diretório vazio (1 cluster, com `.` e
/// `..`). Não abre nada: não usa descritor. Devolve `0`.
pub const SYS_MKDIR: u64 = 15;
/// `rmdir(path_ptr, path_len)`: remove um diretório vazio que ninguém tem
/// aberto e que não é a raiz de um volume. Devolve `0`.
pub const SYS_RMDIR: u64 = 16;

/// `tcp_listen(port)`: abre um socket de escuta na porta. Devolve o número do
/// socket (`0..MAX_SOCKETS`).
pub const SYS_TCP_LISTEN: u64 = 17;
/// `tcp_accept(sock)`: espera uma conexão estabelecida na fila do socket de
/// escuta e devolve um socket novo para ela. Bloqueia (com tempo limite).
pub const SYS_TCP_ACCEPT: u64 = 18;
/// `tcp_connect(ip, port)`: abre uma conexão e espera o aperto de mão de três
/// vias. Devolve o socket conectado. Bloqueia (tempo fixo de 3 s).
pub const SYS_TCP_CONNECT: u64 = 19;
/// `tcp_send(sock, ptr, len)`: copia até `len` bytes para o buffer de envio.
/// Devolve os bytes aceitos.
pub const SYS_TCP_SEND: u64 = 20;
/// `tcp_recv(sock, ptr, len)`: copia até `len` bytes recebidos para `ptr`.
/// Devolve os bytes copiados; `0` quando o par fechou.
pub const SYS_TCP_RECV: u64 = 21;
/// `tcp_close(sock)`: fecha o socket, sem esperar. Devolve `0`.
pub const SYS_TCP_CLOSE: u64 = 22;
/// `tcp_timeout(sock, ms)`: define o tempo limite das esperas do socket.
/// Devolve `0`.
pub const SYS_TCP_TIMEOUT: u64 = 23;

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
/// inválido, erro ao montar); em rede, a placa não existe e o destino não é
/// `127.0.0.0/8`.
pub const ERR_NODEV: i64 = -5;
/// Tipo errado: ler como arquivo um diretório, listar como diretório um
/// arquivo, ou usar um arquivo como componente intermediário do caminho.
pub const ERR_TYPE: i64 = -6;
/// Descritor inválido: fora de `0..MAX_OPEN_FILES`, não aberto ou já fechado.
pub const ERR_BADF: i64 = -7;
/// O programa já tem `MAX_OPEN_FILES` arquivos abertos (ou `MAX_SOCKETS`
/// sockets, nas syscalls de rede).
pub const ERR_MFILE: i64 = -8;
/// Caminho com mais de `MAX_PATH_LEN` bytes (ou profundidade demais).
pub const ERR_NAMETOOLONG: i64 = -9;
/// Erro ao ler o volume: disco que não responde, erro do disco ou estrutura
/// FAT corrompida.
pub const ERR_IO: i64 = -10;
/// O relógio devolveu valores impossíveis, não se estabilizou ou não
/// respondeu (`SYS_TIME`).
pub const ERR_CLOCK: i64 = -11;
/// `SYS_CREATE`: o nome já existe (arquivo, diretório ou a raiz do volume).
pub const ERR_EXIST: i64 = -12;
/// Sem cluster livre no volume, ou diretório sem entrada livre.
pub const ERR_NOSPC: i64 = -13;
/// O volume existe e está montado, mas não aceita escrita (um volume com mais
/// de 1 setor por cluster, ou um dispositivo que não sabe gravar).
pub const ERR_ROFS: i64 = -14;
/// `SYS_REMOVE` de um arquivo, ou `SYS_RMDIR` de um diretório, que algum
/// programa tem aberto; `SYS_RMDIR` da raiz de um volume.
pub const ERR_BUSY: i64 = -15;
/// `SYS_RMDIR` de um diretório que tem qualquer entrada além de `.`, `..` e
/// entradas apagadas.
pub const ERR_NOTEMPTY: i64 = -16;
/// O tempo limite do socket esgotou (`accept`, `recv`, `send`), ou a conexão
/// desistiu (SYN sem resposta em 3 tentativas, dado sem confirmação em 5
/// retransmissões).
pub const ERR_TIMEDOUT: i64 = -17;
/// `SYS_TCP_CONNECT`: o par respondeu RST ao SYN (inclusive o loopback sem
/// servidor).
pub const ERR_CONNREFUSED: i64 = -18;
/// A conexão foi reiniciada pelo par (RST) depois de estabelecida.
pub const ERR_CONNRESET: i64 = -19;
/// `SYS_TCP_LISTEN` numa porta que já tem `listen`.
pub const ERR_ADDRINUSE: i64 = -20;
/// Já existem `MAX_TCP_CONNECTIONS` conexões TCP no kernel.
pub const ERR_NOBUFS: i64 = -21;

/// Sockets abertos ao mesmo tempo por programa (separados dos arquivos).
pub const MAX_SOCKETS: usize = 4;
/// Blocos de conexão TCP no kernel inteiro (incluindo os de escuta).
pub const MAX_TCP_CONNECTIONS: usize = 8;
/// Maior tempo limite aceito por `SYS_TCP_TIMEOUT`, em milissegundos.
pub const TCP_MAX_TIMEOUT_MS: u64 = 60_000;

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
