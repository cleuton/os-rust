//! Driver do relógio de tempo real (RTC), o chip CMOS que guarda a data e a
//! hora com bateria própria.
//!
//! O chip é lido por duas portas de E/S: escreve-se em `0x70` o número do
//! registrador que se quer, e lê-se o valor em `0x71`. Os registradores que
//! interessam:
//!
//! | Índice | Conteúdo |
//! |---|---|
//! | `0x00` | segundos |
//! | `0x02` | minutos |
//! | `0x04` | horas (bit 7 = PM, quando o chip está em 12 horas) |
//! | `0x07` | dia do mês |
//! | `0x08` | mês |
//! | `0x09` | ano, **só os dois últimos dígitos** |
//! | `0x32` | século (nem todo chip tem; `0` quer dizer "não sei") |
//! | `0x0A` | status A: o bit 7 (UIP) diz que o chip está atualizando os registradores |
//! | `0x0B` | status B: o bit 1 liga o modo de 24 horas e o bit 2 o modo binário |
//!
//! Quatro detalhes tornam o driver mais que um `in`:
//!
//! - **BCD ou binário.** Em BCD, cada nibble é um dígito decimal: 26 vem como
//!   `0x26`. O status B diz qual formato o chip usa; o driver trata os dois.
//! - **12 ou 24 horas.** Em 12 horas, o bit 7 da hora marca PM, e 12 AM é a
//!   meia-noite (hora 0), 12 PM é o meio-dia.
//! - **Janela de atualização.** Uma vez por segundo o chip reescreve os
//!   registradores; ler no meio dá uma data misturada (23:59:59 com o dia já
//!   incrementado, por exemplo). O driver espera o UIP baixar e lê duas vezes
//!   até as leituras coincidirem, com limite de tentativas.
//! - **Dois dígitos de ano.** Com o registrador de século, o ano é
//!   `século * 100 + ano`; sem ele, 70 a 99 viram 19xx e 00 a 69 viram 20xx.
//!
//! A leitura é por polling, sem a interrupção IRQ8 do chip e sem mexer no PIC.
//! O chip guarda a hora do hospedeiro em UTC (no QEMU); o kernel não conhece
//! fuso horário.

use abi::DateTime;
use core::fmt;
use x86_64::instructions::port::Port;

/// Porta que recebe o índice do registrador.
const PORT_INDEX: u16 = 0x70;
/// Porta de onde se lê o valor do registrador escolhido.
const PORT_DATA: u16 = 0x71;

const REG_SECOND: u8 = 0x00;
const REG_MINUTE: u8 = 0x02;
const REG_HOUR: u8 = 0x04;
const REG_DAY: u8 = 0x07;
const REG_MONTH: u8 = 0x08;
const REG_YEAR: u8 = 0x09;
const REG_CENTURY: u8 = 0x32;
const REG_STATUS_A: u8 = 0x0A;
const REG_STATUS_B: u8 = 0x0B;

/// Status A, bit 7: o chip está atualizando os registradores.
const STATUS_A_UIP: u8 = 0x80;
/// Status B, bit 1: hora em 24 horas (desligado = 12 horas).
const STATUS_B_24H: u8 = 0b10;
/// Status B, bit 2: valores em binário (desligado = BCD).
const STATUS_B_BINARY: u8 = 0b100;
/// Na hora em 12 horas, o bit 7 marca PM.
const HOUR_PM: u8 = 0x80;

/// Quantas vezes o driver tenta obter duas leituras iguais seguidas antes de
/// desistir. Cinco bastam: a janela de atualização dura menos de 2 ms e as
/// leituras levam microssegundos.
const MAX_ATTEMPTS: usize = 5;
/// Quantas vezes o driver consulta o UIP esperando que ele baixe. Existe só
/// para que um chip que nunca responde não trave o kernel.
const UIP_SPINS: u32 = 100_000;

/// Por que uma leitura não devolveu uma data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtcError {
    /// O chip devolveu valores impossíveis (mês 0, hora 31, dígito BCD maior
    /// que 9, 31 de fevereiro...).
    Invalid,
    /// Não houve duas leituras iguais em `MAX_ATTEMPTS` tentativas.
    Unstable,
    /// O UIP não baixou: o chip não responde.
    Unavailable,
}

impl fmt::Display for RtcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RtcError::Invalid => f.write_str("relogio invalido"),
            RtcError::Unstable => f.write_str("relogio instavel"),
            RtcError::Unavailable => f.write_str("relogio indisponivel"),
        }
    }
}

/// Os valores crus dos registradores, como o chip os entrega. A igualdade é a
/// comparação "duas leituras coincidem".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Regs {
    second: u8,
    minute: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u8,
    century: u8,
    status_b: u8,
}

/// Converte um valor do chip em número: em BCD, cada nibble é um dígito
/// decimal e um nibble maior que 9 é inválido.
fn from_chip(value: u8, binary: bool) -> Result<u8, RtcError> {
    if binary {
        return Ok(value);
    }
    let (high, low) = (value >> 4, value & 0x0F);
    if high > 9 || low > 9 {
        return Err(RtcError::Invalid);
    }
    Ok(high * 10 + low)
}

fn is_leap(year: u16) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Dias do mês (`month` de 1 a 12). Serve só para **rejeitar** datas que não
/// existem; a virada de dia, mês e ano continua vindo do chip.
fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(year) => 29,
        _ => 28,
    }
}

/// Transforma os registradores numa data validada. Função pura: não toca em
/// porta nenhuma, então é testada com valores fabricados.
fn decode(regs: Regs) -> Result<DateTime, RtcError> {
    let binary = regs.status_b & STATUS_B_BINARY != 0;
    let hour_24 = regs.status_b & STATUS_B_24H != 0;

    // Em 12 horas, o bit 7 é PM e não faz parte do número.
    let pm = !hour_24 && regs.hour & HOUR_PM != 0;
    let hour_raw = if hour_24 { regs.hour } else { regs.hour & !HOUR_PM };

    let second = from_chip(regs.second, binary)?;
    let minute = from_chip(regs.minute, binary)?;
    let mut hour = from_chip(hour_raw, binary)?;
    let day = from_chip(regs.day, binary)?;
    let month = from_chip(regs.month, binary)?;
    let year2 = from_chip(regs.year, binary)?;

    if !hour_24 {
        if !(1..=12).contains(&hour) {
            return Err(RtcError::Invalid);
        }
        // 12 AM é 0 h; 12 PM é 12 h; 1 PM a 11 PM somam 12.
        hour %= 12;
        if pm {
            hour += 12;
        }
    }

    // Século: se o registrador existe e é válido, vale; senão, a regra de
    // reserva (70 a 99 são 19xx, 00 a 69 são 20xx).
    let century = match from_chip(regs.century, binary) {
        Ok(c) if c != 0 => c as u16 * 100,
        _ => {
            if year2 >= 70 {
                1900
            } else {
                2000
            }
        }
    };
    let year = century + year2 as u16;

    if !(1..=12).contains(&month)
        || hour > 23
        || minute > 59
        || second > 59
        || day < 1
        || day > days_in_month(year, month)
    {
        return Err(RtcError::Invalid);
    }

    Ok(DateTime { year, month, day, hour, minute, second, _pad: 0 })
}

/// Lê até obter duas leituras iguais em sequência. `read_once` é quem lê o
/// chip (as portas, de verdade; uma fonte fabricada, nos testes). Se nunca
/// houver duas iguais, devolve `Unstable`; nunca trava.
fn read_consistent(mut read_once: impl FnMut() -> Result<Regs, RtcError>) -> Result<Regs, RtcError> {
    for _ in 0..MAX_ATTEMPTS {
        let first = read_once()?;
        let second = read_once()?;
        if first == second {
            return Ok(first);
        }
    }
    Err(RtcError::Unstable)
}

/// Lê um registrador do CMOS.
fn cmos_read(reg: u8) -> u8 {
    // SAFETY: `0x70` e `0x71` são as portas do CMOS/RTC e só este módulo as
    // usa. Escrever o índice com o bit 7 desligado (`& 0x7F`) não mexe na
    // máscara de NMI, e ler o valor não tem efeito além de ler; nenhum dos
    // dois toca em memória.
    unsafe {
        Port::<u8>::new(PORT_INDEX).write(reg & 0x7F);
        Port::<u8>::new(PORT_DATA).read()
    }
}

/// Uma passada pelos registradores, depois de esperar o fim da atualização.
fn read_regs_once() -> Result<Regs, RtcError> {
    let mut spins = 0;
    while cmos_read(REG_STATUS_A) & STATUS_A_UIP != 0 {
        spins += 1;
        if spins >= UIP_SPINS {
            return Err(RtcError::Unavailable);
        }
    }
    Ok(Regs {
        second: cmos_read(REG_SECOND),
        minute: cmos_read(REG_MINUTE),
        hour: cmos_read(REG_HOUR),
        day: cmos_read(REG_DAY),
        month: cmos_read(REG_MONTH),
        year: cmos_read(REG_YEAR),
        century: cmos_read(REG_CENTURY),
        status_b: cmos_read(REG_STATUS_B),
    })
}

/// A data e a hora atuais (UTC), ou o motivo de não haver.
///
/// A leitura inteira roda com as interrupções desligadas: o par índice/valor
/// das portas não pode ser intercalado com outra leitura, e assim dois
/// programas pedindo a hora ao mesmo tempo nunca recebem um valor misturado.
pub fn read() -> Result<DateTime, RtcError> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let regs = read_consistent(read_regs_once)?;
        decode(regs)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIN24: u8 = STATUS_B_BINARY | STATUS_B_24H;
    const BCD24: u8 = STATUS_B_24H;
    const BIN12: u8 = STATUS_B_BINARY;
    const BCD12: u8 = 0;

    fn regs(status_b: u8, hour: u8, day: u8, month: u8, year: u8, century: u8) -> Regs {
        Regs { second: 0, minute: 0, hour, day, month, year, century, status_b }
    }

    fn date(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> DateTime {
        DateTime { year, month, day, hour, minute, second, _pad: 0 }
    }

    // --- BCD x binário, 12 x 24 horas ---

    #[test_case]
    fn as_quatro_combinacoes_dao_o_mesmo_instante() {
        // 2026-10-08 15:04:05 em cada formato que o chip pode usar.
        let esperado = date(2026, 10, 8, 15, 4, 5);
        let bin24 = Regs { second: 5, minute: 4, hour: 15, day: 8, month: 10, year: 26, century: 20, status_b: BIN24 };
        let bcd24 = Regs { second: 0x05, minute: 0x04, hour: 0x15, day: 0x08, month: 0x10, year: 0x26, century: 0x20, status_b: BCD24 };
        let bin12 = Regs { second: 5, minute: 4, hour: 3 | HOUR_PM, day: 8, month: 10, year: 26, century: 20, status_b: BIN12 };
        let bcd12 = Regs { second: 0x05, minute: 0x04, hour: 0x03 | HOUR_PM, day: 0x08, month: 0x10, year: 0x26, century: 0x20, status_b: BCD12 };
        assert_eq!(decode(bin24), Ok(esperado));
        assert_eq!(decode(bcd24), Ok(esperado));
        assert_eq!(decode(bin12), Ok(esperado));
        assert_eq!(decode(bcd12), Ok(esperado));
    }

    #[test_case]
    fn doze_horas_trata_meia_noite_meio_dia_e_tarde() {
        let h = |hour| decode(regs(BIN12, hour, 8, 10, 26, 20)).map(|t| t.hour);
        assert_eq!(h(12), Ok(0)); // 12 AM
        assert_eq!(h(12 | HOUR_PM), Ok(12)); // 12 PM
        assert_eq!(h(1 | HOUR_PM), Ok(13)); // 1 PM
        assert_eq!(h(11 | HOUR_PM), Ok(23)); // 11 PM
        assert_eq!(h(1), Ok(1)); // 1 AM
    }

    #[test_case]
    fn doze_horas_recusa_hora_fora_de_1_a_12() {
        assert_eq!(decode(regs(BIN12, 0, 8, 10, 26, 20)), Err(RtcError::Invalid));
        assert_eq!(decode(regs(BIN12, 13, 8, 10, 26, 20)), Err(RtcError::Invalid));
    }

    // --- Valores impossíveis ---

    #[test_case]
    fn campos_fora_da_faixa_sao_invalidos() {
        let ok = regs(BIN24, 15, 8, 10, 26, 20);
        assert!(decode(ok).is_ok());
        assert_eq!(decode(Regs { month: 0, ..ok }), Err(RtcError::Invalid));
        assert_eq!(decode(Regs { month: 13, ..ok }), Err(RtcError::Invalid));
        assert_eq!(decode(Regs { hour: 31, ..ok }), Err(RtcError::Invalid));
        assert_eq!(decode(Regs { hour: 24, ..ok }), Err(RtcError::Invalid));
        assert_eq!(decode(Regs { minute: 60, ..ok }), Err(RtcError::Invalid));
        assert_eq!(decode(Regs { second: 60, ..ok }), Err(RtcError::Invalid));
        assert_eq!(decode(Regs { day: 0, ..ok }), Err(RtcError::Invalid));
    }

    #[test_case]
    fn nibble_bcd_maior_que_nove_e_invalido() {
        let ok = regs(BCD24, 0x15, 0x08, 0x10, 0x26, 0x20);
        assert!(decode(ok).is_ok());
        assert_eq!(decode(Regs { minute: 0x1A, ..ok }), Err(RtcError::Invalid));
        assert_eq!(decode(Regs { second: 0xA0, ..ok }), Err(RtcError::Invalid));
        assert_eq!(decode(Regs { day: 0xFF, ..ok }), Err(RtcError::Invalid));
    }

    #[test_case]
    fn dia_e_conferido_contra_o_mes_e_o_ano_bissexto() {
        let d = |day, month, year, century| decode(regs(BIN24, 0, day, month, year, century));
        assert_eq!(d(31, 2, 26, 20), Err(RtcError::Invalid));
        assert_eq!(d(30, 2, 24, 20), Err(RtcError::Invalid));
        assert_eq!(d(31, 4, 26, 20), Err(RtcError::Invalid));
        assert_eq!(d(29, 2, 25, 20), Err(RtcError::Invalid)); // 2025 nao e bissexto
        assert!(d(29, 2, 24, 20).is_ok()); // 2024 e bissexto
        assert!(d(29, 2, 0, 20).is_ok()); // 2000 e bissexto (divisivel por 400)
        assert_eq!(d(29, 2, 0, 19), Err(RtcError::Invalid)); // 1900 nao e
        assert!(d(31, 12, 26, 20).is_ok());
        assert!(d(30, 4, 26, 20).is_ok());
    }

    // --- Século ---

    #[test_case]
    fn registrador_de_seculo_compoe_o_ano() {
        assert_eq!(decode(regs(BCD24, 0, 0x01, 0x01, 0x26, 0x20)).map(|t| t.year), Ok(2026));
        assert_eq!(decode(regs(BCD24, 0, 0x01, 0x01, 0x99, 0x19)).map(|t| t.year), Ok(1999));
    }

    #[test_case]
    fn sem_seculo_vale_a_regra_de_reserva() {
        let y = |year| decode(regs(BIN24, 0, 1, 1, year, 0)).map(|t| t.year);
        assert_eq!(y(26), Ok(2026));
        assert_eq!(y(70), Ok(1970));
        assert_eq!(y(69), Ok(2069));
        assert_eq!(y(99), Ok(1999));
        assert_eq!(y(0), Ok(2000));
    }

    #[test_case]
    fn seculo_invalido_cai_na_regra_de_reserva() {
        // 0x2A nao e BCD valido: o driver nao mostra lixo, usa a regra.
        assert_eq!(decode(regs(BCD24, 0, 0x01, 0x01, 0x26, 0x2A)).map(|t| t.year), Ok(2026));
    }

    // --- Leitura consistente ---

    fn amostra(second: u8) -> Regs {
        Regs { second, minute: 4, hour: 15, day: 8, month: 10, year: 26, century: 20, status_b: BIN24 }
    }

    #[test_case]
    fn duas_leituras_iguais_na_primeira_tentativa() {
        let mut chamadas = 0;
        let r = read_consistent(|| {
            chamadas += 1;
            Ok(amostra(5))
        });
        assert_eq!(r, Ok(amostra(5)));
        assert_eq!(chamadas, 2);
    }

    #[test_case]
    fn leitura_e_repetida_enquanto_o_chip_atualiza() {
        // As duas primeiras leituras divergem (o chip virou o segundo no meio);
        // a partir da terceira o valor estabiliza.
        let mut chamadas = 0;
        let r = read_consistent(|| {
            chamadas += 1;
            Ok(amostra(if chamadas == 1 { 59 } else { 0 }))
        });
        assert_eq!(r, Ok(amostra(0)));
        assert_eq!(chamadas, 4);
    }

    #[test_case]
    fn leitura_que_nunca_estabiliza_desiste() {
        let mut chamadas = 0u8;
        let r = read_consistent(|| {
            chamadas += 1;
            Ok(amostra(chamadas))
        });
        assert_eq!(r, Err(RtcError::Unstable));
        assert_eq!(chamadas as usize, MAX_ATTEMPTS * 2);
    }

    #[test_case]
    fn erro_da_fonte_e_propagado() {
        let r = read_consistent(|| Err(RtcError::Unavailable));
        assert_eq!(r, Err(RtcError::Unavailable));
    }
}
