//! Rede TCP para programas: [`TcpListener`] (servidor) e [`TcpStream`]
//! (conexão), mais [`Ipv4`] e [`NetError`].
//!
//! Os dois tipos fecham o socket sozinhos quando saem de escopo. Toda espera
//! tem tempo limite (padrão 10 s, `set_timeout` muda; não existe espera
//! infinita), então um servidor esquecido nunca prende o computador.

use core::fmt;

use abi::{
    ERR_ADDRINUSE, ERR_BADF, ERR_CONNREFUSED, ERR_CONNRESET, ERR_FAULT, ERR_INVAL, ERR_MFILE,
    ERR_NOBUFS, ERR_NODEV, ERR_TIMEDOUT, ERR_TYPE, IO_MAX_LEN,
};

use crate::sys;

/// Um endereço IPv4: quatro bytes, `a.b.c.d`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv4(pub [u8; 4]);

impl Ipv4 {
    /// Interpreta `a.b.c.d` (quatro números de 0 a 255). `None` se o texto não
    /// tem esse formato.
    pub fn parse(text: &str) -> Option<Ipv4> {
        let mut bytes = [0u8; 4];
        let mut parts = text.split('.');
        for byte in bytes.iter_mut() {
            let part = parts.next()?;
            if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let mut value: u16 = 0;
            for digit in part.bytes() {
                value = value * 10 + u16::from(digit - b'0');
            }
            *byte = u8::try_from(value).ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        Some(Ipv4(bytes))
    }

    /// O endereço como o kernel o recebe: `a<<24 | b<<16 | c<<8 | d`.
    pub fn to_u32(self) -> u32 {
        u32::from_be_bytes(self.0)
    }
}

impl fmt::Display for Ipv4 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}

/// Por que uma operação de rede falhou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    /// O tempo limite do socket acabou, ou a conexão desistiu.
    TimedOut,
    /// O outro lado recusou a conexão (não há ninguém ouvindo).
    Refused,
    /// O outro lado reiniciou a conexão.
    Reset,
    /// Já há alguém ouvindo nessa porta.
    AddrInUse,
    /// Todas as conexões do kernel estão em uso.
    NoBuffers,
    /// Não há rede para chegar a esse endereço (sem placa).
    NoNetwork,
    /// O programa já tem [`abi::MAX_SOCKETS`] sockets abertos.
    TooManySockets,
    /// Socket inválido (não deveria acontecer com [`TcpListener`] e [`TcpStream`]).
    BadSocket,
    /// Operação de outro tipo de socket.
    WrongType,
    /// Argumento inválido (porta, endereço, tamanho, tempo).
    Invalid,
    /// Ponteiro inválido (não acontece com fatias vivas).
    Fault,
}

impl NetError {
    /// Traduz um código de erro (`< 0`) de syscall de rede.
    pub fn from_code(code: i64) -> NetError {
        match code {
            ERR_TIMEDOUT => NetError::TimedOut,
            ERR_CONNREFUSED => NetError::Refused,
            ERR_CONNRESET => NetError::Reset,
            ERR_ADDRINUSE => NetError::AddrInUse,
            ERR_NOBUFS => NetError::NoBuffers,
            ERR_NODEV => NetError::NoNetwork,
            ERR_MFILE => NetError::TooManySockets,
            ERR_BADF => NetError::BadSocket,
            ERR_TYPE => NetError::WrongType,
            ERR_INVAL => NetError::Invalid,
            ERR_FAULT => NetError::Fault,
            // Um código que este contrato não conhece (de uma versão futura).
            _ => NetError::Reset,
        }
    }
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            NetError::TimedOut => "tempo esgotado",
            NetError::Refused => "recusado",
            NetError::Reset => "conexao reiniciada",
            NetError::AddrInUse => "porta em uso",
            NetError::NoBuffers => "sem conexoes livres",
            NetError::NoNetwork => "sem rede",
            NetError::TooManySockets => "sockets demais abertos",
            NetError::BadSocket => "socket invalido",
            NetError::WrongType => "tipo de socket errado",
            NetError::Invalid => "argumento invalido",
            NetError::Fault => "ponteiro invalido",
        })
    }
}

/// Converte o resultado de uma syscall (`< 0` é erro) em `Result`.
fn check(code: i64) -> Result<i64, NetError> {
    if code < 0 {
        Err(NetError::from_code(code))
    } else {
        Ok(code)
    }
}

/// Um servidor TCP: espera conexões numa porta.
pub struct TcpListener {
    sock: i64,
}

impl TcpListener {
    /// Começa a ouvir na porta (1 a 65535).
    pub fn bind(port: u16) -> Result<TcpListener, NetError> {
        let sock = check(sys::tcp_listen(port))?;
        Ok(TcpListener { sock })
    }

    /// Espera uma conexão até o tempo limite do socket (padrão 10 s).
    pub fn accept(&self) -> Result<TcpStream, NetError> {
        let sock = check(sys::tcp_accept(self.sock))?;
        Ok(TcpStream { sock })
    }

    /// Muda o tempo limite das esperas, em milissegundos (0 a 60 000).
    pub fn set_timeout(&self, ms: u32) -> Result<(), NetError> {
        check(sys::tcp_timeout(self.sock, ms)).map(|_| ())
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        sys::tcp_close(self.sock);
    }
}

/// Uma conexão TCP.
pub struct TcpStream {
    sock: i64,
}

impl TcpStream {
    /// Conecta em `ip:port` e espera o aperto de mão (até 3 s).
    pub fn connect(ip: Ipv4, port: u16) -> Result<TcpStream, NetError> {
        let sock = check(sys::tcp_connect(ip.to_u32(), port))?;
        Ok(TcpStream { sock })
    }

    /// Lê o que chegou, no máximo `buf.len()` bytes (até 4096). Devolve `0`
    /// quando o outro lado fechou a conexão. Espera até o tempo limite.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, NetError> {
        let len = buf.len().min(IO_MAX_LEN as usize);
        check(sys::tcp_recv(self.sock, &mut buf[..len])).map(|n| n as usize)
    }

    /// Envia o que couber de `data` (até 4096) e devolve quantos bytes entraram
    /// no buffer de envio. Use [`TcpStream::write_all`] para enviar tudo.
    pub fn write(&self, data: &[u8]) -> Result<usize, NetError> {
        let len = data.len().min(IO_MAX_LEN as usize);
        check(sys::tcp_send(self.sock, &data[..len])).map(|n| n as usize)
    }

    /// Envia `data` inteiro, esperando espaço no buffer quando preciso.
    pub fn write_all(&self, mut data: &[u8]) -> Result<(), NetError> {
        while !data.is_empty() {
            let sent = self.write(data)?;
            data = &data[sent..];
        }
        Ok(())
    }

    /// Muda o tempo limite das esperas, em milissegundos (0 a 60 000).
    pub fn set_timeout(&self, ms: u32) -> Result<(), NetError> {
        check(sys::tcp_timeout(self.sock, ms)).map(|_| ())
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        sys::tcp_close(self.sock);
    }
}
