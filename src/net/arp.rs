//! ARP: descobrir o MAC de um IP na mesma rede.
//!
//! Para mandar um pacote IP a alguém na rede local, a pilha precisa do MAC
//! dessa pessoa. Pergunta-se a todos (difusão): "quem tem o IP X?", e o dono
//! responde com o MAC dele. As respostas ficam num cache pequeno (8 entradas;
//! quando enche, sai a mais antiga).

/// Tamanho do pacote ARP de Ethernet/IPv4.
pub const PACKET_LEN: usize = 28;
/// Entradas do cache.
pub const CACHE_SIZE: usize = 8;
/// Operação: pedido ("quem tem?").
pub const OP_REQUEST: u16 = 1;
/// Operação: resposta ("eu tenho, meu MAC é...").
pub const OP_REPLY: u16 = 2;

/// Um pacote ARP, com os campos que importam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet {
    pub op: u16,
    /// MAC e IP de quem enviou.
    pub sender_mac: [u8; 6],
    pub sender_ip: [u8; 4],
    /// MAC e IP de quem se procura (zerado num pedido).
    pub target_mac: [u8; 6],
    pub target_ip: [u8; 4],
}

/// Interpreta um pacote ARP de Ethernet/IPv4 (hardware 1, protocolo 0x0800,
/// tamanhos 6 e 4). Qualquer outro formato é `None`.
pub fn parse(bytes: &[u8]) -> Option<Packet> {
    if bytes.len() < PACKET_LEN {
        return None;
    }
    if bytes[0..2] != [0, 1] || bytes[2..4] != [0x08, 0x00] || bytes[4] != 6 || bytes[5] != 4 {
        return None;
    }
    let op = u16::from_be_bytes([bytes[6], bytes[7]]);
    if op != OP_REQUEST && op != OP_REPLY {
        return None;
    }
    let mut packet = Packet {
        op,
        sender_mac: [0; 6],
        sender_ip: [0; 4],
        target_mac: [0; 6],
        target_ip: [0; 4],
    };
    packet.sender_mac.copy_from_slice(&bytes[8..14]);
    packet.sender_ip.copy_from_slice(&bytes[14..18]);
    packet.target_mac.copy_from_slice(&bytes[18..24]);
    packet.target_ip.copy_from_slice(&bytes[24..28]);
    Some(packet)
}

/// Escreve o pacote em `out` (pelo menos `PACKET_LEN` bytes).
pub fn write(packet: &Packet, out: &mut [u8]) {
    out[0..4].copy_from_slice(&[0, 1, 0x08, 0x00]);
    out[4] = 6;
    out[5] = 4;
    out[6..8].copy_from_slice(&packet.op.to_be_bytes());
    out[8..14].copy_from_slice(&packet.sender_mac);
    out[14..18].copy_from_slice(&packet.sender_ip);
    out[18..24].copy_from_slice(&packet.target_mac);
    out[24..28].copy_from_slice(&packet.target_ip);
}

/// O cache: IP, MAC e a "idade" (um contador que só cresce; a menor sai).
#[derive(Clone, Copy)]
pub struct Cache {
    entries: [Option<([u8; 4], [u8; 6], u32)>; CACHE_SIZE],
    next_age: u32,
}

impl Cache {
    pub const fn new() -> Cache {
        Cache { entries: [None; CACHE_SIZE], next_age: 0 }
    }

    /// Guarda (ou atualiza) o MAC de um IP. Com o cache cheio, substitui a
    /// entrada mais antiga.
    pub fn learn(&mut self, ip: [u8; 4], mac: [u8; 6]) {
        self.next_age = self.next_age.wrapping_add(1);
        let age = self.next_age;
        if let Some(slot) = self.entries.iter_mut().find(|e| matches!(e, Some((i, _, _)) if *i == ip)) {
            *slot = Some((ip, mac, age));
            return;
        }
        if let Some(slot) = self.entries.iter_mut().find(|e| e.is_none()) {
            *slot = Some((ip, mac, age));
            return;
        }
        // Cheio: troca a de menor idade (a mais antiga).
        let oldest = (0..CACHE_SIZE)
            .min_by_key(|&i| self.entries[i].map_or(u32::MAX, |(_, _, a)| a))
            .unwrap_or(0);
        self.entries[oldest] = Some((ip, mac, age));
    }

    pub fn lookup(&self, ip: [u8; 4]) -> Option<[u8; 6]> {
        self.entries.iter().flatten().find(|(i, _, _)| *i == ip).map(|(_, mac, _)| *mac)
    }

    pub fn clear(&mut self) {
        self.entries = [None; CACHE_SIZE];
    }

    /// Quantas entradas estão em uso (para os testes).
    pub fn len(&self) -> usize {
        self.entries.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for Cache {
    fn default() -> Self {
        Cache::new()
    }
}
