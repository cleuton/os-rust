//! A tela de texto de 25 linhas por 80 colunas: limpar, mover o cursor e
//! escolher as cores (o vídeo reverso é trocar texto e fundo).
//!
//! O kernel **não apaga nada sozinho** e não rola a tela fora da última linha:
//! quem desenha apaga o que quer apagar, escrevendo espaços. Um programa que
//! usa a tela deve rodar **sozinho**; dois programas escrevendo ao mesmo tempo
//! disputam o único cursor e misturam o desenho. Ao voltar ao prompt, o kernel
//! apaga só a última linha e restaura a cor padrão.

use core::fmt;

use crate::sys;

pub use abi::{
    COLOR_BLACK, COLOR_BLUE, COLOR_BROWN, COLOR_CYAN, COLOR_DARK_GRAY, COLOR_DEFAULT_BACKGROUND,
    COLOR_DEFAULT_TEXT, COLOR_GREEN, COLOR_LIGHT_BLUE, COLOR_LIGHT_CYAN, COLOR_LIGHT_GRAY,
    COLOR_LIGHT_GREEN, COLOR_LIGHT_RED, COLOR_MAGENTA, COLOR_PINK, COLOR_RED, COLOR_WHITE,
    COLOR_YELLOW,
};

/// Linhas da tela.
pub const ROWS: usize = abi::SCREEN_ROWS;
/// Colunas da tela.
pub const COLS: usize = abi::SCREEN_COLS;

/// Por que uma operação de tela foi recusada (e nada mudou).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenError {
    /// Linha `>= 25` ou coluna `>= 80`.
    OutOfRange,
    /// Cor do texto `> 15` ou do fundo `> 7` (o fundo não chega a 8 porque o
    /// bit mais alto do fundo faz o caractere piscar, e isso não é oferecido).
    BadColor,
}

impl fmt::Display for ScreenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScreenError::OutOfRange => f.write_str("posicao fora da tela de 25 por 80"),
            ScreenError::BadColor => f.write_str("cor invalida (texto 0 a 15, fundo 0 a 7)"),
        }
    }
}

/// Apaga as 25 linhas (com a cor atual) e leva o cursor ao canto de cima, à
/// esquerda.
pub fn clear() {
    sys::screen(abi::SCREEN_CLEAR, 0, 0);
}

/// Leva o cursor à linha `row` (`0..=24`) e à coluna `col` (`0..=79`). Fora
/// disso devolve [`ScreenError::OutOfRange`] e o cursor não se move.
pub fn move_to(row: usize, col: usize) -> Result<(), ScreenError> {
    match sys::screen(abi::SCREEN_MOVE, row as u64, col as u64) {
        0 => Ok(()),
        _ => Err(ScreenError::OutOfRange),
    }
}

/// Escolhe a cor do que for escrito daí em diante: texto `text` (`0..=15`) e
/// fundo `background` (`0..=7`), com os números do VGA (`COLOR_*`). Não muda o
/// que já está na tela. Vídeo reverso: texto escuro sobre fundo claro.
pub fn set_color(text: u8, background: u8) -> Result<(), ScreenError> {
    match sys::screen(abi::SCREEN_COLOR, text as u64, background as u64) {
        0 => Ok(()),
        _ => Err(ScreenError::BadColor),
    }
}

/// Volta ao amarelo sobre preto.
pub fn reset_color() {
    // As duas cores padrão estão sempre dentro dos limites: não há erro.
    let _ = set_color(COLOR_DEFAULT_TEXT, COLOR_DEFAULT_BACKGROUND);
}
