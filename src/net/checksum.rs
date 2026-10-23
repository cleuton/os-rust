//! A soma de verificação da Internet (RFC 1071), usada por IPv4, ICMP e TCP.
//!
//! Soma-se o dado como palavras de 16 bits em *big endian* (um byte sobrando no
//! fim vale como se tivesse um zero depois), dobra-se o "vai-um" de volta nos 16
//! bits baixos e inverte-se o resultado. Para conferir um pacote, soma-se tudo
//! **incluindo** o campo de soma já preenchido: o resultado, antes de inverter,
//! tem de ser `0xFFFF` (ou seja, `sum(..) == 0` depois de invertido).
//!
//! A função aceita vários pedaços (o pseudocabeçalho do TCP, o cabeçalho e os
//! dados) sem exigir que fiquem juntos na memória.

/// A soma bruta de 32 bits, sem dobrar o vai-um. `odd` guarda se o pedaço
/// anterior terminou em byte solto, para que os pedaços se encaixem como se
/// fossem um só.
fn raw(chunks: &[&[u8]]) -> u32 {
    let mut total: u32 = 0;
    let mut pending: Option<u8> = None;
    for chunk in chunks {
        for &byte in chunk.iter() {
            match pending.take() {
                Some(high) => total += (u32::from(high) << 8) | u32::from(byte),
                None => pending = Some(byte),
            }
        }
    }
    if let Some(high) = pending {
        total += u32::from(high) << 8;
    }
    total
}

/// A soma de verificação (já invertida) dos pedaços, pronta para gravar no
/// campo de soma do cabeçalho com o campo zerado. Para conferir um pacote
/// recebido, o resultado sobre o pacote inteiro tem de ser `0`.
pub fn sum(chunks: &[&[u8]]) -> u16 {
    let mut total = raw(chunks);
    while total > 0xFFFF {
        total = (total & 0xFFFF) + (total >> 16);
    }
    !(total as u16)
}

/// Verdadeiro se a soma de verificação de um pacote recebido está correta.
pub fn verify(chunks: &[&[u8]]) -> bool {
    sum(chunks) == 0
}
