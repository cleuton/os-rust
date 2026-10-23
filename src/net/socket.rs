//! Os sockets de cada programa e o pedido de rede pendente.
//!
//! Um **socket** é um número de 0 a 3, só do programa que o abriu, separado dos
//! descritores de arquivo. Cada posição aponta para um bloco de conexão TCP do
//! kernel (`Tcb`) e guarda o tempo limite das esperas daquele socket.
//!
//! Quando o programa termina (por qualquer motivo) a tabela é destruída com ele e
//! cada socket é fechado como `SYS_TCP_CLOSE`, do mesmo jeito que `FileTable`
//! devolve os arquivos abertos.

use abi::MAX_SOCKETS;

/// Tempo limite padrão de um socket novo, em milissegundos.
pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;

/// Um socket aberto.
#[derive(Clone, Copy, Debug)]
pub struct Socket {
    /// O bloco de conexão no kernel.
    pub conn: usize,
    /// Tempo limite das esperas (`accept`, `send`, `recv`), em milissegundos.
    pub timeout_ms: u64,
}

/// Os 4 sockets de um programa.
pub struct SocketTable {
    slots: [Option<Socket>; MAX_SOCKETS],
}

impl SocketTable {
    pub const fn new() -> SocketTable {
        SocketTable { slots: [None; MAX_SOCKETS] }
    }

    pub fn get(&self, index: usize) -> Option<&Socket> {
        self.slots.get(index)?.as_ref()
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut Socket> {
        self.slots.get_mut(index)?.as_mut()
    }

    pub fn is_full(&self) -> bool {
        self.slots.iter().all(|s| s.is_some())
    }

    pub fn count(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// Guarda o socket na primeira posição livre e devolve o número dele.
    pub fn insert(&mut self, conn: usize) -> Option<usize> {
        let index = self.slots.iter().position(|s| s.is_none())?;
        self.slots[index] = Some(Socket { conn, timeout_ms: DEFAULT_TIMEOUT_MS });
        Some(index)
    }

    /// Tira o socket da tabela (o número deixa de valer na hora).
    pub fn take(&mut self, index: usize) -> Option<Socket> {
        self.slots.get_mut(index)?.take()
    }
}

impl Default for SocketTable {
    fn default() -> Self {
        SocketTable::new()
    }
}

impl Drop for SocketTable {
    fn drop(&mut self) {
        for slot in self.slots.iter_mut() {
            if let Some(socket) = slot.take() {
                super::close_conn(socket.conn);
            }
        }
    }
}

/// Qual operação de rede a tarefa espera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetOp {
    Accept,
    Connect,
    Send,
    Recv,
}

/// Um pedido de rede em andamento: a tarefa está bloqueada até a pilha poder
/// atendê-lo ou o prazo vencer. O ponteiro e o tamanho já foram validados pela
/// syscall; o resultado só é calculado quando o espaço de endereçamento da
/// tarefa está ativo.
#[derive(Clone, Copy, Debug)]
pub struct PendingNet {
    pub op: NetOp,
    pub sock: usize,
    pub ptr: u64,
    pub len: u64,
    /// Tick em que a espera acaba (`u64::MAX` no `connect`, que tem prazo próprio).
    pub deadline: u64,
}
