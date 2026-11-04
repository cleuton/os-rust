//! Tradução de Scan Code Set 1 para ASCII, layout US QWERTY.

use spin::Mutex;

const LEFT_SHIFT_MAKE: u8 = 0x2A;
const LEFT_SHIFT_BREAK: u8 = 0xAA;
const RIGHT_SHIFT_MAKE: u8 = 0x36;
const RIGHT_SHIFT_BREAK: u8 = 0xB6;

/// Estado de pressionado/solto das duas teclas Shift, rastreadas de forma
/// independente: soltar uma não deve afetar o efeito da outra, se ainda
/// estiver pressionada.
struct ShiftState {
    left_held: bool,
    right_held: bool,
}

impl ShiftState {
    const fn new() -> Self {
        ShiftState {
            left_held: false,
            right_held: false,
        }
    }

    fn is_active(&self) -> bool {
        self.left_held || self.right_held
    }
}

static SHIFT: Mutex<ShiftState> = Mutex::new(ShiftState::new());

/// Traduz um scancode do Scan Code Set 1 para um byte ASCII (letra, dígito,
/// espaço, Enter, Backspace ou pontuação básica), conforme o estado atual
/// de Shift.
///
/// Retorna `None` — sem que isso seja um erro — para: *break codes* de
/// qualquer tecla exceto Shift (bit 7 ligado); o prefixo de tecla
/// estendida `0xE0` (que também tem o bit 7 ligado, então cai no mesmo
/// caso); e qualquer scancode fora da tabela base. Nenhuma tecla
/// estendida relevante (setas, Home/End, teclado numérico) tem seu
/// segundo byte colidindo com uma tecla mapeada de forma que produza um
/// caractere incorreto — na pior hipótese (ex.: Numpad Enter, Numpad `/`)
/// o resultado coincide com o caractere que a tecla "normal" equivalente
/// já produziria, o que é inofensivo.
pub fn translate(scancode: u8) -> Option<u8> {
    match scancode {
        LEFT_SHIFT_MAKE => {
            SHIFT.lock().left_held = true;
            None
        }
        LEFT_SHIFT_BREAK => {
            SHIFT.lock().left_held = false;
            None
        }
        RIGHT_SHIFT_MAKE => {
            SHIFT.lock().right_held = true;
            None
        }
        RIGHT_SHIFT_BREAK => {
            SHIFT.lock().right_held = false;
            None
        }
        // Break code de qualquer outra tecla (bit 7 ligado) — inclui
        // também o prefixo de tecla estendida 0xE0, que cai no mesmo teste.
        code if code & 0x80 != 0 => None,
        code => {
            let shift = SHIFT.lock().is_active();
            ascii_for_make_code(code, shift)
        }
    }
}

/// Tabela base do Scan Code Set 1 (US QWERTY): cada entrada é
/// (minúscula/sem-Shift, maiúscula/com-Shift). Cobre letras, dígitos,
/// pontuação da linha de números e das teclas ao lado das letras, espaço,
/// Enter e Backspace — exatamente o conjunto exigido pela spec.
fn ascii_for_make_code(code: u8, shift: bool) -> Option<u8> {
    let (lower, upper) = match code {
        0x0E => return Some(0x08), // Backspace
        0x1C => return Some(b'\n'), // Enter
        0x39 => return Some(b' '), // Espaço

        0x02 => (b'1', b'!'),
        0x03 => (b'2', b'@'),
        0x04 => (b'3', b'#'),
        0x05 => (b'4', b'$'),
        0x06 => (b'5', b'%'),
        0x07 => (b'6', b'^'),
        0x08 => (b'7', b'&'),
        0x09 => (b'8', b'*'),
        0x0A => (b'9', b'('),
        0x0B => (b'0', b')'),
        0x0C => (b'-', b'_'),
        0x0D => (b'=', b'+'),

        0x10 => (b'q', b'Q'),
        0x11 => (b'w', b'W'),
        0x12 => (b'e', b'E'),
        0x13 => (b'r', b'R'),
        0x14 => (b't', b'T'),
        0x15 => (b'y', b'Y'),
        0x16 => (b'u', b'U'),
        0x17 => (b'i', b'I'),
        0x18 => (b'o', b'O'),
        0x19 => (b'p', b'P'),
        0x1A => (b'[', b'{'),
        0x1B => (b']', b'}'),

        0x1E => (b'a', b'A'),
        0x1F => (b's', b'S'),
        0x20 => (b'd', b'D'),
        0x21 => (b'f', b'F'),
        0x22 => (b'g', b'G'),
        0x23 => (b'h', b'H'),
        0x24 => (b'j', b'J'),
        0x25 => (b'k', b'K'),
        0x26 => (b'l', b'L'),
        0x27 => (b';', b':'),
        0x28 => (b'\'', b'"'),
        0x29 => (b'`', b'~'),
        0x2B => (b'\\', b'|'),

        0x2C => (b'z', b'Z'),
        0x2D => (b'x', b'X'),
        0x2E => (b'c', b'C'),
        0x2F => (b'v', b'V'),
        0x30 => (b'b', b'B'),
        0x31 => (b'n', b'N'),
        0x32 => (b'm', b'M'),
        0x33 => (b',', b'<'),
        0x34 => (b'.', b'>'),
        0x35 => (b'/', b'?'),

        _ => return None,
    };
    Some(if shift { upper } else { lower })
}
