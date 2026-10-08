//! A data e a hora, em UTC.
//!
//! Um programa nunca fala com o chip do relógio: ele pede ao kernel, pela
//! syscall `SYS_TIME` (`SYSCALLS.md`, seção 5). [`now`] esconde a
//! syscall e devolve um [`DateTime`], que se escreve com `{}` no formato
//! `AAAA-MM-DD HH:MM:SS`.

use core::fmt;

use abi::{ERR_FAULT, ERR_INVAL};

pub use abi::DateTime;

use crate::sys;

/// Por que o programa não recebeu a hora.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeError {
    /// O relógio devolveu valores impossíveis, não se estabilizou ou não
    /// respondeu (`ERR_CLOCK`).
    Clock,
    /// Ponteiro inválido (não acontece com a função segura abaixo).
    Fault,
    /// Tamanho de buffer inválido (idem).
    Invalid,
}

impl TimeError {
    /// Traduz o código de erro (`< 0`) de `SYS_TIME`. Um código que o contrato
    /// não conhece (de uma versão futura) vira [`TimeError::Clock`], em vez de
    /// derrubar o programa.
    pub fn from_code(code: i64) -> TimeError {
        match code {
            ERR_FAULT => TimeError::Fault,
            ERR_INVAL => TimeError::Invalid,
            _ => TimeError::Clock,
        }
    }
}

impl fmt::Display for TimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TimeError::Clock => f.write_str("relogio invalido"),
            TimeError::Fault => f.write_str("ponteiro invalido"),
            TimeError::Invalid => f.write_str("tamanho invalido"),
        }
    }
}

/// A data e a hora atuais, em UTC.
pub fn now() -> Result<DateTime, TimeError> {
    let mut time = DateTime { year: 0, month: 0, day: 0, hour: 0, minute: 0, second: 0, _pad: 0 };
    match sys::time(&mut time) {
        0 => Ok(time),
        code => Err(TimeError::from_code(code)),
    }
}
