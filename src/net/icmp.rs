//! ICMP, só o eco: o `ping` pergunta (tipo 8) e quem recebe responde (tipo 0)
//! devolvendo os mesmos dados. O identificador e o número de sequência deixam
//! quem perguntou casar cada resposta com o seu pedido.

use super::checksum;

/// Tipo: resposta de eco.
pub const ECHO_REPLY: u8 = 0;
/// Tipo: pedido de eco.
pub const ECHO_REQUEST: u8 = 8;
/// Cabeçalho do eco: tipo, código, soma, identificador, sequência.
pub const HEADER_LEN: usize = 8;

/// Uma mensagem de eco interpretada.
#[derive(Debug, PartialEq, Eq)]
pub struct Echo<'a> {
    pub kind: u8,
    pub id: u16,
    pub seq: u16,
    pub data: &'a [u8],
}

/// Interpreta uma mensagem ICMP. `None` se a soma está errada, se é curta ou se
/// o tipo não é eco (os demais tipos são ignorados).
pub fn parse(bytes: &[u8]) -> Option<Echo<'_>> {
    if bytes.len() < HEADER_LEN || !checksum::verify(&[bytes]) {
        return None;
    }
    let kind = bytes[0];
    if (kind != ECHO_REQUEST && kind != ECHO_REPLY) || bytes[1] != 0 {
        return None;
    }
    Some(Echo {
        kind,
        id: u16::from_be_bytes([bytes[4], bytes[5]]),
        seq: u16::from_be_bytes([bytes[6], bytes[7]]),
        data: &bytes[HEADER_LEN..],
    })
}

/// Escreve uma mensagem de eco em `out` e devolve o tamanho.
pub fn write_echo(out: &mut [u8], kind: u8, id: u16, seq: u16, data: &[u8]) -> usize {
    out[0] = kind;
    out[1] = 0;
    out[2..4].copy_from_slice(&[0, 0]);
    out[4..6].copy_from_slice(&id.to_be_bytes());
    out[6..8].copy_from_slice(&seq.to_be_bytes());
    out[HEADER_LEN..HEADER_LEN + data.len()].copy_from_slice(data);
    let len = HEADER_LEN + data.len();
    let sum = checksum::sum(&[&out[..len]]);
    out[2..4].copy_from_slice(&sum.to_be_bytes());
    len
}
