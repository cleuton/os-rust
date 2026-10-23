//! Buffer de linha e prompt de comandos mínimo (`help`, `clear`, `echo`, `sobre`, `panic`, `mem`, `falha`, `ls`, `cat`, `grava`, `anexa`, `apaga`, `mkdir`, `rmdir`, `run`, `data`, `rede`, `ping`).

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use abi::{DateTime, MAX_TASKS};

use crate::fat::Kind;
use crate::fs::{self, FsError, OpenFile, VolumeId};
use crate::keyboard::LINE_CAPACITY;
use crate::net::{self, CardStatus};
use crate::rtc::{self, RtcError};
use crate::user::{self, Finished, RunError, Termination};
use crate::{
    allocator, interrupts, keyboard, memory, print, println, serial_println, vga_buffer, NAME,
    VERSION,
};

/// Texto fixo do prompt, exibido sempre que o sistema está pronto para
/// receber uma nova linha — derivado do nome do pacote em `Cargo.toml`,
/// mesma fonte de `crate::NAME`/`crate::VERSION`. Não pode referenciar
/// `NAME` diretamente: `concat!` é avaliada em tempo de compilação e só
/// aceita literais/outras macros em cada posição, não uma `const` já
/// calculada.
const PROMPT: &str = concat!(env!("CARGO_PKG_NAME"), "> ");

/// Tabela fechada de comandos: nome + descrição de uma linha (usada por
/// `help`) e também a lista de nomes válidos para o `match` de despacho.
/// A descrição de `sobre` é `const`, então deriva o nome com
/// `concat!(env!(...))` em vez de `NAME` (mesmo motivo de `PROMPT` acima).
const COMMANDS: &[(&str, &str)] = &[
    ("help", "lista os comandos disponiveis"),
    ("clear", "limpa a tela"),
    ("echo", "repete o texto digitado"),
    ("sobre", concat!("descreve o ", env!("CARGO_PKG_NAME"))),
    ("panic", "dispara um panic proposital"),
    ("mem", "mostra memoria fisica, heap, Box e Vec"),
    (
        "falha",
        "provoca uma excecao de CPU para demonstracao (pagina, pilha, opcode, protecao, breakpoint)",
    ),
    ("ls", "lista um diretorio (ex.: ls /ram, ls /disco)"),
    ("cat", "mostra o conteudo de um arquivo (ex.: cat /ram/ola.txt)"),
    ("grava", "cria um arquivo no /ram ou /disco (ex.: grava /ram/nota.txt ola)"),
    ("anexa", "acrescenta texto ao fim de um arquivo do /ram ou /disco (ex.: anexa /ram/nota.txt mais)"),
    ("apaga", "apaga um arquivo do /ram ou /disco (ex.: apaga /ram/nota.txt)"),
    ("mkdir", "cria um diretorio no /ram ou /disco (ex.: mkdir /ram/docs/novo)"),
    ("rmdir", "remove um diretorio vazio (ex.: rmdir /ram/docs/novo)"),
    ("data", "mostra a data e a hora (UTC) do relogio"),
    (
        "rede",
        "mostra a placa e o endereco, ou configura (ex.: rede, rede 10.0.2.15 255.255.255.0 10.0.2.2)",
    ),
    (
        "ping",
        "envia 4 pedidos de eco a um IP (ex.: ping 10.0.2.2); nao e o programa ping de run",
    ),
    (
        "run",
        "executa programas embutidos ou arquivos, juntos (ex.: run hello, run ping pong, run /disco/bin/visita)",
    ),
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

/// Esvazia a fila de scancodes e entrega ao prompt cada byte que o teclado
/// traduz. É o laço de teclado do kernel (chamado por `main.rs` a cada volta
/// do laço ocioso), extraído para os testes usarem exatamente o mesmo caminho
/// do prompt real.
///
/// Roda com as interrupções desligadas, para a IRQ1 nunca disputar a fila
/// (ver `interrupts::next_scancode`). Se um comando for `run <programa>`, o
/// programa roda **dentro** deste laço: enquanto ele executa, a fila não é
/// lida por aqui, e o único leitor do teclado é o programa (via
/// `SYS_READ_LINE`); quando ele termina, este laço continua e o prompt volta a
/// ser o leitor, com o que sobrou na fila.
pub fn poll_keyboard() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        while let Some(scancode) = interrupts::next_scancode() {
            if let Some(byte) = keyboard::translate(scancode) {
                feed(byte);
            }
        }
    });
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
        "mem" => cmd_mem(),
        "falha" => cmd_falha(rest),
        "ls" => cmd_ls(rest),
        "cat" => cmd_cat(rest),
        "grava" => cmd_grava(rest),
        "anexa" => cmd_anexa(rest),
        "apaga" => cmd_apaga(rest),
        "mkdir" => cmd_mkdir(rest),
        "rmdir" => cmd_rmdir(rest),
        "run" => cmd_run(rest),
        // Argumentos extras em `rest` são ignorados, como em `sobre`.
        "data" => cmd_data(),
        "rede" => cmd_rede(rest),
        "ping" => cmd_ping(rest),
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
    println!("{}", VERSION);
    println!("{}: demonstracao de boot bare metal em Rust, sem SO por baixo.", NAME);
    println!("Boot via BIOS, saida VGA e teclado via IRQ1, tudo no mesmo binario.");
}

/// `data`: lê o relógio e mostra `AAAA-MM-DD HH:MM:SS UTC`, ou o motivo de não
/// haver hora. Um relógio ruim nunca derruba o kernel.
fn cmd_data() {
    println!("{}", format_clock(rtc::read()));
}

/// O texto de `data` para um resultado do relógio. Separada de `cmd_data` para
/// que a mensagem de erro seja testada sem depender do chip.
fn format_clock(result: Result<DateTime, RtcError>) -> String {
    match result {
        Ok(time) => alloc::format!("{} UTC", time),
        Err(error) => alloc::format!("data: {}", error),
    }
}

/// O texto `a.b.c.d` de um endereço.
fn ip_text(ip: [u8; 4]) -> String {
    alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

/// `rede`: sem argumentos mostra a placa, o endereço, os contadores e as
/// conexões TCP; com três (`ip mascara gateway`) troca a configuração, só em
/// memória. Argumentos inválidos não mudam nada.
fn cmd_rede(rest: &str) {
    let args: Vec<&str> = rest.split_whitespace().collect();
    match args.len() {
        0 => {
            net::poll();
            match net::card_status() {
                CardStatus::Ready(m) => println!(
                    "placa: RTL8139 mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    m[0], m[1], m[2], m[3], m[4], m[5]
                ),
                CardStatus::Absent => println!("placa: nenhuma (RTL8139 nao encontrada)"),
                CardStatus::Unavailable(reason) => println!("placa: indisponivel ({})", reason),
            }
            let config = net::config();
            println!(
                "ip: {} mascara: {} gateway: {}",
                ip_text(config.ip),
                ip_text(config.mask),
                ip_text(config.gateway)
            );
            let stats = net::stats();
            println!(
                "quadros: recebidos={} enviados={} descartados={}",
                stats.rx, stats.tx, stats.dropped
            );
            println!("tcp: {} de {} conexoes em uso", net::tcp_in_use(), abi::MAX_TCP_CONNECTIONS);
        }
        3 => {
            let mut parsed = [[0u8; 4]; 3];
            for (i, text) in args.iter().enumerate() {
                match net::parse_ipv4(text) {
                    Some(ip) => parsed[i] = ip,
                    None => return println!("rede: endereco invalido: {}", text),
                }
            }
            match net::configure(parsed[0], parsed[1], parsed[2]) {
                Ok(()) => println!(
                    "rede: configurado ip {} mascara {} gateway {}",
                    args[0], args[1], args[2]
                ),
                Err(which) => println!("rede: endereco invalido: {}", args[which]),
            }
        }
        _ => println!("uso: rede [<ip> <mascara> <gateway>]"),
    }
}

/// Quantos ticks o `ping` espera cada resposta (1 s).
const PING_WAIT_TICKS: u64 = 100;

/// Espera até `until` ticks, andando a pilha, e devolve cedo se `done` disser
/// que já chegou o que se esperava. Gira alguns instantes antes de dormir (a
/// resposta do QEMU costuma chegar logo) e depois dorme até a próxima
/// interrupção. Roda com as interrupções desligadas, menos durante o `hlt`.
fn wait_net(until: u64, mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..2000 {
        net::poll();
        if done() {
            return true;
        }
    }
    while crate::timer::ticks() < until {
        x86_64::instructions::interrupts::disable();
        net::poll();
        if done() {
            return true;
        }
        x86_64::instructions::interrupts::enable_and_hlt();
    }
    x86_64::instructions::interrupts::disable();
    net::poll();
    done()
}

/// `ping <ip>`: quatro pedidos de eco, um por segundo. Não é o programa `ping`
/// de `run ping pong`. Durante o comando o prompt não aceita outra coisa; as
/// teclas ficam na fila do teclado.
fn cmd_ping(rest: &str) {
    let mut args = rest.split_whitespace();
    let Some(text) = args.next() else {
        return println!("uso: ping <ip>");
    };
    if args.next().is_some() {
        return println!("uso: ping <ip>");
    }
    let Some(ip) = net::parse_ipv4(text) else {
        return println!("ping: endereco invalido: {}", text);
    };
    if !net::can_reach(ip) {
        return println!("ping: sem placa de rede");
    }
    println!("ping {}: 4 pedidos", text);
    let mut received = 0;
    for seq in 1..=4u16 {
        let sent = crate::timer::ticks();
        if net::ping_send(ip, seq).is_err() {
            return println!("ping: sem placa de rede");
        }
        let until = sent + PING_WAIT_TICKS;
        if wait_net(until, || net::ping_reply(seq).is_some()) {
            let elapsed = net::ping_reply(seq).unwrap_or(sent).saturating_sub(sent);
            if elapsed == 0 {
                println!("resposta de {}: seq={} tempo<10 ms", text, seq);
            } else {
                println!("resposta de {}: seq={} tempo={} ms", text, seq, elapsed * 10);
            }
            received += 1;
            // Um pedido por segundo: espera o resto do intervalo.
            if seq < 4 {
                wait_net(until, || false);
            }
        } else {
            println!("sem resposta de {}: seq={}", text, seq);
        }
    }
    println!("ping: 4 enviados, {} recebidos", received);
}

fn cmd_panic() {
    panic!("comando panic executado no prompt");
}

/// Mostra a prova de que o kernel aloca memória dinamicamente:
/// memória física utilizável, posição/tamanho do heap, um `Box` com seu
/// valor e endereço, e um `Vec` construído a partir de vazio com
/// tamanho, capacidade e soma dos elementos. `valor` e `lista` saem de
/// escopo ao final desta função, devolvendo a memória usada ao heap —
/// por isso o comando pode ser repetido indefinidamente sem esgotar a
/// memória.
fn cmd_mem() {
    let info = memory::info();
    println!("memoria fisica utilizavel: {} KiB", info.usable_bytes / 1024);
    println!(
        "heap: {:#x}, {} KiB",
        info.heap_start,
        info.heap_size / 1024
    );

    let valor = Box::new(42);
    println!(
        "Box: valor={}, endereco={:#x}",
        *valor,
        &*valor as *const _ as usize
    );

    let mut lista = Vec::new();
    for i in 1..=10 {
        lista.push(i);
    }
    let soma: i32 = lista.iter().sum();
    println!(
        "Vec: tamanho={}, capacidade={}, soma={}",
        lista.len(),
        lista.capacity(),
        soma
    );
}

/// Despacha o tipo de exceção pedido. Sem argumento ou com um tipo
/// desconhecido, lista os tipos disponíveis em vez de adivinhar um deles
/// ou provocar uma exceção por padrão.
fn cmd_falha(tipo: &str) {
    match tipo.trim() {
        "pagina" => cmd_falha_pagina(),
        "pilha" => cmd_falha_pilha(),
        "opcode" => cmd_falha_opcode(),
        "protecao" => cmd_falha_protecao(),
        "breakpoint" => cmd_falha_breakpoint(),
        _ => println!("tipos disponiveis: pagina, pilha, opcode, protecao, breakpoint"),
    }
}

/// Provoca um page fault (`#PF`) de propósito: lê um byte de um endereço
/// logo após a última página mapeada do heap (`allocator::HEAP_START +
/// allocator::HEAP_SIZE`) — um endereço que o Marco 3 nunca mapeia.
fn cmd_falha_pagina() {
    let endereco = allocator::HEAP_START as u64 + allocator::HEAP_SIZE as u64;
    // SAFETY: este endereço fica deliberadamente logo após a última
    // página mapeada do heap (nunca mapeado — Marco 3); o page fault
    // resultante é o comportamento esperado e intencional desta
    // demonstração.
    unsafe {
        (endereco as *const u8).read_volatile();
    }
}

/// Provoca um estouro real da pilha do kernel, que o processador converte
/// em double fault (`#DF`) assim que a pilha atual se esgota.
fn cmd_falha_pilha() {
    stack_overflow();
}

/// Recursão sem caso base, deliberada: cada chamada empilha um novo
/// quadro, até estourar a pilha. A leitura volátil depois da chamada
/// recursiva impede o compilador de aplicar otimização de *tail call*
/// (que transformaria a recursão num laço sem crescer a pilha, e a
/// demonstração nunca estouraria nada).
#[allow(unconditional_recursion)]
fn stack_overflow() {
    stack_overflow();
    volatile::Volatile::new(0u8).read();
}

/// Provoca uma instrução inválida (`#UD`) de propósito.
fn cmd_falha_opcode() {
    // SAFETY: `ud2` é um opcode reservado, definido pelo manual
    // Intel/AMD como sempre inválido — a instrução seguinte nunca é
    // alcançada; a falha é o resultado intencional desta demonstração.
    unsafe {
        core::arch::asm!("ud2", options(noreturn));
    }
}

/// Provoca uma violação de proteção geral (`#GP`) de propósito: escreve
/// em um endereço virtual deliberadamente não canônico.
fn cmd_falha_protecao() {
    // SAFETY: este endereço é deliberadamente não canônico (bit 63
    // ligado, bit 47 desligado) — o modo longo do x86_64 exige que os
    // bits 63..47 de todo endereço virtual sejam iguais; qualquer acesso
    // que viole essa regra provoca #GP por definição da arquitetura. A
    // falha é o resultado intencional desta demonstração.
    unsafe {
        (0x_8000_0000_0000_0000u64 as *mut u8).write_volatile(0);
    }
}

/// Provoca um breakpoint (`#BP`) de propósito — não fatal, o controle
/// volta ao prompt logo em seguida.
fn cmd_falha_breakpoint() {
    x86_64::instructions::interrupts::int3();
}

/// Colunas de uma linha da tela de texto VGA.
const SCREEN_COLUMNS: usize = 80;

/// Mostra `disponiveis: a, b, c`, com os nomes dos programas embutidos, em uma
/// ou mais linhas. A linha quebra **entre** os nomes: a tela tem 80 colunas e,
/// com vários programas, a lista não cabe em uma linha só; deixar o `Writer`
/// quebrar sozinho partiria um nome ao meio (`falha_` na linha de cima e
/// `memoria` na de baixo).
fn print_available_programs() {
    let label = "disponiveis: ";
    print!("{}", label);
    let mut column = label.len();
    let names: Vec<&str> = user::program_names().collect();
    for (index, name) in names.iter().enumerate() {
        let separator = if index + 1 < names.len() { "," } else { "" };
        let width = name.len() + separator.len();
        // O que não cabe no resto da linha começa uma linha nova.
        if column + width > SCREEN_COLUMNS - 1 {
            println!();
            column = 0;
        }
        print!("{}{}", name, separator);
        column += width;
        if !separator.is_empty() {
            print!(" ");
            column += 1;
        }
    }
    println!();
}

/// Escreve a mesma linha na tela e na serial: a serial é o canal que o
/// host (e os testes) enxergam, a tela é a que a aula mostra.
fn say(args: core::fmt::Arguments) {
    println!("{}", args);
    serial_println!("{}", args);
}

/// Escreve `volumes: /ram (ok) /disco (indisponivel)`: o estado de cada volume.
fn print_volumes() {
    print!("volumes:");
    for volume in VolumeId::ALL {
        let state = if fs::volume_status(volume).is_ok() { "ok" } else { "indisponivel" };
        print!(" /{} ({})", volume.name(), state);
    }
    println!();
}

/// Mostra o erro de um comando que usa caminhos (`ls`, `cat`, `run`), no
/// formato de `<comando>: <o que houve>: <caminho>`. Cada erro tem uma
/// mensagem própria: a pessoa precisa saber se errou o nome, se o volume não
/// está lá ou se o disco está ruim.
fn report_fs_error(command: &str, path: &str, error: FsError) {
    match error {
        FsError::UnknownVolume => {
            say(format_args!("{}: volume desconhecido: {}", command, path));
            print_volumes();
        }
        FsError::Unavailable(reason) => {
            let volume = fs::parse(path.as_bytes()).map(|p| p.volume.name()).unwrap_or("?");
            say(format_args!("{}: volume indisponivel: /{} ({})", command, volume, reason));
        }
        FsError::InvalidPath => say(format_args!("{}: caminho invalido: {}", command, path)),
        FsError::PathTooLong => say(format_args!(
            "{}: caminho longo demais (maximo {} bytes, {} niveis)",
            command,
            fs::MAX_PATH_LEN,
            fs::MAX_DEPTH
        )),
        FsError::NotFound => say(format_args!("{}: nao encontrado: {}", command, path)),
        FsError::NotADirectory => say(format_args!("{}: nao e um diretorio: {}", command, path)),
        FsError::IsADirectory => say(format_args!("{}: e um diretorio: {}", command, path)),
        FsError::TooBig { size, max } => say(format_args!(
            "{}: {}: arquivo grande demais ({} bytes; maximo {})",
            command, path, size, max
        )),
        FsError::Io => say(format_args!(
            "{}: erro de leitura (disco ou volume corrompido): {}",
            command, path
        )),
        FsError::AlreadyExists => say(format_args!("{}: ja existe: {}", command, path)),
        FsError::NoSpace => {
            say(format_args!("{}: sem espaco (volume ou diretorio cheio): {}", command, path))
        }
        FsError::ReadOnly => {
            let volume = fs::parse(path.as_bytes()).map(|p| p.volume.name()).unwrap_or("?");
            say(format_args!("{}: volume somente leitura: /{}", command, volume));
        }
        // O mesmo erro serve a arquivo e a diretório; a mensagem diz qual.
        FsError::Busy if command == "rmdir" => say(format_args!("{}: diretorio em uso: {}", command, path)),
        FsError::Busy => say(format_args!("{}: arquivo em uso: {}", command, path)),
        FsError::NotEmpty => say(format_args!("{}: diretorio nao vazio: {}", command, path)),
        FsError::BadDescriptor | FsError::TooManyOpen => {
            say(format_args!("{}: erro interno de arquivo: {}", command, path))
        }
    }
}

/// O único argumento de `ls` e `cat`; sem ele (ou com mais de um) mostra o
/// uso e o estado dos volumes.
fn single_path<'a>(command: &str, rest: &'a str) -> Option<&'a str> {
    let mut words = rest.split_whitespace();
    match (words.next(), words.next()) {
        (Some(path), None) => Some(path),
        _ => {
            say(format_args!("uso: {} <caminho>", command));
            print_volumes();
            None
        }
    }
}

/// `ls <caminho>`: lista as entradas de um diretório, uma por linha, com o
/// tipo, o tamanho e o nome. Diretórios não têm tamanho (`-`).
fn cmd_ls(rest: &str) {
    let Some(path) = single_path("ls", rest) else { return };
    let mut dir = match OpenFile::open(path.as_bytes()) {
        Ok(dir) => dir,
        Err(error) => return report_fs_error("ls", path, error),
    };
    if dir.kind() != Kind::Dir {
        return report_fs_error("ls", path, FsError::NotADirectory);
    }
    loop {
        match dir.next_entry() {
            Ok(Some(entry)) => match entry.kind {
                Kind::Dir => println!("{:<7} {:>8}  {}", "dir", "-", entry.name_str()),
                Kind::File => println!("{:<7} {:>8}  {}", "arquivo", entry.size, entry.name_str()),
            },
            Ok(None) => return,
            Err(error) => return report_fs_error("ls", path, error),
        }
    }
}

/// `cat <caminho>`: escreve o conteúdo de um arquivo na tela, como está (um
/// byte fora do ASCII imprimível aparece como o quadrado `0xfe`, a mesma regra
/// de `SYS_WRITE`). Lê em pedaços de 128 bytes: o arquivo nunca é carregado
/// inteiro.
fn cmd_cat(rest: &str) {
    let Some(path) = single_path("cat", rest) else { return };
    let mut file = match OpenFile::open(path.as_bytes()) {
        Ok(file) => file,
        Err(error) => return report_fs_error("cat", path, error),
    };
    if file.kind() == Kind::Dir {
        return report_fs_error("cat", path, FsError::IsADirectory);
    }
    let mut chunk = [0u8; 128];
    let mut last = b'\n';
    loop {
        match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                vga_buffer::WRITER.lock().write_bytes(&chunk[..n]);
                last = chunk[n - 1];
            }
            Err(error) => {
                // O que já saiu fica na tela; a mensagem começa em linha nova.
                println!();
                return report_fs_error("cat", path, error);
            }
        }
    }
    // Um arquivo que não termina em quebra de linha não pode grudar no prompt.
    if last != b'\n' {
        println!();
    }
}

/// Separa o caminho (primeira palavra) do texto (o resto, sem os espaços
/// iniciais; os espaços internos ficam como foram digitados).
fn split_path_and_text(rest: &str) -> (&str, &str) {
    let rest = rest.trim_start();
    match rest.split_once(char::is_whitespace) {
        Some((path, text)) => (path, text.trim_start()),
        None => (rest, ""),
    }
}

/// Grava `text` e uma quebra de linha no fim do arquivo aberto. Repete a
/// escrita até acabar: um volume que enche no meio devolve o erro, e o que
/// coube já está gravado. A linha tem no máximo 128 caracteres (limite do
/// teclado), então cabe no buffer.
fn write_line(file: &mut OpenFile, text: &str) -> Result<usize, FsError> {
    let mut line = [0u8; LINE_CAPACITY + 2];
    let bytes = text.as_bytes();
    let len = bytes.len().min(LINE_CAPACITY);
    line[..len].copy_from_slice(&bytes[..len]);
    line[len] = b'\n';
    let total = len + 1;
    let mut done = 0;
    while done < total {
        done += file.write(&line[done..total])?;
    }
    Ok(total)
}

/// `grava <caminho> [<texto>]`: cria um arquivo num volume gravável (`/ram` ou
/// `/disco`) e, se houver texto, grava o texto mais uma quebra de linha. Nunca
/// sobrescreve: um nome que já existe é erro.
fn cmd_grava(rest: &str) {
    let (path, text) = split_path_and_text(rest);
    if path.is_empty() {
        say(format_args!("uso: grava <caminho> [<texto>]"));
        print_volumes();
        return;
    }
    let mut file = match OpenFile::create(path.as_bytes()) {
        Ok(file) => file,
        Err(error) => return report_fs_error("grava", path, error),
    };
    let written = if text.is_empty() {
        0
    } else {
        match write_line(&mut file, text) {
            Ok(n) => n,
            // O que coube fica gravado; a mensagem é a do erro.
            Err(error) => return report_fs_error("grava", path, error),
        }
    };
    say(format_args!("grava: criado {} ({} bytes)", path, written));
}

/// `anexa <caminho> <texto>`: acrescenta o texto, mais uma quebra de linha, ao
/// fim de um arquivo que já existe. Nunca cria.
fn cmd_anexa(rest: &str) {
    let (path, text) = split_path_and_text(rest);
    if path.is_empty() || text.is_empty() {
        say(format_args!("uso: anexa <caminho> <texto>"));
        print_volumes();
        return;
    }
    let mut file = match OpenFile::open_append(path.as_bytes()) {
        Ok(file) => file,
        Err(error) => return report_fs_error("anexa", path, error),
    };
    if let Err(error) = write_line(&mut file, text) {
        return report_fs_error("anexa", path, error);
    }
    // O tamanho novo vem do volume, não de uma conta: é o que `ls` mostraria.
    match fs::lookup(path.as_bytes()) {
        Ok((_, node)) => say(format_args!("anexa: {} agora tem {} bytes", path, node.size)),
        Err(error) => report_fs_error("anexa", path, error),
    }
}

/// `apaga <caminho>`: remove um arquivo e devolve os clusters dele ao volume.
fn cmd_apaga(rest: &str) {
    let Some(path) = single_path("apaga", rest) else { return };
    match fs::remove(path.as_bytes()) {
        Ok(()) => say(format_args!("apaga: removido {}", path)),
        Err(error) => report_fs_error("apaga", path, error),
    }
}

/// `mkdir <caminho>`: cria um diretório vazio. Não é recursivo: o pai precisa
/// existir. O diretório ocupa um cluster e não cresce (14 entradas).
fn cmd_mkdir(rest: &str) {
    let Some(path) = single_path("mkdir", rest) else { return };
    match fs::mkdir(path.as_bytes()) {
        Ok(()) => say(format_args!("mkdir: criado {}", path)),
        Err(error) => report_fs_error("mkdir", path, error),
    }
}

/// `rmdir <caminho>`: remove um diretório vazio e devolve o cluster dele ao
/// volume.
fn cmd_rmdir(rest: &str) {
    let Some(path) = single_path("rmdir", rest) else { return };
    match fs::rmdir(path.as_bytes()) {
        Ok(()) => say(format_args!("rmdir: removido {}", path)),
        Err(error) => report_fs_error("rmdir", path, error),
    }
}

/// Único ponto que mostra como um programa terminou (`SYSCALLS.md`,
/// seção 7): o kernel (`user.rs`, `syscall.rs`, `interrupts.rs`) só
/// devolve o motivo, e este comando o transforma em texto. Um término com
/// código 0 é o caso normal e não escreve nada na tela, só na serial.
fn report(name: &str, termination: &Termination) {
    match *termination {
        Termination::Exit { code: 0 } => {
            serial_println!("[run] {} terminou com codigo 0", name);
        }
        Termination::Exit { code } => {
            say(format_args!("[run] {} terminou com codigo {}", name, code));
        }
        Termination::BadSyscall { number } => {
            say(format_args!(
                "[run] {} encerrado: syscall inexistente ({})",
                name, number
            ));
        }
        Termination::Fault {
            mnemonic,
            name: exception,
            rip,
            error_code,
            fault_address,
        } => {
            // Um `#PF` é um acesso inválido à memória: a mensagem diz isso em
            // palavras (`SYSCALLS.md`, seção 7). As demais exceções mantêm o
            // texto do Marco 5.
            let kind = if mnemonic == "#PF" {
                "erro de memoria"
            } else {
                "erro"
            };
            say(format_args!(
                "[run] {} encerrado por {}: {} ({}) em {:#x}",
                name, kind, mnemonic, exception, rip
            ));
            if let Some(code) = error_code {
                say(format_args!("codigo de erro: {:#x}", code));
            }
            if let Some(address) = fault_address {
                say(format_args!("endereco de falha: {:#x}", address));
            }
        }
    }
}

/// Chamado pelo escalonador no instante em que cada programa termina: mostra
/// a mensagem na hora, antes de os outros programas acabarem.
fn report_finished(finished: &Finished) {
    report(&finished.name, &finished.termination);
}

/// De onde vem a imagem de um programa de `run`: embutida no kernel ou lida
/// de um arquivo (e então dona de um `Vec` no heap, que vive até o fim do `run`).
enum Source {
    Builtin(&'static [u8]),
    File(Vec<u8>),
}

impl Source {
    fn bytes(&self) -> &[u8] {
        match self {
            Source::Builtin(image) => image,
            Source::File(data) => data,
        }
    }
}

/// O nome mostrado nas mensagens de um programa lido de `path`: o nome do
/// arquivo em minúsculas (`/disco/bin/VISITA` vira `visita`).
fn file_name_of(path: &str) -> String {
    path.rsplit('/').find(|part| !part.is_empty()).unwrap_or(path).to_ascii_lowercase()
}

/// Executa programas de usuário (`run <alvo> [<alvo>...]`) ao mesmo tempo,
/// cada um em modo usuário e na sua própria memória, e volta ao prompt quando
/// o **último** terminar. Um alvo que começa com `/` é o caminho de um
/// arquivo executável (lido inteiro do volume, até `MAX_EXEC_SIZE` bytes);
/// qualquer outro é o nome de um programa embutido. Os dois tipos se misturam.
///
/// Tudo ou nada: sem alvo, com um alvo que não existe (ou não pôde ser lido) ou
/// com mais programas que o máximo, o comando mostra o problema e não inicia
/// nenhum. Um arquivo que não é um ELF64 válido chega ao carregador, que o
/// recusa antes de mapear qualquer página. O mesmo alvo pode se repetir: são
/// instâncias independentes.
fn cmd_run(rest: &str) {
    let targets: Vec<&str> = rest.split_whitespace().collect();
    if targets.is_empty() {
        println!("run: informe o programa.");
        print_available_programs();
        return;
    }
    if targets.len() > MAX_TASKS {
        println!("run: no maximo {} programas ao mesmo tempo", MAX_TASKS);
        return;
    }
    // Resolve **todos** os alvos antes de iniciar qualquer um.
    let mut sources: Vec<(String, Source)> = Vec::new();
    for target in &targets {
        if target.starts_with('/') {
            match fs::read_whole(target.as_bytes(), fs::MAX_EXEC_SIZE) {
                Ok(data) => sources.push((file_name_of(target), Source::File(data))),
                Err(error) => return report_fs_error("run", target, error),
            }
        } else {
            match user::builtin_image(target) {
                Some(image) => sources.push((String::from(*target), Source::Builtin(image))),
                None => {
                    println!("programa desconhecido: {}", target);
                    print_available_programs();
                    return;
                }
            }
        }
    }
    let images: Vec<(&str, &[u8])> = sources.iter().map(|(name, source)| (name.as_str(), source.bytes())).collect();
    match user::run_images(&images, Some(report_finished)) {
        Ok(_) => {}
        Err(RunError::LoadFailed { index, error }) => {
            say(format_args!("[run] erro ao carregar {}: {}", sources[index].0, error));
        }
        Err(RunError::Load(error)) => {
            say(format_args!("[run] erro ao carregar {}: {}", sources[0].0, error));
        }
        Err(RunError::UnknownProgram) => {
            println!("programa desconhecido");
            print_available_programs();
        }
        Err(RunError::TooManyPrograms { max }) => {
            println!("run: no maximo {} programas ao mesmo tempo", max);
        }
    }
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
    // fica por conta da validação manual no QEMU e da leitura do código
    // acima.

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
    fn comando_sobre_descreve_o_os_rust() {
        vga_buffer::clear_screen();
        execute("sobre");
        assert!(vga_buffer::screen_contains(NAME));
    }

    #[test_case]
    fn comando_mem_mostra_memoria_heap_box_e_vec() {
        vga_buffer::clear_screen();
        execute("mem");
        assert!(vga_buffer::screen_contains("memoria fisica utilizavel"));
        assert!(vga_buffer::screen_contains("heap"));
        assert!(vga_buffer::screen_contains("Box"));
        assert!(vga_buffer::screen_contains("Vec"));
    }

    // --- data ---

    fn t(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> DateTime {
        DateTime { year, month, day, hour, minute, second, _pad: 0 }
    }

    #[test_case]
    fn data_formata_com_zeros_a_esquerda_e_utc() {
        assert_eq!(format_clock(Ok(t(2026, 10, 8, 15, 4, 5))), "2026-10-08 15:04:05 UTC");
        assert_eq!(format_clock(Ok(t(2026, 1, 2, 3, 4, 5))), "2026-01-02 03:04:05 UTC");
    }

    #[test_case]
    fn data_com_relogio_ruim_mostra_mensagem_clara_sem_hora() {
        for (error, palavra) in [
            (RtcError::Invalid, "invalido"),
            (RtcError::Unstable, "instavel"),
            (RtcError::Unavailable, "indisponivel"),
        ] {
            let texto = format_clock(Err(error));
            assert!(texto.contains(palavra));
            assert!(!texto.contains("UTC"));
        }
    }

    #[test_case]
    fn comando_data_mostra_uma_linha_no_formato() {
        vga_buffer::clear_screen();
        execute("data");
        assert_eq!(vga_buffer::screen_count_clock_lines(), 1);
    }

    #[test_case]
    fn comando_data_ignora_argumentos_extras() {
        vga_buffer::clear_screen();
        execute("data xyz abc");
        assert_eq!(vga_buffer::screen_count_clock_lines(), 1);
    }

    #[test_case]
    fn help_lista_o_comando_data() {
        vga_buffer::clear_screen();
        execute("help");
        assert!(vga_buffer::screen_contains("data - mostra a data e a hora"));
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
        assert!(vga_buffer::screen_contains(NAME));
    }

    #[test_case]
    fn comando_sobre_mostra_a_versao() {
        vga_buffer::clear_screen();
        execute("sobre");
        assert!(vga_buffer::screen_contains(crate::VERSION));
    }

    #[test_case]
    fn comando_falha_sem_argumento_lista_tipos() {
        vga_buffer::clear_screen();
        execute("falha");
        assert!(vga_buffer::screen_contains("tipos disponiveis"));
    }

    #[test_case]
    fn comando_falha_tipo_desconhecido_lista_tipos() {
        vga_buffer::clear_screen();
        execute("falha xyz");
        assert!(vga_buffer::screen_contains("tipos disponiveis"));
    }

    #[test_case]
    fn run_sem_argumento_lista_os_programas() {
        vga_buffer::clear_screen();
        execute("run");
        assert!(vga_buffer::screen_contains("run: informe o programa"));
        assert!(vga_buffer::screen_contains("hello"));
        assert!(vga_buffer::screen_contains("eco"));
        assert!(vga_buffer::screen_contains("falha_memoria"));
    }

    #[test_case]
    fn run_com_nome_inexistente_mostra_erro_e_lista() {
        vga_buffer::clear_screen();
        execute("run xpto");
        assert!(vga_buffer::screen_contains("programa desconhecido: xpto"));
        assert!(vga_buffer::screen_contains("disponiveis: "));
        assert!(vga_buffer::screen_contains("hello"));
        assert!(vga_buffer::screen_contains("eco"));
        assert!(vga_buffer::screen_contains("falha_memoria"));
    }

    #[test_case]
    fn run_falha_memoria_mostra_erro_de_memoria_e_o_kernel_segue_vivo() {
        // Ao contrário de `falha pagina`, `run falha_memoria` não é fatal: só o
        // programa é encerrado.
        vga_buffer::clear_screen();
        execute("run falha_memoria");
        assert!(vga_buffer::screen_contains("encerrado por erro de memoria"));
        assert!(vga_buffer::screen_contains("0xdeadbeef"));

        // Outra exceção de programa mantém o texto do Marco 5, sem "de memoria".
        vga_buffer::clear_screen();
        execute("run crash");
        assert!(vga_buffer::screen_contains("encerrado por erro:"));
        assert!(!vga_buffer::screen_contains("erro de memoria"));
        assert!(vga_buffer::screen_contains("#UD"));

        // O kernel segue vivo: o próximo programa roda.
        vga_buffer::clear_screen();
        execute("run hello");
        assert!(vga_buffer::screen_contains("Ola do ring 3!"));
    }

    #[test_case]
    fn comando_falha_breakpoint_retorna_ao_prompt() {
        vga_buffer::clear_screen();
        execute("falha breakpoint");
        assert!(vga_buffer::screen_contains("#BP"));

        // O controle voltou normalmente ao chamador: um comando seguinte
        // continua funcionando.
        vga_buffer::clear_screen();
        execute("sobre");
        assert!(vga_buffer::screen_contains(NAME));
    }

    // --- ls e cat (Marco 8) ---

    /// O conteúdo de `discos/ram/ola.txt`, sem o `\n` final: a tela mostra a
    /// linha, e `screen_contains` procura o texto dentro de uma linha.
    const OLA: &str = include_str!("../discos/ram/ola.txt");

    #[test_case]
    fn ls_mostra_tipo_tamanho_e_nome_da_raiz_do_ramdisk() {
        vga_buffer::clear_screen();
        execute("ls /ram");
        assert!(vga_buffer::screen_contains("ola.txt"));
        assert!(vga_buffer::screen_contains("docs"));
        assert!(vga_buffer::screen_contains("dir"));
        assert!(vga_buffer::screen_contains("arquivo"));
        // O tamanho de `ola.txt` aparece na mesma linha que o nome.
        assert!(vga_buffer::screen_contains(&alloc::format!("{:>8}  ola.txt", OLA.len())));
        assert!(vga_buffer::screen_contains(&alloc::format!("{:>8}  docs", "-")));
    }

    #[test_case]
    fn ls_de_subdiretorio_mostra_as_entradas_dele() {
        vga_buffer::clear_screen();
        execute("ls /ram/docs");
        assert!(vga_buffer::screen_contains("sobre.txt"));
        assert!(!vga_buffer::screen_contains("ola.txt"));
    }

    #[test_case]
    fn cat_mostra_o_conteudo_exato_do_arquivo() {
        vga_buffer::clear_screen();
        execute("cat /ram/ola.txt");
        assert!(vga_buffer::screen_contains(OLA.trim_end()));
        vga_buffer::clear_screen();
        execute("cat /RAM/OLA.TXT");
        assert!(vga_buffer::screen_contains(OLA.trim_end()));
    }

    #[test_case]
    fn cat_de_arquivo_vazio_nao_mostra_nada_e_nao_da_erro() {
        vga_buffer::clear_screen();
        execute("cat /ram/vazio.txt");
        assert!(!vga_buffer::screen_contains("cat:"));
        assert!(!vga_buffer::screen_contains("erro"));
    }

    #[test_case]
    fn ls_e_cat_sem_argumento_mostram_o_uso_e_os_volumes() {
        vga_buffer::clear_screen();
        execute("ls");
        assert!(vga_buffer::screen_contains("uso: ls <caminho>"));
        assert!(vga_buffer::screen_contains("volumes: /ram (ok)"));
        vga_buffer::clear_screen();
        execute("cat");
        assert!(vga_buffer::screen_contains("uso: cat <caminho>"));
        assert!(vga_buffer::screen_contains("volumes:"));
    }

    #[test_case]
    fn ls_e_cat_mostram_cada_tipo_de_erro() {
        vga_buffer::clear_screen();
        execute("ls /ram/nada");
        assert!(vga_buffer::screen_contains("ls: nao encontrado: /ram/nada"));
        vga_buffer::clear_screen();
        execute("ls /ram/ola.txt");
        assert!(vga_buffer::screen_contains("ls: nao e um diretorio: /ram/ola.txt"));
        vga_buffer::clear_screen();
        execute("cat /ram/docs");
        assert!(vga_buffer::screen_contains("cat: e um diretorio: /ram/docs"));
        vga_buffer::clear_screen();
        execute("cat /xyz/a");
        assert!(vga_buffer::screen_contains("cat: volume desconhecido: /xyz/a"));
        assert!(vga_buffer::screen_contains("volumes:"));
        vga_buffer::clear_screen();
        execute("ls ram");
        assert!(vga_buffer::screen_contains("ls: caminho invalido: ram"));
        vga_buffer::clear_screen();
        execute("ls /ram/..");
        assert!(vga_buffer::screen_contains("ls: caminho invalido: /ram/.."));
        vga_buffer::clear_screen();
        execute("cat /ram/ola.txt/x");
        assert!(vga_buffer::screen_contains("cat: nao e um diretorio: /ram/ola.txt/x"));
    }

    #[test_case]
    fn caminho_longo_demais_e_recusado() {
        let mut line = alloc::string::String::from("ls /ram/");
        for _ in 0..70 {
            line.push('a');
        }
        vga_buffer::clear_screen();
        execute(&line);
        assert!(vga_buffer::screen_contains("ls: caminho longo demais (maximo 64 bytes, 8 niveis)"));
    }

    #[test_case]
    fn help_lista_ls_e_cat_e_o_prompt_segue_respondendo() {
        vga_buffer::clear_screen();
        execute("help");
        assert!(vga_buffer::screen_contains("ls - lista um diretorio"));
        assert!(vga_buffer::screen_contains("cat - mostra o conteudo"));
        // Depois de um erro, o próximo comando responde normalmente.
        execute("cat /ram/nada");
        vga_buffer::clear_screen();
        execute("echo oi");
        assert!(vga_buffer::screen_contains("oi"));
    }

    // NÃO testar execute("falha pagina"), execute("falha pilha"),
    // execute("falha opcode") ou execute("falha protecao") aqui: são
    // exceções fatais de verdade — o kernel para em `halt_loop()` — e um
    // `#[test_case]` que nunca retorna trava o binário de teste inteiro
    // até o tempo máximo de execução esgotar, o mesmo cuidado já tomado
    // para o comando `panic` (ver comentário acima, seção "Interpretação
    // de comandos"). A cobertura fica por conta da validação manual no
    // QEMU e dos testes de integração dedicados (`tests/page_fault.rs`,
    // `tests/double_fault.rs`).

    // --- grava, anexa e apaga (Marco 10) ---

    /// Roda `f` com o `/ram` no estado inicial e o devolve a ele, conferindo
    /// que os clusters livres e a raiz voltaram ao que eram.
    fn com_ram_limpo(f: impl FnOnce()) {
        fs::reset_ramdisk();
        let livres = fs::free_clusters(VolumeId::Ram).unwrap();
        let raiz = {
            vga_buffer::clear_screen();
            execute("ls /ram");
            vga_buffer::screen_contains("ola.txt")
        };
        assert!(raiz);
        f();
        fs::reset_ramdisk();
        assert_eq!(fs::free_clusters(VolumeId::Ram).unwrap(), livres);
    }

    fn tela(comando: &str) -> bool {
        vga_buffer::clear_screen();
        execute(comando);
        true
    }

    #[test_case]
    fn grava_cat_e_apaga_mostram_as_linhas_da_historia_1() {
        com_ram_limpo(|| {
            let livres = fs::free_clusters(VolumeId::Ram).unwrap();
            tela("grava /ram/nota.txt ola, ramdisk");
            assert!(vga_buffer::screen_contains("grava: criado /ram/nota.txt (13 bytes)"));
            tela("cat /ram/nota.txt");
            assert!(vga_buffer::screen_contains("ola, ramdisk"));
            tela("ls /ram");
            assert!(vga_buffer::screen_contains(&alloc::format!("{:>8}  nota.txt", 13)));
            tela("apaga /ram/nota.txt");
            assert!(vga_buffer::screen_contains("apaga: removido /ram/nota.txt"));
            tela("cat /ram/nota.txt");
            assert!(vga_buffer::screen_contains("cat: nao encontrado: /ram/nota.txt"));
            assert_eq!(fs::free_clusters(VolumeId::Ram).unwrap(), livres);
        });
    }

    #[test_case]
    fn grava_sem_texto_cria_arquivo_de_zero_bytes() {
        com_ram_limpo(|| {
            tela("grava /ram/novo.txt");
            assert!(vga_buffer::screen_contains("grava: criado /ram/novo.txt (0 bytes)"));
            tela("ls /ram");
            assert!(vga_buffer::screen_contains(&alloc::format!("{:>8}  novo.txt", 0)));
        });
    }

    #[test_case]
    fn grava_em_subdiretorio_existente() {
        com_ram_limpo(|| {
            tela("grava /ram/docs/nota.txt oi");
            assert!(vga_buffer::screen_contains("grava: criado /ram/docs/nota.txt (3 bytes)"));
            tela("ls /ram/docs");
            assert!(vga_buffer::screen_contains("nota.txt"));
            assert!(vga_buffer::screen_contains("sobre.txt"));
        });
    }

    #[test_case]
    fn grava_em_nome_existente_nao_sobrescreve_e_ignora_maiusculas() {
        com_ram_limpo(|| {
            tela("grava /ram/nota.txt primeiro");
            tela("grava /RAM/Nota.TXT x");
            assert!(vga_buffer::screen_contains("grava: ja existe: /RAM/Nota.TXT"));
            tela("cat /ram/nota.txt");
            assert!(vga_buffer::screen_contains("primeiro"));
            tela("grava /ram/docs x");
            assert!(vga_buffer::screen_contains("grava: ja existe: /ram/docs"));
            tela("grava /ram x");
            assert!(vga_buffer::screen_contains("grava: ja existe: /ram"));
        });
    }

    #[test_case]
    fn grava_preserva_espacos_internos_e_corta_os_iniciais() {
        com_ram_limpo(|| {
            tela("grava /ram/e.txt    a  b   c");
            assert!(vga_buffer::screen_contains("(9 bytes)"));
            tela("cat /ram/e.txt");
            assert!(vga_buffer::screen_contains("a  b   c"));
        });
    }

    #[test_case]
    fn anexa_amplia_o_arquivo_e_atravessa_a_fronteira_de_um_cluster() {
        com_ram_limpo(|| {
            tela("grava /ram/nota.txt ola, ramdisk");
            tela("anexa /ram/nota.txt segunda linha");
            assert!(vga_buffer::screen_contains("anexa: /ram/nota.txt agora tem 27 bytes"));
            tela("cat /ram/nota.txt");
            assert!(vga_buffer::screen_contains("ola, ramdisk"));
            assert!(vga_buffer::screen_contains("segunda linha"));
            // Passa de 512 bytes: um segundo cluster.
            for _ in 0..8 {
                execute("anexa /ram/nota.txt 0123456789012345678901234567890123456789012345678901234567890123456789");
            }
            tela("ls /ram");
            assert!(vga_buffer::screen_contains(&alloc::format!("{:>8}  nota.txt", 27 + 8 * 71)));
            let (_, bytes) = {
                let dados = fs::read_whole(b"/ram/nota.txt", 4096).unwrap();
                ((), dados)
            };
            assert_eq!(bytes.len(), 27 + 8 * 71);
            assert!(bytes.starts_with(b"ola, ramdisk\nsegunda linha\n"));
            assert!(bytes.ends_with(b"0123456789\n"));
        });
    }

    #[test_case]
    fn anexa_nao_cria_e_recusa_diretorio_e_raiz() {
        com_ram_limpo(|| {
            tela("anexa /ram/nada.txt x");
            assert!(vga_buffer::screen_contains("anexa: nao encontrado: /ram/nada.txt"));
            tela("ls /ram");
            assert!(!vga_buffer::screen_contains("nada.txt"));
            tela("anexa /ram/docs x");
            assert!(vga_buffer::screen_contains("anexa: e um diretorio: /ram/docs"));
            tela("anexa /ram x");
            assert!(vga_buffer::screen_contains("anexa: e um diretorio: /ram"));
        });
    }

    #[test_case]
    fn apaga_recusa_diretorio_raiz_e_inexistente() {
        com_ram_limpo(|| {
            tela("apaga /ram/docs");
            assert!(vga_buffer::screen_contains("apaga: e um diretorio: /ram/docs"));
            tela("apaga /ram");
            assert!(vga_buffer::screen_contains("apaga: e um diretorio: /ram"));
            tela("apaga /ram/nada.txt");
            assert!(vga_buffer::screen_contains("apaga: nao encontrado: /ram/nada.txt"));
        });
    }

    #[test_case]
    fn escrita_no_disco_funciona_e_volume_desconhecido_e_reportado() {
        if fs::volume_status(VolumeId::Disco).is_ok() {
            // O `/disco` agora grava (o QEMU dos testes usa `snapshot=on`);
            // o teste apaga o que criou e confere os clusters livres.
            let livres = fs::free_clusters(VolumeId::Disco).unwrap();
            tela("grava /disco/x.txt a");
            assert!(vga_buffer::screen_contains("grava: criado /disco/x.txt (2 bytes)"));
            tela("anexa /disco/x.txt b");
            assert!(vga_buffer::screen_contains("anexa: /disco/x.txt agora tem 4 bytes"));
            tela("apaga /disco/x.txt");
            assert!(vga_buffer::screen_contains("apaga: removido /disco/x.txt"));
            assert_eq!(fs::free_clusters(VolumeId::Disco).unwrap(), livres);
        } else {
            for comando in ["grava /disco/x.txt a", "anexa /disco/leiame.txt a", "apaga /disco/leiame.txt"] {
                tela(comando);
                assert!(vga_buffer::screen_contains("volume indisponivel: /disco"));
            }
        }
        tela("grava /xyz/a x");
        assert!(vga_buffer::screen_contains("grava: volume desconhecido: /xyz/a"));
        tela("apaga ram/x");
        assert!(vga_buffer::screen_contains("apaga: caminho invalido: ram/x"));
    }

    #[test_case]
    fn comandos_de_escrita_sem_argumentos_mostram_o_uso_e_os_volumes() {
        for (comando, uso) in [
            ("grava", "uso: grava <caminho> [<texto>]"),
            ("anexa", "uso: anexa <caminho> <texto>"),
            ("anexa /ram/ola.txt", "uso: anexa <caminho> <texto>"),
            ("apaga", "uso: apaga <caminho>"),
        ] {
            tela(comando);
            assert!(vga_buffer::screen_contains(uso));
            assert!(vga_buffer::screen_contains("volumes: /ram (ok)"));
        }
    }

    #[test_case]
    fn apaga_arquivo_em_uso_mostra_a_mensagem_e_o_prompt_segue_respondendo() {
        com_ram_limpo(|| {
            let mut tabela = fs::FileTable::new();
            let fd = tabela.open(b"/ram/ola.txt").unwrap();
            tela("apaga /ram/ola.txt");
            assert!(vga_buffer::screen_contains("apaga: arquivo em uso: /ram/ola.txt"));
            tela("echo oi");
            assert!(vga_buffer::screen_contains("oi"));
            tabela.close(fd).unwrap();
        });
    }

    #[test_case]
    fn volume_cheio_no_meio_do_texto_mostra_sem_espaco_e_deixa_o_que_coube() {
        com_ram_limpo(|| {
            // Enche o volume até sobrar um cluster livre depois de o último
            // cluster de `enche.bin` ficar com 3 bytes de folga.
            let mut enche = fs::OpenFile::create(b"/ram/enche.bin").unwrap();
            let bloco = [b'x'; 512];
            while fs::free_clusters(VolumeId::Ram).unwrap() > 2 {
                enche.write(&bloco).unwrap();
            }
            enche.write(&bloco[..509]).unwrap();
            let (_, antes) = fs::lookup(b"/ram/enche.bin").unwrap();
            // O último cluster livre vai para `cabe.txt`.
            tela("grava /ram/cabe.txt 0123456789");
            assert!(vga_buffer::screen_contains("grava: criado /ram/cabe.txt (11 bytes)"));
            assert_eq!(fs::free_clusters(VolumeId::Ram).unwrap(), 0);
            // Em `enche.bin` só cabem os 3 bytes que faltavam no cluster.
            tela("anexa /ram/enche.bin 0123456789");
            assert!(vga_buffer::screen_contains("sem espaco (volume ou diretorio cheio): /ram/enche.bin"));
            let (_, depois) = fs::lookup(b"/ram/enche.bin").unwrap();
            assert_eq!(depois.size, antes.size + 3);
            drop(enche);
            tela("apaga /ram/enche.bin");
            assert!(vga_buffer::screen_contains("apaga: removido /ram/enche.bin"));
        });
    }

    #[test_case]
    fn help_lista_grava_anexa_e_apaga() {
        tela("help");
        for (nome, trecho) in [("grava -", "/ram"), ("anexa -", "/ram"), ("apaga -", "/ram")] {
            assert!(vga_buffer::screen_contains(nome));
            let _ = trecho;
        }
        for (nome, descricao) in COMMANDS {
            if ["grava", "anexa", "apaga"].contains(nome) {
                assert!(descricao.contains("/ram"));
            }
        }
    }
}
