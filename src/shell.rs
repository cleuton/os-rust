//! Buffer de linha e prompt de comandos mínimo (`help`, `clear`, `echo`, `sobre`, `panic`).

use spin::Mutex;

use crate::{print, println, vga_buffer};

/// Texto fixo do prompt, exibido sempre que o sistema está pronto para
/// receber uma nova linha.
const PROMPT: &str = "proto-os> ";

/// Tamanho máximo de uma linha digitada. Suficiente para qualquer comando
/// razoável em uma demonstração ao vivo; nada além disso é alocado.
const LINE_CAPACITY: usize = 128;

/// Tabela fechada de comandos: nome + descrição de uma linha (usada por
/// `help`) e também a lista de nomes válidos para o `match` de despacho.
const COMMANDS: &[(&str, &str)] = &[
    ("help", "lista os comandos disponiveis"),
    ("clear", "limpa a tela"),
    ("echo", "repete o texto digitado"),
    ("sobre", "descreve o proto-os"),
    ("panic", "dispara um panic proposital"),
];

/// Acumulador de tamanho fixo dos caracteres digitados até o próximo
/// Enter — sem alocação de heap.
struct LineBuffer {
    bytes: [u8; LINE_CAPACITY],
    len: usize,
}

impl LineBuffer {
    const fn new() -> Self {
        LineBuffer {
            bytes: [0; LINE_CAPACITY],
            len: 0,
        }
    }

    /// Adiciona um byte à linha. Retorna `false` sem efeito quando a linha
    /// já está cheia — o chamador simplesmente não ecoa o byte recusado.
    fn push(&mut self, byte: u8) -> bool {
        if self.len == LINE_CAPACITY {
            return false;
        }
        self.bytes[self.len] = byte;
        self.len += 1;
        true
    }

    /// Remove o último byte da linha. Retorna `false` sem efeito quando a
    /// linha já está vazia (Backspace não apaga o prompt).
    fn pop(&mut self) -> bool {
        if self.len == 0 {
            return false;
        }
        self.len -= 1;
        true
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    /// Conteúdo válido da linha. Sempre ASCII válido, já que só bytes
    /// ASCII imprimíveis chegam a `push`.
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

static LINE: Mutex<LineBuffer> = Mutex::new(LineBuffer::new());

/// Imprime o prompt fixo, sem quebra de linha (o cursor fica logo após).
pub fn print_prompt() {
    print!("{}", PROMPT);
}

/// Processa um byte já traduzido pelo teclado (ver `keyboard::translate`):
/// Enter encerra e executa a linha atual; Backspace apaga o último
/// caractere; qualquer outro byte é acumulado no buffer de linha e, se
/// aceito, ecoado na tela.
pub fn feed(byte: u8) {
    match byte {
        b'\n' => {
            println!();
            let mut line = LINE.lock();
            execute(line.as_str());
            line.clear();
            drop(line);
            print_prompt();
        }
        0x08 => {
            if LINE.lock().pop() {
                vga_buffer::backspace();
            }
        }
        byte if (0x20..=0x7e).contains(&byte) => {
            if LINE.lock().push(byte) {
                print!("{}", byte as char);
            }
        }
        _ => {}
    }
}

/// Interpreta uma linha já completa: separa o nome do comando (primeira
/// palavra) do restante, ignorando espaços extras nas pontas, e despacha
/// para o comportamento correspondente.
fn execute(line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }

    let (name, rest) = match line.split_once(' ') {
        Some((name, rest)) => (name, rest),
        None => (line, ""),
    };

    match name {
        "help" => cmd_help(),
        "clear" => vga_buffer::clear_screen(),
        "echo" => cmd_echo(rest),
        "sobre" => cmd_sobre(),
        "panic" => cmd_panic(),
        _ => println!("comando desconhecido: {} (digite help)", name),
    }
}

fn cmd_help() {
    for (name, description) in COMMANDS {
        println!("{} - {}", name, description);
    }
}

fn cmd_echo(rest: &str) {
    println!("{}", rest);
}

fn cmd_sobre() {
    println!("proto-os: demonstracao de boot bare metal em Rust, sem SO por baixo.");
    println!("Boot via BIOS, saida VGA e teclado via IRQ1, tudo no mesmo binario.");
}

fn cmd_panic() {
    panic!("comando panic executado no prompt");
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- LineBuffer ---

    #[test_case]
    fn linha_acumula_caracteres_digitados() {
        let mut line = LineBuffer::new();
        assert!(line.push(b'a'));
        assert!(line.push(b'b'));
        assert!(line.push(b'c'));
        assert_eq!(line.as_str(), "abc");
    }

    #[test_case]
    fn linha_backspace_remove_ultimo_caractere() {
        let mut line = LineBuffer::new();
        line.push(b'a');
        line.push(b'b');
        assert!(line.pop());
        assert_eq!(line.as_str(), "a");
        // Backspace com a linha já vazia não tem efeito (retorna false).
        let mut empty = LineBuffer::new();
        assert!(!empty.pop());
        assert_eq!(empty.as_str(), "");
    }

    #[test_case]
    fn linha_respeita_limite_de_tamanho() {
        let mut line = LineBuffer::new();
        for _ in 0..LINE_CAPACITY {
            assert!(line.push(b'x'));
        }
        // O byte além do limite é recusado, sem efeito.
        assert!(!line.push(b'y'));
        assert_eq!(line.as_str().len(), LINE_CAPACITY);
    }

    // --- Interpretação de comandos ---
    //
    // NÃO testar `execute("panic")` aqui: `cmd_panic()` chama `panic!`,
    // e qualquer panic dentro de um `#[test_case]` normal é tratado como
    // falha pelo executor de testes (`test_panic_handler`), abortando o
    // binário inteiro em vez de passar. A cobertura do comando `panic`
    // fica por conta da validação manual (`quickstart.md`, Cenário 1) e
    // da leitura do código acima.

    #[test_case]
    fn comando_help_lista_os_comandos() {
        vga_buffer::clear_screen();
        execute("help");
        assert!(vga_buffer::screen_contains("lista os comandos disponiveis"));
    }

    #[test_case]
    fn comando_clear_limpa_a_tela() {
        vga_buffer::clear_screen();
        print!("conteudo antes do clear");
        execute("clear");
        assert!(vga_buffer::screen_is_blank());
    }

    #[test_case]
    fn comando_echo_repete_o_texto_digitado() {
        vga_buffer::clear_screen();
        execute("echo ola mundo");
        assert!(vga_buffer::screen_contains("ola mundo"));
    }

    #[test_case]
    fn comando_sobre_descreve_o_proto_os() {
        vga_buffer::clear_screen();
        execute("sobre");
        assert!(vga_buffer::screen_contains("proto-os"));
    }

    #[test_case]
    fn comando_desconhecido_mostra_mensagem_de_erro() {
        vga_buffer::clear_screen();
        execute("xyz");
        assert!(vga_buffer::screen_contains("comando desconhecido: xyz"));
    }

    #[test_case]
    fn linha_vazia_nao_faz_nada() {
        vga_buffer::clear_screen();
        execute("");
        assert!(vga_buffer::screen_is_blank());
    }

    #[test_case]
    fn espacos_extras_nas_pontas_sao_ignorados() {
        vga_buffer::clear_screen();
        execute("   sobre   ");
        assert!(vga_buffer::screen_contains("proto-os"));
    }
}
