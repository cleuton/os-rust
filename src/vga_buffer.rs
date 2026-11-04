//! Escrita de texto no buffer de vídeo VGA (modo texto, 0xb8000).

use core::fmt;
use lazy_static::lazy_static;
use spin::Mutex;
use volatile::Volatile;

/// As 16 cores fixas do hardware VGA em modo texto.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

/// Um byte de cor VGA: 4 bits de primeiro plano + 4 bits de fundo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct ColorCode(u8);

impl ColorCode {
    fn new(foreground: Color, background: Color) -> ColorCode {
        ColorCode((background as u8) << 4 | (foreground as u8))
    }
}

/// Uma posição da grade de texto: caractere ASCII + cor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct ScreenChar {
    ascii_character: u8,
    color_code: ColorCode,
}

const BUFFER_HEIGHT: usize = 25;
const BUFFER_WIDTH: usize = 80;

/// O hardware de texto VGA mapeado em memória, em 0xb8000.
#[repr(transparent)]
struct Buffer {
    chars: [[Volatile<ScreenChar>; BUFFER_WIDTH]; BUFFER_HEIGHT],
}

/// Escreve texto na tela, controlando cursor lógico, avanço de linha e
/// rolagem simples quando o conteúdo ultrapassa a altura da tela.
pub struct Writer {
    column_position: usize,
    color_code: ColorCode,
    buffer: &'static mut Buffer,
}

impl Writer {
    fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            byte => {
                if self.column_position >= BUFFER_WIDTH {
                    self.new_line();
                }

                let row = BUFFER_HEIGHT - 1;
                let col = self.column_position;
                let color_code = self.color_code;
                self.buffer.chars[row][col].write(ScreenChar {
                    ascii_character: byte,
                    color_code,
                });
                self.column_position += 1;
            }
        }
    }

    fn write_string(&mut self, s: &str) {
        for byte in s.bytes() {
            match byte {
                // ASCII imprimível ou nova linha: escreve como está.
                0x20..=0x7e | b'\n' => self.write_byte(byte),
                // Qualquer outro byte (fora do ASCII imprimível) vira um
                // quadrado de "caractere desconhecido" (0xfe no code page
                // padrão do modo texto VGA) em vez de corromper a tela.
                _ => self.write_byte(0xfe),
            }
        }
    }

    /// Avança para a próxima linha; se já estava na última linha visível,
    /// rola todo o conteúdo uma linha para cima (rolagem simples) em vez
    /// de travar ou sobrescrever de forma ilegível.
    fn new_line(&mut self) {
        for row in 1..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                let character = self.buffer.chars[row][col].read();
                self.buffer.chars[row - 1][col].write(character);
            }
        }
        self.clear_row(BUFFER_HEIGHT - 1);
        self.column_position = 0;
    }

    fn clear_row(&mut self, row: usize) {
        let blank = ScreenChar {
            ascii_character: b' ',
            color_code: self.color_code,
        };
        for col in 0..BUFFER_WIDTH {
            self.buffer.chars[row][col].write(blank);
        }
    }

    /// Limpa a tela inteira e volta o cursor lógico para o início.
    fn clear_screen(&mut self) {
        for row in 0..BUFFER_HEIGHT {
            self.clear_row(row);
        }
        self.column_position = 0;
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

lazy_static! {
    /// O `Writer` global usado por `print!`/`println!` em todo o kernel,
    /// incluindo o handler de panic.
    pub static ref WRITER: Mutex<Writer> = Mutex::new(Writer {
        column_position: 0,
        color_code: ColorCode::new(Color::Yellow, Color::Black),
        buffer: unsafe {
            // SAFETY: 0xb8000 é o endereço físico fixo do buffer de texto
            // VGA neste target (constituição do projeto); nenhum outro
            // código do kernel acessa essa região de memória, então uma
            // única referência `&'static mut` para ela é válida durante
            // toda a execução do programa.
            &mut *(0xb8000 as *mut Buffer)
        },
    });
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::vga_buffer::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    // `Writer::write_str` (acima) sempre retorna `Ok(())` — nunca há um
    // caminho de erro real a tratar, então este `unwrap` nunca entra em
    // pânico; ele existe só porque `write_fmt` retorna `fmt::Result`.
    WRITER.lock().write_fmt(args).unwrap();
}

/// Limpa a tela do buffer VGA global (chamado uma vez, no boot).
pub fn clear_screen() {
    WRITER.lock().clear_screen();
}
