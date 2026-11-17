//! Escrita de texto no buffer de vídeo VGA (modo texto, 0xb8000).

use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};
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
///
/// O cursor tem linha e coluna. A linha começa na **última** (como sempre foi)
/// e só sai de lá quando um programa usa `SYS_SCREEN`: enquanto estiver na
/// última, o `Writer` se comporta exatamente como antes (escreve ali e rola a
/// tela ao quebrar a linha). Acima da última, `\n` e a quebra de coluna só
/// descem o cursor, sem rolar nada.
pub struct Writer {
    row_position: usize,
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

                let row = self.row_position;
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
        self.write_bytes(s.as_bytes());
    }

    /// Escreve bytes crus na tela, com a mesma regra de `write_string`:
    /// ASCII imprimível e `\n` aparecem como são, qualquer outro byte vira
    /// o quadrado `0xfe`. Existe porque a syscall `write` (Marco 5) recebe
    /// de um programa de usuário bytes que não são necessariamente UTF-8, e
    /// por isso não podem passar por um `&str`.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
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
        // Acima da última linha o kernel não rola nem apaga nada: só desce o
        // cursor. Quem desenha apaga o que quer apagar.
        if self.row_position < BUFFER_HEIGHT - 1 {
            self.row_position += 1;
            self.column_position = 0;
            return;
        }
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

    /// Limpa a tela inteira e volta o cursor lógico para o início **da última
    /// linha** (o prompt no pé). Sem efeito visível a mais: o cursor já estava
    /// lá, a não ser depois de um programa que usou `SYS_SCREEN`.
    fn clear_screen(&mut self) {
        for row in 0..BUFFER_HEIGHT {
            self.clear_row(row);
        }
        self.row_position = BUFFER_HEIGHT - 1;
        self.column_position = 0;
        self.move_hardware_cursor();
    }

    /// `SCREEN_CLEAR`: apaga as 25 linhas com a cor **atual** e leva o cursor
    /// ao canto de cima, à esquerda.
    fn clear_all(&mut self) {
        for row in 0..BUFFER_HEIGHT {
            self.clear_row(row);
        }
        self.row_position = 0;
        self.column_position = 0;
        self.move_hardware_cursor();
    }

    /// `SCREEN_MOVE`: leva o cursor a `(row, col)`. Quem chama já conferiu os
    /// limites.
    fn move_to(&mut self, row: usize, col: usize) {
        self.row_position = row;
        self.column_position = col;
        self.move_hardware_cursor();
    }

    /// `SCREEN_COLOR`: a cor de tudo o que for escrito daí em diante. Quem
    /// chama já conferiu os limites (texto `0..=15`, fundo `0..=7`).
    fn set_color(&mut self, text: u8, background: u8) {
        self.color_code = ColorCode((background << 4) | text);
    }

    /// Volta ao amarelo sobre preto.
    fn reset_color(&mut self) {
        self.color_code = ColorCode::new(Color::Yellow, Color::Black);
    }

    /// Remove o último caractere escrito, apagando-o da tela e voltando o
    /// cursor lógico uma posição. Sem efeito quando a linha já está vazia
    /// (não apaga o prompt).
    fn backspace(&mut self) {
        if self.column_position == 0 {
            return;
        }
        self.column_position -= 1;

        let row = self.row_position;
        let col = self.column_position;
        let blank = ScreenChar {
            ascii_character: b' ',
            color_code: self.color_code,
        };
        self.buffer.chars[row][col].write(blank);
        self.move_hardware_cursor();
    }

    /// Move o cursor de hardware do modo texto VGA para acompanhar a
    /// posição atual de escrita. Com a coluna em 80 (linha cheia, a quebra
    /// só acontece quando chega o próximo caractere) o cursor fica na coluna
    /// 79: a coluna 80 não existe na grade.
    fn move_hardware_cursor(&self) {
        let position = self.row_position * BUFFER_WIDTH + self.column_position.min(BUFFER_WIDTH - 1);

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
        row_position: BUFFER_HEIGHT - 1,
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

/// Marca de que um programa usou `SYS_SCREEN` neste `run`: na volta ao prompt a
/// última linha é apagada e a cor padrão volta (ver [`restore_prompt_line`]).
static SCREEN_POSITIONED: AtomicBool = AtomicBool::new(false);

/// Registra que a tela foi posicionada por um programa (qualquer das três
/// operações de `SYS_SCREEN` que foi aceita).
pub fn mark_positioned() {
    SCREEN_POSITIONED.store(true, Ordering::SeqCst);
}

/// `SCREEN_CLEAR`: apaga as 25 linhas com a cor atual e leva o cursor a `(0, 0)`.
pub fn screen_clear_all() {
    WRITER.lock().clear_all();
}

/// `SCREEN_MOVE`: leva o cursor a `(row, col)`, que o chamador já conferiu
/// (`row < 25`, `col < 80`).
pub fn screen_move(row: usize, col: usize) {
    WRITER.lock().move_to(row, col);
}

/// `SCREEN_COLOR`: escolhe a cor atual. O chamador já conferiu os limites
/// (`text <= 15`, `background <= 7`).
pub fn screen_set_color(text: u8, background: u8) {
    WRITER.lock().set_color(text, background);
}

/// Volta à cor padrão (amarelo sobre preto).
pub fn reset_color() {
    WRITER.lock().reset_color();
}

/// A volta ao prompt depois de um programa que usou a tela: se a marca está
/// ligada, leva o cursor a `(24, 0)`, restaura a cor padrão, apaga a última
/// linha (já com a cor padrão) e desliga a marca. As outras 24 linhas ficam
/// como o programa as deixou, com as cores. Sem marca não faz nada. Devolve se
/// restaurou.
pub fn restore_prompt_line() -> bool {
    if !SCREEN_POSITIONED.swap(false, Ordering::SeqCst) {
        return false;
    }
    let mut writer = WRITER.lock();
    writer.reset_color();
    writer.row_position = BUFFER_HEIGHT - 1;
    writer.column_position = 0;
    writer.clear_row(BUFFER_HEIGHT - 1);
    writer.move_hardware_cursor();
    true
}

/// Onde está o cursor, no formato `(linha, coluna)`: a posição que vai ao
/// cursor de hardware (a coluna chega a 79, nunca a 80). Só para os testes.
#[doc(hidden)]
pub fn screen_cursor() -> (usize, usize) {
    let writer = WRITER.lock();
    (writer.row_position, writer.column_position.min(BUFFER_WIDTH - 1))
}

/// O byte de cor (`fundo << 4 | texto`) da célula `(row, col)`. Só para os
/// testes.
#[doc(hidden)]
pub fn screen_cell_color(row: usize, col: usize) -> u8 {
    WRITER.lock().buffer.chars[row][col].read().color_code.0
}

/// Apaga o último caractere escrito no `Writer` VGA global (Backspace).
pub fn backspace() {
    WRITER.lock().backspace();
}

/// Verdadeiro se `needle` aparece em alguma linha da tela, em sequência
/// (sem quebra de linha no meio). Usada por testes de outros módulos
/// (ex.: `shell.rs`) e por testes de integração em `tests/` (por isso é
/// `pub`, não só `#[cfg(test)]`: um binário de teste de integração
/// depende de `os_rust` como uma crate externa comum, compilada sem
/// `--cfg test`, então não enxergaria um item `pub(crate)`/`cfg(test)`)
/// para verificar o efeito observável de um comando sem precisar prever
/// a linha exata onde o texto termina depois da rolagem.
pub fn screen_contains(needle: &str) -> bool {
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

/// Desenha o logo canônico (`crate::logo::LOGO`) direto nas linhas 0-19
/// do buffer VGA, sem passar pelo `Writer` normal — que sempre escreve na
/// última linha e rola a tela inteira a cada `\n`, mesmo partindo de uma
/// tela vazia (ver os testes `quebra_de_linha`/`rolagem_ao_ultrapassar_a_altura_da_tela`
/// abaixo). Por isso só deve ser chamada depois que `print_welcome`/
/// `shell::print_prompt` já tiverem escrito seu conteúdo pelo caminho
/// normal, nunca antes — assim o logo nasce exatamente nas linhas 0-19 e
/// nenhuma escrita seguinte o desloca. Cada `'█'` vira o byte `0xDB`
/// (bloco cheio da code page 437); ASCII imprimível vira o próprio byte;
/// qualquer outro caractere vira `0xfe`, como em `write_string`, para não
/// confiar que o texto do logo seja só ASCII/`'█'`. O logo é só visual:
/// nunca escreve na serial, e não move `column_position` nem o cursor de
/// hardware, porque nenhum código depois dele volta a escrever nessas
/// linhas.
pub fn draw_logo() {
    let mut writer = WRITER.lock();
    let color_code = ColorCode::new(Color::LightCyan, Color::Black);
    for (row, line) in crate::logo::LOGO.lines().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            let byte = match ch {
                '█' => 0xDB,
                ' '..='~' => ch as u8,
                _ => 0xfe,
            };
            writer.buffer.chars[row][col].write(ScreenChar {
                ascii_character: byte,
                color_code,
            });
        }
    }
}

/// Devolve o byte `ascii_character` de cada coluna da linha `row`, sem
/// nenhuma cor. Existe só para teste: `pub`, não `pub(crate)`/`cfg(test)`,
/// pelo mesmo motivo de `screen_contains` logo acima — o binário de teste
/// de integração compila esta crate como uma crate externa comum, sem
/// `--cfg test`. Usada para comparar linhas inteiras da tela (inclusive
/// bytes fora do ASCII imprimível, como `0xDB`) contra `logo::LOGO`
/// traduzido: `screen_contains` não serve para isso, porque recebe
/// `&str`, e `0xDB` não é um `char` UTF-8 válido sozinho.
pub fn screen_row_bytes(row: usize) -> [u8; BUFFER_WIDTH] {
    let writer = WRITER.lock();
    let mut bytes = [0u8; BUFFER_WIDTH];
    for (col, byte) in bytes.iter_mut().enumerate() {
        *byte = writer.buffer.chars[row][col].read().ascii_character;
    }
    bytes
}

/// Quantas linhas da tela têm a forma `dddd-dd-dd dd:dd:dd UTC` (a saída de
/// `data` e de `hora`), começando na primeira coluna. Confere só a **forma**,
/// nunca o valor: os testes não podem depender da hora real. `pub` pelo mesmo
/// motivo de `screen_contains`: serve também aos testes de `tests/`.
pub fn screen_count_clock_lines() -> usize {
    // d = dígito; qualquer outro caractere tem de ser igual ao do modelo.
    const MODEL: &[u8] = b"dddd-dd-dd dd:dd:dd UTC";
    (0..BUFFER_HEIGHT)
        .filter(|&row| {
            let bytes = screen_row_bytes(row);
            MODEL.iter().enumerate().all(|(i, &m)| {
                if m == b'd' {
                    bytes[i].is_ascii_digit()
                } else {
                    bytes[i] == m
                }
            })
        })
        .count()
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
    fn write_bytes_troca_byte_fora_do_ascii_pelo_quadrado() {
        clear_screen();
        WRITER.lock().write_bytes(b"ab\xffc");
        let writer = WRITER.lock();
        let ultima_linha = &writer.buffer.chars[BUFFER_HEIGHT - 1];
        assert_eq!(ultima_linha[0].read().ascii_character, b'a');
        assert_eq!(ultima_linha[1].read().ascii_character, b'b');
        assert_eq!(ultima_linha[2].read().ascii_character, 0xfe);
        assert_eq!(ultima_linha[3].read().ascii_character, b'c');
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

    // --- linha do cursor, cor e volta ao prompt (Marco 13) ---

    const PADRAO: u8 = 0x0E;

    fn restaura() {
        restore_prompt_line();
        reset_color();
        clear_screen();
    }

    fn celula(row: usize, col: usize) -> u8 {
        WRITER.lock().buffer.chars[row][col].read().ascii_character
    }

    #[test_case]
    fn sem_syscall_de_tela_o_writer_e_igual_ao_de_antes() {
        restaura();
        crate::print!("\nPRIMEIRA\nSEGUNDA");
        for (i, c) in "PRIMEIRA".bytes().enumerate() {
            assert_eq!(celula(BUFFER_HEIGHT - 2, i), c);
            assert_eq!(screen_cell_color(BUFFER_HEIGHT - 2, i), PADRAO);
        }
        for (i, c) in "SEGUNDA".bytes().enumerate() {
            assert_eq!(celula(BUFFER_HEIGHT - 1, i), c);
        }
        assert_eq!(screen_cursor(), (BUFFER_HEIGHT - 1, 7));
        restaura();
    }

    #[test_case]
    fn acima_da_ultima_linha_a_quebra_so_desce_o_cursor() {
        restaura();
        screen_clear_all();
        screen_move(5, 10);
        crate::print!("ab\ncd");
        assert_eq!((celula(5, 10), celula(5, 11)), (b'a', b'b'));
        assert_eq!((celula(6, 0), celula(6, 1)), (b'c', b'd'));
        // Nada rolou: a linha 0 continua em branco e o cursor está logo depois do `d`.
        assert_eq!(celula(0, 0), b' ');
        assert_eq!(screen_cursor(), (6, 2));
        restaura();
    }

    #[test_case]
    fn passar_da_coluna_79_acima_da_ultima_linha_vai_para_a_seguinte_sem_rolar() {
        restaura();
        screen_clear_all();
        screen_move(3, 79);
        crate::print!("xy");
        assert_eq!(celula(3, 79), b'x');
        assert_eq!(celula(4, 0), b'y');
        assert_eq!(celula(0, 0), b' ');
        restaura();
    }

    #[test_case]
    fn na_ultima_linha_a_quebra_de_coluna_rola_como_sempre() {
        restaura();
        screen_clear_all();
        screen_move(BUFFER_HEIGHT - 1, 79);
        crate::print!("xy");
        // O `x` subiu para a linha 23 e o `y` começa a linha 24.
        assert_eq!(celula(BUFFER_HEIGHT - 2, 79), b'x');
        assert_eq!(celula(BUFFER_HEIGHT - 1, 0), b'y');
        restaura();
    }

    #[test_case]
    fn oitenta_caracteres_a_partir_de_24_0_nao_rolam_e_o_cursor_fica_na_79() {
        restaura();
        screen_clear_all();
        screen_move(BUFFER_HEIGHT - 1, 0);
        for _ in 0..BUFFER_WIDTH {
            crate::print!("z");
        }
        assert_eq!(celula(BUFFER_HEIGHT - 1, 0), b'z');
        assert_eq!(celula(BUFFER_HEIGHT - 1, 79), b'z');
        assert_eq!(celula(BUFFER_HEIGHT - 2, 0), b' ');
        assert_eq!(screen_cursor(), (BUFFER_HEIGHT - 1, 79));
        // O caractere seguinte é que quebra a linha.
        crate::print!("w");
        assert_eq!(celula(BUFFER_HEIGHT - 2, 0), b'z');
        assert_eq!(celula(BUFFER_HEIGHT - 1, 0), b'w');
        restaura();
    }

    #[test_case]
    fn limpar_tudo_deixa_as_25_linhas_em_branco_e_o_cursor_em_0_0() {
        restaura();
        crate::print!("lixo");
        screen_clear_all();
        for row in 0..BUFFER_HEIGHT {
            assert_eq!(celula(row, 0), b' ');
        }
        assert_eq!(screen_cursor(), (0, 0));
        restaura();
    }

    #[test_case]
    fn sem_marca_restaurar_nao_faz_nada() {
        restaura();
        crate::print!("fica");
        assert!(!restore_prompt_line());
        assert_eq!(celula(BUFFER_HEIGHT - 1, 0), b'f');
        restaura();
    }

    #[test_case]
    fn com_marca_restaurar_apaga_so_a_linha_24_e_volta_a_cor_padrao() {
        restaura();
        screen_clear_all();
        screen_set_color(0, 7);
        screen_move(0, 0);
        crate::print!("titulo");
        screen_move(BUFFER_HEIGHT - 1, 0);
        crate::print!("rodape");
        mark_positioned();
        assert!(restore_prompt_line());
        // As linhas de cima ficaram, com a cor do programa.
        assert_eq!(celula(0, 0), b't');
        assert_eq!(screen_cell_color(0, 0), 0x70);
        // A linha 24 foi apagada com a cor padrão e o cursor está nela.
        assert_eq!(celula(BUFFER_HEIGHT - 1, 0), b' ');
        assert_eq!(screen_cell_color(BUFFER_HEIGHT - 1, 0), PADRAO);
        assert_eq!(screen_cursor(), (BUFFER_HEIGHT - 1, 0));
        // E a marca foi desligada.
        assert!(!restore_prompt_line());
        crate::print!("p");
        assert_eq!(screen_cell_color(BUFFER_HEIGHT - 1, 0), PADRAO);
        restaura();
    }

    #[test_case]
    fn a_cor_escolhida_vale_para_o_que_se_escreve_a_rolagem_e_o_clear_all() {
        restaura();
        screen_set_color(0, 7);
        crate::print!("A");
        assert_eq!(screen_cell_color(BUFFER_HEIGHT - 1, 0), 0x70);
        // A linha nova aberta pela rolagem usa a cor atual.
        crate::print!("\n");
        assert_eq!(screen_cell_color(BUFFER_HEIGHT - 1, 5), 0x70);
        // O que já estava escrito não mudou de cor.
        reset_color();
        crate::print!("B");
        assert_eq!(screen_cell_color(BUFFER_HEIGHT - 1, 0), PADRAO);
        // `screen_clear_all` pinta as 25 linhas com a cor atual e não a muda.
        screen_set_color(15, 7);
        screen_clear_all();
        assert_eq!(screen_cell_color(0, 0), 0x7F);
        assert_eq!(screen_cell_color(BUFFER_HEIGHT - 1, 79), 0x7F);
        crate::print!("C");
        assert_eq!(screen_cell_color(0, 0), 0x7F);
        restaura();
    }

    #[test_case]
    fn todas_as_cores_de_texto_e_fundo_viram_o_byte_certo() {
        restaura();
        for fundo in 0..8u8 {
            for texto in 0..16u8 {
                screen_set_color(texto, fundo);
                screen_move(1, 1);
                crate::print!("x");
                assert_eq!(screen_cell_color(1, 1), (fundo << 4) | texto);
            }
        }
        restaura();
    }
}
