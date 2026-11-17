//! Teclas cruas: o programa espera uma tecla qualquer, sem esperar o Enter e
//! sem eco (`SYS_READ_KEY`). Setas, Home, End, Page Up, Page Down, Delete,
//! Insert e Ctrl chegam como qualquer outra tecla.

use crate::sys;

/// Uma tecla. As de texto chegam como `Char` (já com o Shift aplicado, layout
/// US); Enter, Backspace, Tab e Esc têm nome próprio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Um caractere imprimível (`' '` a `'~'`).
    Char(char),
    Enter,
    Backspace,
    Tab,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Insert,
}

/// Uma tecla pressionada e se Ctrl estava apertado. Com Ctrl, uma letra chega
/// sempre minúscula (`Ctrl+Q` é `Char('q')` com `ctrl: true`), de modo que
/// `Ctrl+J` e `Enter`, ou `Ctrl+H` e `Backspace`, nunca se confundem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub ctrl: bool,
}

/// Espera uma tecla ser **pressionada** (soltar não conta) e a devolve.
///
/// O programa fica bloqueado, sem gastar CPU, até a tecla chegar. O kernel
/// **não ecoa** nada na tela: quem chama decide o que mostrar. Teclas
/// digitadas antes do pedido chegam primeiro, na ordem. `Ctrl+C` não
/// interrompe o programa: é só mais uma tecla.
pub fn read_key() -> KeyEvent {
    loop {
        let code = sys::read_key();
        // O contrato garante um código `>= 1` e dentro da tabela; um valor fora
        // dela (impossível) é ignorado e a leitura se repete, em vez de
        // quebrar o programa.
        let Ok(code) = u32::try_from(code) else { continue };
        let ctrl = code & abi::KEY_CTRL != 0;
        let base = code & !abi::KEY_CTRL;
        let key = match base {
            0x0A => Key::Enter,
            0x08 => Key::Backspace,
            0x09 => Key::Tab,
            0x1B => Key::Esc,
            0x20..=0x7E => Key::Char(base as u8 as char),
            abi::KEY_UP => Key::Up,
            abi::KEY_DOWN => Key::Down,
            abi::KEY_LEFT => Key::Left,
            abi::KEY_RIGHT => Key::Right,
            abi::KEY_HOME => Key::Home,
            abi::KEY_END => Key::End,
            abi::KEY_PAGE_UP => Key::PageUp,
            abi::KEY_PAGE_DOWN => Key::PageDown,
            abi::KEY_DELETE => Key::Delete,
            abi::KEY_INSERT => Key::Insert,
            _ => continue,
        };
        return KeyEvent { key, ctrl };
    }
}
