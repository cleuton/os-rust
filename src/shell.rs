//! Buffer de linha e prompt de comandos mínimo (`help`, `clear`, `echo`, `sobre`, `panic`, `mem`, `falha`, `ls`, `cat`, `run`, `data`).

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use abi::{DateTime, MAX_TASKS};

use crate::fat::Kind;
use crate::fs::{self, FsError, OpenFile, VolumeId};
use crate::keyboard::LINE_CAPACITY;
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
    ("data", "mostra a data e a hora (UTC) do relogio"),
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
        "run" => cmd_run(rest),
        // Argumentos extras em `rest` são ignorados, como em `sobre`.
        "data" => cmd_data(),
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
}
