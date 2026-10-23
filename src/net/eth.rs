//! Quadros Ethernet II: 6 bytes de MAC de destino, 6 de origem, 2 de tipo e o
//! conteúdo. A placa acrescenta e confere o CRC; o driver só vê o resto.

/// Tamanho do cabeçalho Ethernet.
pub const HEADER_LEN: usize = 14;
/// O endereço de difusão (broadcast): todos os que estão no mesmo segmento.
pub const BROADCAST: [u8; 6] = [0xFF; 6];
/// Tipo de conteúdo: IPv4.
pub const ETHERTYPE_IPV4: u16 = 0x0800;
/// Tipo de conteúdo: ARP.
pub const ETHERTYPE_ARP: u16 = 0x0806;

/// Um quadro lido: quem recebe, quem enviou, o tipo e o conteúdo.
#[derive(Debug, PartialEq, Eq)]
pub struct Frame<'a> {
    pub dst: [u8; 6],
    pub src: [u8; 6],
    pub ethertype: u16,
    pub payload: &'a [u8],
}

/// Interpreta um quadro. `None` se não tem nem o cabeçalho.
pub fn parse(frame: &[u8]) -> Option<Frame<'_>> {
    if frame.len() < HEADER_LEN {
        return None;
    }
    let mut dst = [0u8; 6];
    let mut src = [0u8; 6];
    dst.copy_from_slice(&frame[0..6]);
    src.copy_from_slice(&frame[6..12]);
    Some(Frame {
        dst,
        src,
        ethertype: u16::from_be_bytes([frame[12], frame[13]]),
        payload: &frame[HEADER_LEN..],
    })
}

/// Escreve o cabeçalho Ethernet no começo de `out` (o conteúdo vai depois, a
/// cargo de quem chama, para não copiar duas vezes).
pub fn write_header(out: &mut [u8], dst: [u8; 6], src: [u8; 6], ethertype: u16) {
    out[0..6].copy_from_slice(&dst);
    out[6..12].copy_from_slice(&src);
    out[12..14].copy_from_slice(&ethertype.to_be_bytes());
}
