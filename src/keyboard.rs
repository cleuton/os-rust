//! Tradução de Scan Code Set 1 para ASCII, layout US QWERTY.

use spin::Mutex;

/// Tamanho máximo de uma linha digitada, do prompt e também das linhas que um
/// programa lê com `SYS_READ_LINE`. Suficiente para qualquer comando razoável
/// em uma demonstração ao vivo; nada além disso é alocado.
pub const LINE_CAPACITY: usize = 128;

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


// ---------------------------------------------------------------------------
// Decodificador de teclas (SYS_READ_KEY)
// ---------------------------------------------------------------------------

/// Onde o decodificador está dentro de uma sequência de vários scancodes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// Nenhuma sequência em andamento.
    Nothing,
    /// Veio o prefixo `0xE0`: o próximo byte é de uma tecla estendida.
    Extended,
    /// Veio o prefixo `0xE1` (Pause/Break): ainda faltam `n` bytes a descartar.
    Pause(u8),
}

/// O que o decodificador lembra entre um scancode e o seguinte: o prefixo
/// pendente e as duas teclas Ctrl (independentes, como o Shift).
struct Decoder {
    pending: Pending,
    ctrl_left: bool,
    ctrl_right: bool,
}

impl Decoder {
    const fn new() -> Decoder {
        Decoder { pending: Pending::Nothing, ctrl_left: false, ctrl_right: false }
    }

    fn ctrl(&self) -> bool {
        self.ctrl_left || self.ctrl_right
    }

    /// Soma `KEY_CTRL` ao código quando Ctrl está pressionado.
    fn with_ctrl(&self, code: u32) -> u32 {
        if self.ctrl() {
            code | abi::KEY_CTRL
        } else {
            code
        }
    }
}

/// O estado único do decodificador. Ele precisa enxergar **todo** scancode que
/// o kernel tira da fila (o prompt, a espera de linha e a de tecla), senão um
/// Ctrl apertado num lugar e solto em outro ficaria preso.
static DECODER: Mutex<Decoder> = Mutex::new(Decoder::new());

/// Decodifica um scancode do Scan Code Set 1 e devolve o código da tecla
/// quando ele completa um evento (uma tecla **pressionada**; soltar nunca gera
/// evento). O código é `>= 1`: o ASCII para teclas de texto, `KEY_*` (a partir
/// de `0x100`) para as especiais, com `KEY_CTRL` somado se Ctrl está
/// pressionado (e, com Ctrl, uma letra sempre minúscula).
///
/// Atualiza o estado (prefixo pendente e Ctrl). Depois de `0xE0`, o byte
/// seguinte **não** passa por [`translate`]: assim o "falso Shift" que algumas
/// teclas estendidas enviam (`0xE0 0x2A`, `0xE0 0xAA`) nunca liga nem desliga o
/// Shift. Teclas fora da tabela (F1 a F12, Alt, Caps Lock, Windows, teclado
/// numérico sem prefixo, PrintScreen, Pause) não geram evento.
pub fn decode(scancode: u8) -> Option<u32> {
    let mut state = DECODER.lock();
    match state.pending {
        Pending::Pause(left) => {
            state.pending = if left > 1 { Pending::Pause(left - 1) } else { Pending::Nothing };
            return None;
        }
        Pending::Extended => {
            state.pending = Pending::Nothing;
            return match scancode {
                0x1D => {
                    state.ctrl_right = true;
                    None
                }
                0x9D => {
                    state.ctrl_right = false;
                    None
                }
                code if code & 0x80 != 0 => None,
                code => {
                    let key = match code {
                        0x48 => abi::KEY_UP,
                        0x50 => abi::KEY_DOWN,
                        0x4B => abi::KEY_LEFT,
                        0x4D => abi::KEY_RIGHT,
                        0x47 => abi::KEY_HOME,
                        0x4F => abi::KEY_END,
                        0x49 => abi::KEY_PAGE_UP,
                        0x51 => abi::KEY_PAGE_DOWN,
                        0x52 => abi::KEY_INSERT,
                        0x53 => abi::KEY_DELETE,
                        // Enter e barra do teclado numérico valem as teclas normais.
                        0x1C => b'\n' as u32,
                        0x35 => b'/' as u32,
                        _ => return None,
                    };
                    Some(state.with_ctrl(key))
                }
            };
        }
        Pending::Nothing => {}
    }
    match scancode {
        0xE0 => {
            state.pending = Pending::Extended;
            None
        }
        0xE1 => {
            state.pending = Pending::Pause(2);
            None
        }
        0x1D => {
            state.ctrl_left = true;
            None
        }
        0x9D => {
            state.ctrl_left = false;
            None
        }
        // Tab e Esc: o `translate` não os conhece (e não deve conhecer, para o
        // prompt e as linhas seguirem iguais).
        0x0F => Some(state.with_ctrl(0x09)),
        0x01 => Some(state.with_ctrl(0x1B)),
        _ => {
            // Letras, dígitos, pontuação, espaço, Enter e Backspace, com o
            // mesmo Shift de sempre.
            let byte = translate(scancode)?;
            let byte = if state.ctrl() { byte.to_ascii_lowercase() } else { byte };
            Some(state.with_ctrl(byte as u32))
        }
    }
}

/// Descarta o prefixo pendente (`0xE0` ou `0xE1`), sem tocar em Ctrl: a tecla
/// pode continuar fisicamente apertada. O `run` e o `exec` o chamam quando o
/// último programa termina, para uma sequência cortada não estragar o prompt.
pub fn reset_prefix() {
    DECODER.lock().pending = Pending::Nothing;
}

/// Zera o prefixo e solta Ctrl. Só para os testes, que esvaziam a fila de
/// scancodes sem passar pelo decodificador.
#[doc(hidden)]
pub fn reset_decoder() {
    *DECODER.lock() = Decoder::new();
}

/// O montador de uma linha digitada: recebe um scancode por vez, faz o eco na
/// tela, trata o Backspace e avisa quando o Enter completa a linha. É a parte
/// de `SYS_READ_LINE` que traduz teclas em texto, separada de **quem espera**:
/// o escalonador usa um `LineEditor` para montar a linha do programa que pediu
/// primeiro (`scheduler.rs`), e `read_line` o usa para um leitor só.
///
/// Aceita bytes ASCII imprimíveis (`0x20`–`0x7e`), no máximo `max_chars`
/// (sobra lugar para o `\n`); o que passar disso é ignorado, sem eco, como o
/// prompt faz no limite de linha.
pub struct LineEditor {
    buffer: [u8; LINE_CAPACITY],
    len: usize,
    max_chars: usize,
}

impl LineEditor {
    /// Um editor vazio. É `const` porque mora dentro de uma `static`.
    pub const fn new() -> LineEditor {
        LineEditor {
            buffer: [0; LINE_CAPACITY],
            len: 0,
            max_chars: 0,
        }
    }

    /// Começa uma linha nova, com no máximo `max_chars` caracteres (limitado a
    /// `LINE_CAPACITY - 1`, para caber o `\n`).
    pub fn reset(&mut self, max_chars: usize) {
        self.len = 0;
        self.max_chars = max_chars.min(LINE_CAPACITY - 1);
    }

    /// Processa um scancode. Devolve `Some(n)` quando o Enter completou a
    /// linha: `n` é o total de bytes de [`LineEditor::line`], incluindo o `\n`
    /// final (`1 ≤ n ≤ max_chars + 1`). Devolve `None` para qualquer outra
    /// tecla.
    pub fn feed(&mut self, scancode: u8) -> Option<usize> {
        match translate(scancode) {
            Some(b'\n') => {
                crate::println!();
                self.buffer[self.len] = b'\n';
                Some(self.len + 1)
            }
            Some(0x08) => {
                if self.len > 0 {
                    self.len -= 1;
                    crate::vga_buffer::backspace();
                }
                None
            }
            Some(byte) if (0x20..=0x7e).contains(&byte) && self.len < self.max_chars => {
                self.buffer[self.len] = byte;
                self.len += 1;
                crate::print!("{}", byte as char);
                None
            }
            // Byte fora da faixa, linha cheia ou tecla sem tradução: ignora.
            _ => None,
        }
    }

    /// Os primeiros `written` bytes da linha completa (o valor que `feed`
    /// devolveu).
    pub fn line(&self, written: usize) -> &[u8] {
        &self.buffer[..written]
    }
}

/// Lê uma linha do teclado para `out`, **bloqueando** até o Enter, com eco na
/// tela e Backspace. Devolve quantos bytes escreveu, incluindo o `\n` final
/// (sempre `1 ≤ n ≤ out.len()`; `0` só se `out` for vazio).
///
/// É a leitura de **um** leitor só, que fica esperando na própria CPU: usada
/// pelos testes. Os programas de usuário não passam por aqui: o escalonador
/// bloqueia a tarefa e monta a linha com um [`LineEditor`], de modo que quem
/// espera não gasta CPU e vários programas podem esperar ao mesmo tempo.
///
/// Espera com `sti; hlt`, e **sempre volta com as interrupções desligadas**:
/// quem chama é um trecho do kernel que roda com `IF` desligado.
pub fn read_line(out: &mut [u8]) -> usize {
    use x86_64::instructions::interrupts;

    if out.is_empty() {
        return 0;
    }
    let mut editor = LineEditor::new();
    editor.reset(out.len() - 1);
    loop {
        // A fila só é tocada com as interrupções desligadas (invariante de
        // `next_scancode`): a IRQ1 nunca disputa o `Mutex` dela.
        interrupts::disable();
        let Some(scancode) = crate::interrupts::next_scancode() else {
            // Fila vazia. `enable_and_hlt` é `sti; hlt` como uma única
            // instrução atômica: uma tecla que chegue entre o teste da fila e
            // o `hlt` acorda a CPU em vez de ficar esquecida até a tecla
            // seguinte. Ao acordar as interrupções seguem ligadas; o topo do
            // laço as desliga de novo antes de tocar na fila.
            interrupts::enable_and_hlt();
            continue;
        };
        // O decodificador vê todo scancode, para o Ctrl nunca ficar preso.
        decode(scancode);
        if let Some(written) = editor.feed(scancode) {
            out[..written].copy_from_slice(editor.line(written));
            interrupts::disable();
            return written;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Garante Shift solto no início de cada teste, independente da
    /// ordem de execução — `SHIFT` é um estado global compartilhado por
    /// toda a suíte.
    fn reset_shift() {
        translate(LEFT_SHIFT_BREAK);
        translate(RIGHT_SHIFT_BREAK);
    }

    #[test_case]
    fn traduz_letra_minuscula() {
        reset_shift();
        assert_eq!(translate(0x1E), Some(b'a'));
    }

    #[test_case]
    fn traduz_letra_maiuscula_com_shift() {
        reset_shift();
        assert_eq!(translate(LEFT_SHIFT_MAKE), None);
        assert_eq!(translate(0x1E), Some(b'A'));
        reset_shift();
    }

    #[test_case]
    fn traduz_digito_e_simbolo_com_shift() {
        reset_shift();
        assert_eq!(translate(0x02), Some(b'1'));
        assert_eq!(translate(RIGHT_SHIFT_MAKE), None);
        assert_eq!(translate(0x02), Some(b'!'));
        reset_shift();
    }

    #[test_case]
    fn ignora_break_code_de_tecla_comum() {
        reset_shift();
        // Break code de 'a' (0x1E | 0x80): qualquer break code exceto o
        // de Shift é ignorado.
        assert_eq!(translate(0x9E), None);
    }

    #[test_case]
    fn ignora_scancode_nao_mapeado() {
        reset_shift();
        // 0x01 (Escape) não está na tabela de tradução.
        assert_eq!(translate(0x01), None);
    }

    // --- read_line ---

    const ENTER: u8 = 0x1C;
    const BACKSPACE: u8 = 0x0E;
    const KEY_H: u8 = 0x23;
    const KEY_I: u8 = 0x17;
    const KEY_A: u8 = 0x1E;
    const KEY_B: u8 = 0x30;
    const KEY_C: u8 = 0x2E;
    const KEY_D: u8 = 0x20;
    const KEY_E: u8 = 0x12;
    const ESCAPE: u8 = 0x01;

    /// Fila vazia e Shift solto: `read_line` usa a fila global e o estado
    /// global de Shift, então cada teste começa do zero.
    fn reset_teclado() {
        while crate::interrupts::next_scancode().is_some() {}
        reset_shift();
        reset_decoder();
    }

    fn digitar(scancodes: &[u8]) {
        for &scancode in scancodes {
            crate::interrupts::push_scancode(scancode);
        }
    }

    #[test_case]
    fn read_line_devolve_exatamente_o_que_foi_digitado() {
        reset_teclado();
        digitar(&[KEY_H, KEY_I, ENTER]);
        let mut out = [0u8; 8];
        assert_eq!(read_line(&mut out), 3);
        assert_eq!(&out[..3], b"hi\n");
    }

    #[test_case]
    fn read_line_com_so_enter_devolve_a_quebra_de_linha() {
        reset_teclado();
        digitar(&[ENTER]);
        let mut out = [0u8; 8];
        assert_eq!(read_line(&mut out), 1);
        assert_eq!(out[0], b'\n');
    }

    #[test_case]
    fn read_line_respeita_shift() {
        reset_teclado();
        digitar(&[LEFT_SHIFT_MAKE, KEY_A, LEFT_SHIFT_BREAK, ENTER]);
        let mut out = [0u8; 8];
        assert_eq!(read_line(&mut out), 2);
        assert_eq!(&out[..2], b"A\n");
    }

    #[test_case]
    fn read_line_backspace_apaga_o_ultimo_caractere() {
        reset_teclado();
        digitar(&[KEY_A, KEY_B, KEY_C, BACKSPACE, KEY_D, ENTER]);
        let mut out = [0u8; 8];
        assert_eq!(read_line(&mut out), 4);
        assert_eq!(&out[..4], b"abd\n");
    }

    #[test_case]
    fn read_line_backspace_com_linha_vazia_nao_faz_nada() {
        reset_teclado();
        digitar(&[BACKSPACE, KEY_A, ENTER]);
        let mut out = [0u8; 8];
        assert_eq!(read_line(&mut out), 2);
        assert_eq!(&out[..2], b"a\n");
    }

    #[test_case]
    fn read_line_ignora_o_que_passa_do_limite() {
        reset_teclado();
        // Buffer de 4 bytes: cabem 3 caracteres e o `\n`.
        digitar(&[KEY_A, KEY_B, KEY_C, KEY_D, KEY_E, ENTER]);
        let mut out = [0u8; 4];
        assert_eq!(read_line(&mut out), 4);
        assert_eq!(&out[..4], b"abc\n");
    }

    #[test_case]
    fn read_line_ignora_tecla_sem_traducao() {
        reset_teclado();
        digitar(&[ESCAPE, KEY_A, ENTER]);
        let mut out = [0u8; 8];
        assert_eq!(read_line(&mut out), 2);
        assert_eq!(&out[..2], b"a\n");
    }

    #[test_case]
    fn read_line_com_buffer_vazio_devolve_zero_sem_esperar() {
        reset_teclado();
        let mut out = [0u8; 0];
        assert_eq!(read_line(&mut out), 0);
    }

    #[test_case]
    fn read_line_volta_com_as_interrupcoes_desligadas() {
        use x86_64::instructions::interrupts;
        reset_teclado();
        digitar(&[KEY_H, ENTER]);
        // Liga as interrupções antes: `read_line` precisa devolvê-las
        // desligadas, porque quem a chama é um trecho do kernel que roda com
        // `IF` desligado (o retorno a ring 3 por `iretq` é que religa o `IF`,
        // junto com a pilha do programa).
        interrupts::enable();
        let mut out = [0u8; 8];
        read_line(&mut out);
        assert!(!interrupts::are_enabled());
    }

    // --- decodificador de teclas ---

    use abi::{
        KEY_CTRL, KEY_DELETE, KEY_DOWN, KEY_END, KEY_HOME, KEY_INSERT, KEY_LEFT, KEY_PAGE_DOWN,
        KEY_PAGE_UP, KEY_RIGHT, KEY_UP,
    };

    /// Começa do zero: Shift solto e decodificador limpo.
    fn limpo() {
        reset_shift();
        reset_decoder();
    }

    /// Passa uma sequência de scancodes e devolve todos os eventos.
    fn eventos(scancodes: &[u8]) -> alloc::vec::Vec<u32> {
        scancodes.iter().filter_map(|&s| decode(s)).collect()
    }

    #[test_case]
    fn decodifica_texto_e_shift() {
        limpo();
        assert_eq!(eventos(&[0x1E, 0x9E]), [b'a' as u32]);
        assert_eq!(eventos(&[LEFT_SHIFT_MAKE, 0x1E, 0x9E, LEFT_SHIFT_BREAK]), [b'A' as u32]);
        assert_eq!(eventos(&[0x02, 0x39]), [b'1' as u32, b' ' as u32]);
        limpo();
    }

    #[test_case]
    fn decodifica_enter_backspace_tab_esc() {
        limpo();
        assert_eq!(eventos(&[0x1C, 0x0E, 0x0F, 0x01]), [0x0A, 0x08, 0x09, 0x1B]);
        limpo();
    }

    #[test_case]
    fn decodifica_as_dez_teclas_especiais() {
        limpo();
        let tabela = [
            (0x48, KEY_UP),
            (0x50, KEY_DOWN),
            (0x4B, KEY_LEFT),
            (0x4D, KEY_RIGHT),
            (0x47, KEY_HOME),
            (0x4F, KEY_END),
            (0x49, KEY_PAGE_UP),
            (0x51, KEY_PAGE_DOWN),
            (0x53, KEY_DELETE),
            (0x52, KEY_INSERT),
        ];
        for (make, esperado) in tabela {
            // Pressionar gera o evento; soltar (o mesmo código com o bit 7) não.
            assert_eq!(eventos(&[0xE0, make, 0xE0, make | 0x80]), [esperado]);
        }
        limpo();
    }

    #[test_case]
    fn decodifica_enter_e_barra_do_teclado_numerico() {
        limpo();
        assert_eq!(eventos(&[0xE0, 0x1C, 0xE0, 0x9C]), [0x0A]);
        assert_eq!(eventos(&[0xE0, 0x35, 0xE0, 0xB5]), [0x2F]);
        limpo();
    }

    #[test_case]
    fn ctrl_soma_o_bit_e_a_letra_vem_minuscula() {
        limpo();
        // Ctrl esquerdo + q.
        assert_eq!(eventos(&[0x1D, 0x10]), [KEY_CTRL | b'q' as u32]);
        // Com Shift também: continua minúscula.
        assert_eq!(eventos(&[LEFT_SHIFT_MAKE, 0x10, LEFT_SHIFT_BREAK]), [KEY_CTRL | b'q' as u32]);
        // Com Ctrl, uma seta também leva o bit.
        assert_eq!(eventos(&[0xE0, 0x4B]), [KEY_CTRL | KEY_LEFT]);
        // Solto o Ctrl, volta ao normal.
        assert_eq!(eventos(&[0x9D, 0x10]), [b'q' as u32]);
        limpo();
    }

    #[test_case]
    fn ctrl_direito_vale_igual_e_os_dois_sao_independentes() {
        limpo();
        assert_eq!(eventos(&[0xE0, 0x1D, 0x10]), [KEY_CTRL | b'q' as u32]);
        // Aperta o esquerdo também e solta o direito: ainda vale (o esquerdo).
        assert_eq!(eventos(&[0x1D, 0xE0, 0x9D, 0x10]), [KEY_CTRL | b'q' as u32]);
        assert_eq!(eventos(&[0x9D, 0x10]), [b'q' as u32]);
        limpo();
    }

    #[test_case]
    fn falso_shift_nao_mexe_no_shift_e_nao_gera_evento() {
        limpo();
        // Seta para cima como o teclado a envia com Num Lock: falso Shift,
        // a tecla e o falso Shift de volta.
        assert_eq!(eventos(&[0xE0, 0x2A, 0xE0, 0x48, 0xE0, 0xC8, 0xE0, 0xAA]), [KEY_UP]);
        // O Shift não ficou ligado: a letra sai minúscula.
        assert_eq!(eventos(&[0x1E]), [b'a' as u32]);
        assert_eq!(eventos(&[0xE0, 0x36, 0xE0, 0xB6, 0x1E]), [b'a' as u32]);
        limpo();
    }

    #[test_case]
    fn pause_e_descartada_por_inteiro() {
        limpo();
        // E1 1D 45 E1 9D C5: nem liga o Ctrl (0x1D) nem gera evento.
        assert_eq!(eventos(&[0xE1, 0x1D, 0x45, 0xE1, 0x9D, 0xC5]), []);
        assert_eq!(eventos(&[0x1E]), [b'a' as u32]);
        limpo();
    }

    #[test_case]
    fn teclas_fora_da_tabela_nao_geram_evento_nem_estragam_a_seguinte() {
        limpo();
        // F1 a F10, F11, F12.
        for f in (0x3B..=0x44).chain([0x57, 0x58]) {
            assert_eq!(eventos(&[f, f | 0x80]), []);
        }
        // Alt, Caps Lock, Alt direito (E0 38), Windows (E0 5B, E0 5C), PrintScreen.
        assert_eq!(eventos(&[0x38, 0xB8, 0x3A, 0xBA]), []);
        assert_eq!(eventos(&[0xE0, 0x38, 0xE0, 0xB8]), []);
        assert_eq!(eventos(&[0xE0, 0x5B, 0xE0, 0xDB, 0xE0, 0x5C, 0xE0, 0xDC]), []);
        assert_eq!(eventos(&[0xE0, 0x2A, 0xE0, 0x37, 0xE0, 0xB7, 0xE0, 0xAA]), []);
        // Teclado numérico sem prefixo (7, 8, *, -, +).
        assert_eq!(eventos(&[0x47, 0x48, 0x37, 0x4A, 0x4E]), []);
        // Shift sozinho e Ctrl sozinho não geram evento.
        assert_eq!(eventos(&[LEFT_SHIFT_MAKE, LEFT_SHIFT_BREAK, 0x1D, 0x9D]), []);
        assert_eq!(eventos(&[0x1E]), [b'a' as u32]);
        limpo();
    }

    #[test_case]
    fn soltar_nunca_gera_evento_e_nenhum_codigo_e_zero() {
        limpo();
        for scancode in 0x80..=0xFFu8 {
            assert_eq!(decode(scancode), None);
            reset_decoder();
        }
        for scancode in 0..0x80u8 {
            if let Some(code) = decode(scancode) {
                assert!(code >= 1);
            }
            reset_decoder();
            reset_shift();
        }
        limpo();
    }

    #[test_case]
    fn reset_prefix_descarta_o_prefixo_sem_soltar_o_ctrl() {
        limpo();
        assert_eq!(eventos(&[0x1D, 0xE0]), []);
        reset_prefix();
        // O 0x48 agora é só o scancode de 8 do teclado numérico: sem evento.
        assert_eq!(eventos(&[0x48]), []);
        // O Ctrl continua apertado.
        assert_eq!(eventos(&[0x10]), [KEY_CTRL | b'q' as u32]);
        limpo();
    }

    #[test_case]
    fn o_prompt_o_line_editor_e_o_translate_seguem_iguais() {
        limpo();
        // Tab e Esc continuam sem tradução para o prompt e as linhas.
        assert_eq!(translate(0x0F), None);
        assert_eq!(translate(0x01), None);
        // O decodificador não altera o resultado de `translate`.
        decode(0x1E);
        assert_eq!(translate(0x1E), Some(b'a'));
        limpo();
    }

    #[test_case]
    fn ctrl_apertado_dentro_de_read_line_nao_fica_preso() {
        reset_teclado();
        // Ctrl (0x1D), "a" e Enter passam por `read_line`; o Ctrl solto (0x9D)
        // chega depois, pelo decodificador direto.
        digitar(&[0x1D, KEY_A, ENTER]);
        let mut out = [0u8; 8];
        let n = read_line(&mut out);
        assert_eq!(&out[..n], b"a\n");
        // O decodificador viu o Ctrl apertado dentro de `read_line`.
        assert_eq!(decode(0x10), Some(KEY_CTRL | b'q' as u32));
        // Solto depois, a tecla volta ao normal.
        decode(0x9D);
        assert_eq!(decode(0x10), Some(b'q' as u32));
        reset_teclado();
    }
}
