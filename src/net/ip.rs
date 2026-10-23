//! IPv4: o endereçamento e o roteamento. Cabeçalho de 20 bytes, sem opções,
//! sem fragmentação (bit `DF` ligado), TTL 64.

use super::checksum;

/// Cabeçalho IPv4 sem opções.
pub const HEADER_LEN: usize = 20;
/// Protocolos que a pilha entende.
pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
/// TTL de todo pacote que a pilha envia.
pub const TTL: u8 = 64;
/// MTU: o maior pacote IP que a pilha envia.
pub const MTU: usize = 1500;

/// Por que um pacote recebido foi recusado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpError {
    TooShort,
    BadVersion,
    BadHeaderLength,
    BadTotalLength,
    BadChecksum,
    Fragment,
}

/// Um pacote IPv4 válido: origem, destino, protocolo e o conteúdo.
#[derive(Debug, PartialEq, Eq)]
pub struct Packet<'a> {
    pub src: [u8; 4],
    pub dst: [u8; 4],
    pub protocol: u8,
    pub payload: &'a [u8],
}

/// Interpreta um pacote IPv4 e confere, nesta ordem: tamanho mínimo, versão,
/// `IHL`, comprimento total coerente com os bytes disponíveis, soma do
/// cabeçalho e ausência de fragmentação (`MF` ou deslocamento diferente de
/// zero). Bytes além do comprimento total (preenchimento do Ethernet) são
/// cortados. O destino é conferido por quem chama.
pub fn parse(bytes: &[u8]) -> Result<Packet<'_>, IpError> {
    if bytes.len() < HEADER_LEN {
        return Err(IpError::TooShort);
    }
    if bytes[0] >> 4 != 4 {
        return Err(IpError::BadVersion);
    }
    let header_len = usize::from(bytes[0] & 0x0F) * 4;
    if header_len < HEADER_LEN || header_len > bytes.len() {
        return Err(IpError::BadHeaderLength);
    }
    let total = usize::from(u16::from_be_bytes([bytes[2], bytes[3]]));
    if total < header_len || total > bytes.len() {
        return Err(IpError::BadTotalLength);
    }
    if !checksum::verify(&[&bytes[..header_len]]) {
        return Err(IpError::BadChecksum);
    }
    let flags_and_offset = u16::from_be_bytes([bytes[6], bytes[7]]);
    if flags_and_offset & 0x2000 != 0 || flags_and_offset & 0x1FFF != 0 {
        return Err(IpError::Fragment);
    }
    let mut src = [0u8; 4];
    let mut dst = [0u8; 4];
    src.copy_from_slice(&bytes[12..16]);
    dst.copy_from_slice(&bytes[16..20]);
    Ok(Packet { src, dst, protocol: bytes[9], payload: &bytes[header_len..total] })
}

/// Escreve o cabeçalho IPv4 de um pacote com `payload_len` bytes de conteúdo
/// nos primeiros `HEADER_LEN` bytes de `out`.
pub fn write_header(
    out: &mut [u8],
    src: [u8; 4],
    dst: [u8; 4],
    protocol: u8,
    id: u16,
    payload_len: usize,
) {
    let total = (HEADER_LEN + payload_len) as u16;
    out[0] = 0x45;
    out[1] = 0;
    out[2..4].copy_from_slice(&total.to_be_bytes());
    out[4..6].copy_from_slice(&id.to_be_bytes());
    out[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // DF
    out[8] = TTL;
    out[9] = protocol;
    out[10..12].copy_from_slice(&[0, 0]);
    out[12..16].copy_from_slice(&src);
    out[16..20].copy_from_slice(&dst);
    let sum = checksum::sum(&[&out[..HEADER_LEN]]);
    out[10..12].copy_from_slice(&sum.to_be_bytes());
}

/// Para onde um pacote vai.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// `127.0.0.0/8`: volta para a própria pilha.
    Loopback,
    /// Na mesma sub-rede: direto ao dono.
    Direct,
    /// Fora da sub-rede: ao gateway.
    Gateway,
}

pub fn is_loopback(ip: [u8; 4]) -> bool {
    ip[0] == 127
}

pub fn in_subnet(ip: [u8; 4], local: [u8; 4], mask: [u8; 4]) -> bool {
    (0..4).all(|i| ip[i] & mask[i] == local[i] & mask[i])
}

pub fn route(dst: [u8; 4], local: [u8; 4], mask: [u8; 4]) -> Route {
    if is_loopback(dst) {
        Route::Loopback
    } else if in_subnet(dst, local, mask) {
        Route::Direct
    } else {
        Route::Gateway
    }
}

/// O endereço de difusão da sub-rede (`ip | !mascara`).
pub fn subnet_broadcast(local: [u8; 4], mask: [u8; 4]) -> [u8; 4] {
    [local[0] | !mask[0], local[1] | !mask[1], local[2] | !mask[2], local[3] | !mask[3]]
}

/// A fila do loopback: 8 pacotes IP de até `MTU` bytes (sem Ethernet).
pub const LOOP_SLOTS: usize = 8;

pub struct LoopQueue {
    packets: [[u8; MTU]; LOOP_SLOTS],
    lens: [usize; LOOP_SLOTS],
    head: usize,
    count: usize,
}

impl LoopQueue {
    pub const fn new() -> LoopQueue {
        LoopQueue { packets: [[0; MTU]; LOOP_SLOTS], lens: [0; LOOP_SLOTS], head: 0, count: 0 }
    }

    /// Enfileira um pacote. `false` (e o pacote é descartado) se a fila está cheia.
    pub fn push(&mut self, packet: &[u8]) -> bool {
        if self.count == LOOP_SLOTS || packet.len() > MTU {
            return false;
        }
        let slot = (self.head + self.count) % LOOP_SLOTS;
        self.packets[slot][..packet.len()].copy_from_slice(packet);
        self.lens[slot] = packet.len();
        self.count += 1;
        true
    }

    /// Tira o pacote mais antigo, copiando-o para `out`. Devolve o tamanho.
    pub fn pop(&mut self, out: &mut [u8]) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        let len = self.lens[self.head];
        out[..len].copy_from_slice(&self.packets[self.head][..len]);
        self.head = (self.head + 1) % LOOP_SLOTS;
        self.count -= 1;
        Some(len)
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.count = 0;
    }
}

impl Default for LoopQueue {
    fn default() -> Self {
        LoopQueue::new()
    }
}
