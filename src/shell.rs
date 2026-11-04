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
