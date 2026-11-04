//! Escrita de texto no buffer de vídeo VGA (modo texto, 0xb8000).

use core::fmt;
use lazy_static::lazy_static;
use spin::Mutex;
use volatile::Volatile;
use x86_64::instructions::port::Port;

/// Portas de índice/dado do controlador CRTC do VGA, usadas para mover o
/// cursor de hardware do modo texto.
const CRTC_INDEX_PORT: u16 = 0x3D4;
const CRTC_DATA_PORT: u16 = 0x3D5;
/// Índices dos registradores do CRTC que guardam a posição do cursor
/// (16 bits, partido em byte alto e byte baixo).
const CURSOR_LOCATION_HIGH: u8 = 0x0E;
const CURSOR_LOCATION_LOW: u8 = 0x0F;

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
        self.move_hardware_cursor();
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
        self.move_hardware_cursor();
    }

    /// Remove o último caractere escrito, apagando-o da tela e voltando o
    /// cursor lógico uma posição. Sem efeito quando a linha já está vazia
    /// (não apaga o prompt).
    fn backspace(&mut self) {
        if self.column_position == 0 {
            return;
        }
        self.column_position -= 1;

        let row = BUFFER_HEIGHT - 1;
        let col = self.column_position;
        let blank = ScreenChar {
            ascii_character: b' ',
            color_code: self.color_code,
        };
        self.buffer.chars[row][col].write(blank);
        self.move_hardware_cursor();
    }

    /// Move o cursor de hardware do modo texto VGA para acompanhar a
    /// posição atual de escrita — sempre na última linha visível, já que
    /// este `Writer` só escreve ali e rola a tela ao quebrar linha.
    fn move_hardware_cursor(&self) {
        let position = (BUFFER_HEIGHT - 1) * BUFFER_WIDTH + self.column_position;

        let mut index_port: Port<u8> = Port::new(CRTC_INDEX_PORT);
        let mut data_port: Port<u8> = Port::new(CRTC_DATA_PORT);

        // SAFETY: 0x3D4/0x3D5 são as portas fixas de índice/dado do CRTC do
        // VGA; os índices 0x0E/0x0F selecionam, respectivamente, o byte
        // alto e o baixo do registrador de posição do cursor (um valor de
        // 16 bits sem nenhum outro efeito colateral no hardware).
        unsafe {
            index_port.write(CURSOR_LOCATION_HIGH);
            data_port.write(((position >> 8) & 0xff) as u8);
            index_port.write(CURSOR_LOCATION_LOW);
            data_port.write((position & 0xff) as u8);
        }
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

/// Apaga o último caractere escrito no `Writer` VGA global (Backspace).
pub fn backspace() {
    WRITER.lock().backspace();
}

/// Verdadeiro se `needle` aparece em alguma linha da tela, em sequência
/// (sem quebra de linha no meio). Usada só por testes de outros módulos
/// (ex.: `shell.rs`) para verificar o efeito observável de um comando
/// sem precisar prever a linha exata onde o texto termina depois da
/// rolagem.
#[cfg(test)]
pub(crate) fn screen_contains(needle: &str) -> bool {
    let needle = needle.as_bytes();
    if needle.is_empty() || needle.len() > BUFFER_WIDTH {
        return false;
    }
    let writer = WRITER.lock();
    for row in 0..BUFFER_HEIGHT {
        'window: for start in 0..=(BUFFER_WIDTH - needle.len()) {
            for (i, &b) in needle.iter().enumerate() {
                if writer.buffer.chars[row][start + i].read().ascii_character != b {
                    continue 'window;
                }
            }
            return true;
        }
    }
    false
}

/// Verdadeiro se a tela inteira está em branco (todas as posições com
/// espaço). Usada só por testes de outros módulos para confirmar o
/// efeito do comando `clear`.
#[cfg(test)]
pub(crate) fn screen_is_blank() -> bool {
    let writer = WRITER.lock();
    for row in 0..BUFFER_HEIGHT {
        for col in 0..BUFFER_WIDTH {
            if writer.buffer.chars[row][col].read().ascii_character != b' ' {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn escreve_caractere_simples() {
        clear_screen();
        crate::print!("A");
        let ch = WRITER.lock().buffer.chars[BUFFER_HEIGHT - 1][0].read();
        assert_eq!(ch.ascii_character, b'A');
    }

    #[test_case]
    fn quebra_de_linha() {
        clear_screen();
        // O primeiro "\n" garante uma linha 24 limpa antes de "cd", já
        // que `write_byte` sempre escreve na última linha visível: a
        // quebra de linha rola o conteúdo anterior ("ab") para a linha
        // 23 e reseta a coluna, então "cd" começa do zero na linha 24.
        crate::print!("ab\ncd");
        let writer = WRITER.lock();
        assert_eq!(
            writer.buffer.chars[BUFFER_HEIGHT - 1][0].read().ascii_character,
            b'c'
        );
        assert_eq!(
            writer.buffer.chars[BUFFER_HEIGHT - 1][1].read().ascii_character,
            b'd'
        );
    }

    #[test_case]
    fn rolagem_ao_ultrapassar_a_altura_da_tela() {
        clear_screen();
        crate::print!("\nPRIMEIRA\nSEGUNDA");
        let writer = WRITER.lock();
        for (i, c) in "PRIMEIRA".bytes().enumerate() {
            assert_eq!(
                writer.buffer.chars[BUFFER_HEIGHT - 2][i].read().ascii_character,
                c
            );
        }
        for (i, c) in "SEGUNDA".bytes().enumerate() {
            assert_eq!(
                writer.buffer.chars[BUFFER_HEIGHT - 1][i].read().ascii_character,
                c
            );
        }
    }

    #[test_case]
    fn apaga_ultimo_caractere_com_backspace() {
        clear_screen();
        crate::print!("AB");
        backspace();
        let writer = WRITER.lock();
        assert_eq!(
            writer.buffer.chars[BUFFER_HEIGHT - 1][0].read().ascii_character,
            b'A'
        );
        assert_eq!(
            writer.buffer.chars[BUFFER_HEIGHT - 1][1].read().ascii_character,
            b' '
        );
    }
}
