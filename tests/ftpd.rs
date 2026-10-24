//! Testes do servidor FTP `ftpd` (versão 0.12.1), de ponta a ponta dentro do
//! QEMU. O cliente FTP não é um programa: é um roteiro que roda no gancho do
//! escalonador (`scheduler::set_poll_hook`) e fala com o `ftpd` pelo loopback,
//! usando a camada de sockets do kernel (`net::sys_*`) sobre uma `SocketTable`
//! só dele. O gancho não pode esperar, então todo socket do cliente tem tempo
//! limite 0 e cada passo do roteiro tenta de novo na volta seguinte. O Enter da
//! pergunta inicial do `ftpd` é "digitado" empurrando scancodes na fila.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os_rust::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use abi::{ERR_CONNREFUSED, ERR_TIMEDOUT};
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use os_rust::net::socket::SocketTable;
use os_rust::net::{self, Outcome};
use os_rust::user::{self, Finished, RunReport, Termination};
use os_rust::{interrupts, keyboard, memory, scheduler, shell, timer, vga_buffer};
use spin::Mutex;

entry_point!(main);

fn main(boot_info: &'static BootInfo) -> ! {
    os_rust::init(boot_info);
    test_main();
    os_rust::panic::halt_loop();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    os_rust::test_panic_handler(info)
}

// ---------------------------------------------------------------------------
// Ajudantes de tela, teclado e pilha
// ---------------------------------------------------------------------------

fn reset_teclado() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        while interrupts::next_scancode().is_some() {}
    });
    keyboard::translate(0xAA);
    keyboard::translate(0xB6);
}

/// Empurra `texto` na fila de scancodes (letras minúsculas, dígitos, `.`,
/// espaço e Enter); a fila tem 16 posições.
fn empurrar(texto: &str) {
    let mut total = 0;
    for ch in texto.bytes() {
        let scancode = match ch {
            b'a'..=b'z' => {
                const LINHAS: [(&[u8], u8); 3] =
                    [(b"qwertyuiop", 0x10), (b"asdfghjkl", 0x1E), (b"zxcvbnm", 0x2C)];
                let mut achado = None;
                for (letras, base) in LINHAS {
                    if let Some(pos) = letras.iter().position(|&l| l == ch) {
                        achado = Some(base + pos as u8);
                    }
                }
                achado.expect("letra na tabela")
            }
            b'1'..=b'9' => 0x02 + (ch - b'1'),
            b'0' => 0x0B,
            b'.' => 0x34,
            b' ' => 0x39,
            b'\n' => 0x1C,
            _ => panic!("caractere sem scancode no teste: {:?}", ch as char),
        };
        interrupts::push_scancode(scancode);
        total += 1;
    }
    assert!(total <= 16, "texto gera {} scancodes; a fila tem 16", total);
}

fn digitar_no_prompt(linha: &str) {
    for byte in linha.bytes() {
        shell::feed(byte);
    }
    shell::feed(b'\n');
}

fn linhas_da_tela() -> Vec<String> {
    let mut linhas = Vec::new();
    for row in 0..25 {
        let texto: String = vga_buffer::screen_row_bytes(row)
            .iter()
            .map(|&b| if (0x20..=0x7e).contains(&b) { b as char } else { '?' })
            .collect();
        let texto = String::from(texto.trim_end());
        if !texto.is_empty() {
            linhas.push(texto);
        }
    }
    linhas
}

fn tem(linhas: &[String], texto: &str) -> bool {
    linhas.iter().any(|linha| linha.contains(texto))
}

fn exige(linhas: &[String], texto: &str) {
    assert!(tem(linhas, texto), "a tela nao tem {:?}: {:?}", texto, linhas);
}

fn rodar_comando(comando: &str) -> Vec<String> {
    vga_buffer::clear_screen();
    digitar_no_prompt(comando);
    linhas_da_tela()
}

fn termino_de<'a>(relatorio: &'a RunReport, nome: &str) -> Vec<&'a Finished> {
    relatorio.finished.iter().filter(|f| f.name == nome).collect()
}

fn saiu_com(f: &Finished, codigo: u64) -> bool {
    f.termination == Termination::Exit { code: codigo }
}

/// Espera (com a pilha andando) até a tabela de conexões esvaziar, no máximo
/// `limite_ticks`. Devolve quantos blocos restam.
fn esperar_pilha_vazia(limite_ticks: u64) -> usize {
    let fim = timer::ticks() + limite_ticks;
    loop {
        x86_64::instructions::interrupts::disable();
        net::poll();
        let em_uso = net::tcp_in_use();
        if em_uso == 0 || timer::ticks() >= fim {
            x86_64::instructions::interrupts::enable();
            return em_uso;
        }
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

fn preparar() -> usize {
    reset_teclado();
    vga_buffer::clear_screen();
    memory::frames_outstanding()
}

/// O fim de um teste comum: nenhum frame preso. Esperar a pilha esvaziar custa
/// 1 s de `TIME_WAIT` por teste, e a suite tem de caber nos 60 s: so os testes
/// que mexem com queda e com muitas sessoes usam `conferir_fim`.
fn conferir_frames(frames: usize) {
    assert_eq!(memory::frames_outstanding(), frames, "frames presos");
}

/// O fim de um teste que podia prender algo: pilha vazia e nenhum frame preso.
fn conferir_fim(frames: usize) {
    assert_eq!(esperar_pilha_vazia(300), 0, "conexoes presas");
    conferir_frames(frames);
}

// ---------------------------------------------------------------------------
// O cliente FTP: um roteiro avançado pelo gancho do escalonador
// ---------------------------------------------------------------------------

/// Junta pedaços de roteiro.
macro_rules! roteiro {
    ($($parte:expr),* $(,)?) => {{
        let mut v: Vec<Passo> = Vec::new();
        $( v.extend($parte); )*
        v
    }};
}

/// Um passo do roteiro.
enum Passo {
    /// Já executado (ou em execução): o lugar fica ocupado enquanto o passo
    /// está emprestado ao laço.
    Vazio,
    /// Conecta no controle (porta 21); tenta de novo se ainda ninguém ouve.
    Conecta,
    /// Envia bytes pelo controle.
    Envia(Vec<u8>),
    /// A próxima resposta do controle tem de começar por este texto.
    Espera(&'static str),
    /// O servidor fechou o controle.
    EsperaFim,
    /// Conecta nos dados, na porta do último `227` ou `229`.
    AbreDados,
    /// Lê a conexão de dados até o servidor fechar e guarda os bytes.
    LeDados,
    /// Envia bytes pelos dados e fecha a conexão.
    EnviaDados(Vec<u8>),
    /// Envia os bytes da transferência de número `n` (já lida) pelos dados e fecha.
    EnviaDadosDe(usize),
    /// Envia bytes pelos dados e **não** fecha.
    EnviaSemFechar(Vec<u8>),
    FechaDados,
    FechaControle,
    /// Não faz nada por tantos ticks.
    Dorme(u64),
}

/// O cliente e o que ele viu.
struct Cliente {
    passos: Vec<Passo>,
    atual: usize,
    tabela: SocketTable,
    controle: Option<usize>,
    dados: Option<usize>,
    pendente: Option<os_rust::net::socket::PendingNet>,
    entrada: Vec<u8>,
    fila: VecDeque<String>,
    controle_fechou: bool,
    enviados: usize,
    prazo: u64,
    atual_dados: Vec<u8>,
    limite: u64,
    terminou: bool,
    /// Todas as respostas do controle, em ordem, sem o `\r\n`.
    linhas: Vec<String>,
    /// Os bytes de cada conexão de dados lida (`LeDados`), em ordem.
    transferencias: Vec<Vec<u8>>,
    /// O tick em que cada passo terminou.
    ticks: Vec<u64>,
    erro: Option<String>,
    /// Depois de um erro o roteiro troca de lugar com um epilogo que manda
    /// `SITE EXIT`: sem ele o `ftpd` esperaria 30 s por outro cliente.
    epilogo: bool,
}

static CLIENTE: Mutex<Option<Cliente>> = Mutex::new(None);

/// O gancho: avança o roteiro um pouco.
fn tick() {
    let mut guarda = CLIENTE.lock();
    if let Some(cliente) = guarda.as_mut() {
        cliente.avancar(timer::ticks());
    }
}

enum Lado {
    Controle,
    Dados,
}

impl Cliente {
    fn novo(passos: Vec<Passo>, limite: u64) -> Cliente {
        Cliente {
            passos,
            atual: 0,
            tabela: SocketTable::new(),
            controle: None,
            dados: None,
            pendente: None,
            entrada: Vec::new(),
            fila: VecDeque::new(),
            controle_fechou: false,
            enviados: 0,
            prazo: 0,
            atual_dados: Vec::new(),
            limite,
            terminou: false,
            linhas: Vec::new(),
            transferencias: Vec::new(),
            ticks: Vec::new(),
            erro: None,
            epilogo: false,
        }
    }

    fn falhar(&mut self, motivo: String) {
        if self.erro.is_none() && !self.epilogo {
            let erro = format!("passo {}: {}", self.atual, motivo);
            os_rust::serial_println!("[ftpd teste] {}", erro);
            self.erro = Some(erro);
        }
    }

    /// Troca o roteiro por um que encerra o `ftpd` sem fazer mais perguntas.
    fn iniciar_epilogo(&mut self, agora: u64) {
        for sock in [self.controle.take(), self.dados.take()].into_iter().flatten() {
            net::sys_close(&mut self.tabela, sock as u64);
        }
        if let Some(p) = self.pendente.take() {
            net::sys_close(&mut self.tabela, p.sock as u64);
        }
        self.passos = roteiro![login(), par("SITE EXIT", "221"), vec![Passo::FechaControle]];
        self.atual = 0;
        self.enviados = 0;
        self.prazo = 0;
        self.epilogo = true;
        self.limite = agora + 1500;
    }

    /// Fecha o que sobrou aberto e para o roteiro.
    fn terminar(&mut self) {
        for sock in [self.controle.take(), self.dados.take()].into_iter().flatten() {
            net::sys_close(&mut self.tabela, sock as u64);
        }
        if let Some(p) = self.pendente.take() {
            net::sys_close(&mut self.tabela, p.sock as u64);
        }
        self.terminou = true;
    }

    fn avancar(&mut self, agora: u64) {
        if self.terminou {
            return;
        }
        if agora > self.limite {
            if self.epilogo {
                self.terminar();
                return;
            }
            self.falhar(String::from("o roteiro passou do limite de ticks"));
        }
        loop {
            if self.erro.is_some() && !self.epilogo {
                self.iniciar_epilogo(agora);
                continue;
            }
            if self.atual >= self.passos.len() {
                self.terminar();
                return;
            }
            // Os bytes de uma transferência já lida viram um envio comum.
            if let Passo::EnviaDadosDe(n) = &self.passos[self.atual] {
                let bytes = self.transferencias[*n].clone();
                self.passos[self.atual] = Passo::EnviaDados(bytes);
            }
            let passo = core::mem::replace(&mut self.passos[self.atual], Passo::Vazio);
            let feito = self.executar(&passo, agora);
            self.passos[self.atual] = passo;
            if !feito {
                return;
            }
            self.ticks.push(agora);
            self.atual += 1;
            self.enviados = 0;
            self.prazo = 0;
        }
    }

    /// Tenta o passo; `true` se terminou (com sucesso ou com `erro` preenchido).
    fn executar(&mut self, passo: &Passo, agora: u64) -> bool {
        match passo {
            Passo::Vazio => true,
            Passo::Conecta => match self.conectar(21, agora) {
                None => false,
                Some(Ok(sock)) => {
                    self.controle = Some(sock);
                    self.entrada.clear();
                    self.fila.clear();
                    self.controle_fechou = false;
                    true
                }
                // Ainda ninguém ouve: o `ftpd` pode estar na pergunta do endereço.
                Some(Err(codigo)) if codigo == ERR_CONNREFUSED => false,
                Some(Err(codigo)) => {
                    self.falhar(format!("connect devolveu {}", codigo));
                    true
                }
            },
            Passo::Envia(bytes) | Passo::EnviaSemFechar(bytes) | Passo::EnviaDados(bytes) => {
                let lado = match passo {
                    Passo::Envia(_) => Lado::Controle,
                    _ => Lado::Dados,
                };
                let feito = self.enviar(lado, bytes, agora);
                if feito && self.erro.is_none() {
                    if let Passo::EnviaDados(_) = passo {
                        if let Some(sock) = self.dados.take() {
                            net::sys_close(&mut self.tabela, sock as u64);
                        }
                    }
                }
                feito
            }
            Passo::EnviaDadosDe(_) => true,
            Passo::Espera(prefixo) => {
                self.ler_controle(agora);
                match self.fila.pop_front() {
                    Some(linha) => {
                        self.linhas.push(linha.clone());
                        if !linha.starts_with(prefixo) {
                            self.falhar(format!("esperava {:?}, veio {:?}", prefixo, linha));
                        }
                        true
                    }
                    None if self.controle_fechou => {
                        self.falhar(format!("o controle fechou esperando {:?}", prefixo));
                        true
                    }
                    None => false,
                }
            }
            Passo::EsperaFim => {
                self.ler_controle(agora);
                self.controle_fechou
            }
            Passo::AbreDados => {
                let Some(porta) = self.linhas.iter().rev().find_map(|l| porta_do_pasv(l)) else {
                    self.falhar(String::from("nenhum 227 ou 229 para achar a porta"));
                    return true;
                };
                match self.conectar(porta, agora) {
                    None => false,
                    Some(Ok(sock)) => {
                        self.dados = Some(sock);
                        self.atual_dados.clear();
                        true
                    }
                    Some(Err(codigo)) => {
                        self.falhar(format!("connect de dados devolveu {}", codigo));
                        true
                    }
                }
            }
            Passo::LeDados => {
                let Some(sock) = self.dados else {
                    self.falhar(String::from("LeDados sem conexao de dados"));
                    return true;
                };
                let mut buf = vec![0u8; 2048];
                loop {
                    match net::sys_recv(&mut self.tabela, sock as u64, buf.as_mut_ptr() as u64, 2048, agora) {
                        Outcome::Done(n) if n > 0 => self.atual_dados.extend_from_slice(&buf[..n as usize]),
                        Outcome::Done(0) => {
                            let bytes = core::mem::take(&mut self.atual_dados);
                            self.transferencias.push(bytes);
                            if let Some(sock) = self.dados.take() {
                                net::sys_close(&mut self.tabela, sock as u64);
                            }
                            return true;
                        }
                        Outcome::Done(codigo) if codigo == ERR_TIMEDOUT => return false,
                        Outcome::Done(codigo) => {
                            self.falhar(format!("recv de dados devolveu {}", codigo));
                            return true;
                        }
                        Outcome::Block(_) => return false,
                    }
                }
            }
            Passo::FechaDados => {
                if let Some(sock) = self.dados.take() {
                    net::sys_close(&mut self.tabela, sock as u64);
                }
                true
            }
            Passo::FechaControle => {
                if let Some(sock) = self.controle.take() {
                    net::sys_close(&mut self.tabela, sock as u64);
                }
                true
            }
            Passo::Dorme(ticks) => {
                if self.prazo == 0 {
                    self.prazo = agora + ticks;
                }
                agora >= self.prazo
            }
        }
    }

    /// Conecta em `127.0.0.1:porta`. `None` enquanto espera; `Some` com o
    /// número do socket (já com tempo limite 0) ou com o código de erro.
    fn conectar(&mut self, porta: u16, agora: u64) -> Option<Result<usize, i64>> {
        let resultado = match self.pendente.take() {
            Some(pedido) => match net::attempt(&mut self.tabela, &pedido, agora) {
                Some(resultado) => resultado,
                None => {
                    self.pendente = Some(pedido);
                    return None;
                }
            },
            None => match net::sys_connect(&mut self.tabela, [127, 0, 0, 1], porta, agora) {
                Outcome::Done(resultado) => resultado,
                Outcome::Block(pedido) => {
                    self.pendente = Some(pedido);
                    return None;
                }
            },
        };
        if resultado >= 0 {
            net::sys_timeout(&mut self.tabela, resultado as u64, 0);
            Some(Ok(resultado as usize))
        } else {
            Some(Err(resultado))
        }
    }

    fn enviar(&mut self, lado: Lado, bytes: &[u8], agora: u64) -> bool {
        let sock = match lado {
            Lado::Controle => self.controle,
            Lado::Dados => self.dados,
        };
        let Some(sock) = sock else {
            self.falhar(String::from("envio sem socket aberto"));
            return true;
        };
        loop {
            if self.enviados >= bytes.len() {
                return true;
            }
            let resto = &bytes[self.enviados..];
            let n = resto.len().min(4096);
            match net::sys_send(&mut self.tabela, sock as u64, resto.as_ptr() as u64, n as u64, agora) {
                Outcome::Done(r) if r > 0 => self.enviados += r as usize,
                Outcome::Done(r) if r == ERR_TIMEDOUT => return false,
                Outcome::Done(r) => {
                    self.falhar(format!("send devolveu {}", r));
                    return true;
                }
                Outcome::Block(_) => return false,
            }
        }
    }

    /// Lê o que chegou no controle e separa em linhas.
    fn ler_controle(&mut self, agora: u64) {
        let Some(sock) = self.controle else { return };
        let mut buf = vec![0u8; 512];
        loop {
            match net::sys_recv(&mut self.tabela, sock as u64, buf.as_mut_ptr() as u64, 512, agora) {
                Outcome::Done(n) if n > 0 => self.entrada.extend_from_slice(&buf[..n as usize]),
                Outcome::Done(0) => {
                    self.controle_fechou = true;
                    break;
                }
                _ => break,
            }
        }
        while let Some(pos) = self.entrada.iter().position(|&b| b == b'\n') {
            let linha: Vec<u8> = self.entrada.drain(..=pos).collect();
            let texto = String::from_utf8_lossy(&linha);
            self.fila.push_back(String::from(texto.trim_end_matches(['\r', '\n'])));
        }
    }
}

/// A porta de uma resposta `227 ... (a,b,c,d,p1,p2)` ou `229 ... (|||porta|)`.
fn porta_do_pasv(linha: &str) -> Option<u16> {
    let dentro = linha.split_once('(')?.1.split_once(')')?.0;
    if linha.starts_with("229") {
        return dentro.trim_matches('|').parse().ok();
    }
    if !linha.starts_with("227") {
        return None;
    }
    let numeros: Vec<u16> = dentro.split(',').filter_map(|n| n.parse().ok()).collect();
    if numeros.len() != 6 {
        return None;
    }
    Some(numeros[4] * 256 + numeros[5])
}

// ---------------------------------------------------------------------------
// Construção de roteiros
// ---------------------------------------------------------------------------

fn cmd(texto: &str) -> Passo {
    Passo::Envia(format!("{}\r\n", texto).into_bytes())
}

/// Um comando e a resposta que se espera (pelo prefixo).
fn par(comando: &str, resposta: &'static str) -> Vec<Passo> {
    vec![cmd(comando), Passo::Espera(resposta)]
}

fn login() -> Vec<Passo> {
    let mut v = vec![Passo::Conecta, Passo::Espera("220")];
    v.extend(par("USER aluno", "331"));
    v.extend(par("PASS qualquer", "230"));
    v
}

fn fim() -> Vec<Passo> {
    let mut v = par("SITE EXIT", "221");
    v.push(Passo::FechaControle);
    v
}

fn pasv() -> Vec<Passo> {
    par("PASV", "227")
}

/// `PASV`, o comando de dados, a conexão, e o `150`, os bytes e o `226`.
fn baixar(comando: &str) -> Vec<Passo> {
    let mut v = pasv();
    v.push(cmd(comando));
    v.extend([Passo::AbreDados, Passo::Espera("150"), Passo::LeDados, Passo::Espera("226")]);
    v
}

/// `PASV`, `STOR`, a conexão, o `150`, os bytes, o fim da conexão e o `226`.
fn enviar(comando: &str, bytes: Vec<u8>) -> Vec<Passo> {
    let mut v = pasv();
    v.push(cmd(comando));
    v.extend([Passo::AbreDados, Passo::Espera("150"), Passo::EnviaDados(bytes), Passo::Espera("226")]);
    v
}

/// Os bytes de um arquivo de teste: um padrão que muda a cada byte.
fn padrao(tamanho: usize) -> Vec<u8> {
    (0..tamanho).map(|i| (i % 251) as u8).collect()
}

/// Roda o `ftpd` (e outros programas, se pedidos) com o roteiro no gancho.
fn executar_com(programas: &[&str], passos: Vec<Passo>, limite_ticks: u64) -> (RunReport, Cliente) {
    *CLIENTE.lock() = Some(Cliente::novo(passos, timer::ticks() + limite_ticks));
    scheduler::set_poll_hook(Some(tick));
    let relatorio = user::run_all(programas, None);
    scheduler::set_poll_hook(None);
    let relatorio = relatorio.expect("run ftpd");
    let mut cliente = CLIENTE.lock().take().expect("cliente");
    // O gancho já fechou o que sobrou; a tabela do cliente tem de estar vazia.
    cliente.terminar();
    assert_eq!(cliente.tabela.count(), 0, "o cliente deixou sockets abertos");
    (relatorio, cliente)
}

/// Roda só o `ftpd` com o roteiro (limite de 20 s).
fn executar(passos: Vec<Passo>) -> (RunReport, Cliente) {
    executar_com(&["ftpd"], passos, 2000)
}

/// O roteiro terminou sem erro, o `ftpd` saiu com 0 e a pilha esvaziou.
fn conferir_ok(relatorio: &RunReport, cliente: &Cliente) {
    assert!(cliente.erro.is_none(), "{:?}; respostas: {:?}", cliente.erro, cliente.linhas);
    assert_eq!(cliente.atual, cliente.passos.len(), "o roteiro nao chegou ao fim");
    let ftpd = termino_de(relatorio, "ftpd");
    assert!(ftpd.len() == 1 && saiu_com(ftpd[0], 0), "{:?}", relatorio.finished);
}

fn ha(cliente: &Cliente, linha: &str) {
    assert!(
        cliente.linhas.iter().any(|l| l == linha),
        "falta {:?} nas respostas: {:?}",
        linha,
        cliente.linhas
    );
}

fn nao_ha(cliente: &Cliente, prefixo: &str) {
    assert!(
        !cliente.linhas.iter().any(|l| l.starts_with(prefixo)),
        "nao devia haver {:?} nas respostas: {:?}",
        prefixo,
        cliente.linhas
    );
}

// ---------------------------------------------------------------------------
// Início, login e comandos simples
// ---------------------------------------------------------------------------

#[test_case]
fn o_cliente_de_teste_fala_com_o_ftpd_pelo_loopback() {
    let frames = preparar();
    empurrar("\n");
    let (relatorio, cliente) = executar(roteiro![login(), fim()]);
    conferir_ok(&relatorio, &cliente);
    assert_eq!(
        cliente.linhas,
        ["220 os-rust ftpd pronto", "331 envie a senha", "230 login aceito", "221 encerrando o servidor"]
    );
    let tela = linhas_da_tela();
    exige(&tela, "ftpd: endereco a anunciar no modo passivo (vazio = 127.0.0.1):");
    exige(&tela, "ftpd: encerrado por SITE EXIT");
    conferir_frames(frames);
}

fn texto(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).expect("a transferencia nao e texto")
}

const OLA_TXT: &[u8] = include_bytes!("../discos/ram/ola.txt");
const LINHA_RAM: &str = "drwxr-xr-x 1 os-rust os-rust 0 Jan  1  1980 ram\r\n";
const LINHA_DISCO: &str = "drwxr-xr-x 1 os-rust os-rust 0 Jan  1  1980 disco\r\n";

#[test_case]
fn o_ftpd_pergunta_o_endereco_e_anuncia_o_padrao_com_enter() {
    let frames = preparar();
    empurrar("\n");
    let (relatorio, cliente) = executar(roteiro![login(), pasv(), fim()]);
    conferir_ok(&relatorio, &cliente);
    let tela = linhas_da_tela();
    exige(&tela, "ftpd: endereco a anunciar no modo passivo (vazio = 127.0.0.1):");
    exige(&tela, "ftpd: ouvindo na porta 21 (dados na porta 2100), anunciando 127.0.0.1");
    exige(&tela, "ftpd: cliente conectado");
    exige(&tela, "ftpd: sessao encerrada");
    ha(&cliente, "227 Entering Passive Mode (127,0,0,1,8,52)");
    conferir_frames(frames);
}

#[test_case]
fn o_ftpd_anuncia_o_endereco_digitado() {
    let frames = preparar();
    empurrar("10.0.2.15\n");
    let (relatorio, cliente) = executar(roteiro![login(), pasv(), par("EPSV", "229"), fim()]);
    conferir_ok(&relatorio, &cliente);
    exige(&linhas_da_tela(), "ftpd: ouvindo na porta 21 (dados na porta 2100), anunciando 10.0.2.15");
    ha(&cliente, "227 Entering Passive Mode (10,0,2,15,8,52)");
    ha(&cliente, "229 Entering Extended Passive Mode (|||2100|)");
    conferir_frames(frames);
}

#[test_case]
fn endereco_malformado_sai_com_codigo_2() {
    let frames = preparar();
    empurrar("abc\n");
    let relatorio = user::run_all(&["ftpd"], None).expect("run ftpd");
    let fim = termino_de(&relatorio, "ftpd");
    assert!(fim.len() == 1 && saiu_com(fim[0], 2), "{:?}", relatorio.finished);
    let tela = linhas_da_tela();
    exige(&tela, "ftpd: endereco invalido: abc");
    assert!(!tem(&tela, "ouvindo"), "{:?}", tela);
    conferir_frames(frames);
}

#[test_case]
fn login_e_comandos_simples() {
    let frames = preparar();
    empurrar("\n");
    let sessao = roteiro![
        login(),
        par("noop", "200"),
        par("SYST", "215"),
        par("TYPE A", "200"),
        par("TYPE I", "200"),
        par("TYPE E", "504"),
        par("TYPE", "501"),
        par("SITE", "501"),
        par("SITE FOO", "504"),
        par("PORT 1,2,3,4,5,6", "502"),
        par("EPRT |1|1.2.3.4|5|", "502"),
        par("REST 0", "502"),
        par("RNFR a", "502"),
        par("APPE a", "502"),
        par("FEAT", "502"),
        par("OPTS UTF8 ON", "502"),
        par("MLSD", "502"),
        par("XYZZY", "502"),
        par("QUIT", "221"),
        vec![Passo::EsperaFim, Passo::FechaControle],
    ];
    // Antes do login: so USER, PASS, QUIT e NOOP.
    let antes = roteiro![
        vec![Passo::Conecta, Passo::Espera("220")],
        par("PWD", "530"),
        par("CWD /ram", "530"),
        par("PASS x", "503"),
        par("USER", "501"),
        par("NOOP", "200"),
        par("USER aluno", "331"),
        par("PASS", "230"),
        par("QUIT", "221"),
        vec![Passo::EsperaFim, Passo::FechaControle],
    ];
    // O QUIT encerra a sessao, nao o servidor: a segunda sessao funciona.
    let (relatorio, cliente) = executar(roteiro![antes, sessao, login(), fim()]);
    conferir_ok(&relatorio, &cliente);
    for linha in [
        "530 faca login primeiro",
        "503 envie USER antes",
        "501 faltou o argumento",
        "215 UNIX Type: L8",
        "200 tipo A",
        "200 tipo I",
        "504 tipo nao suportado",
        "504 so SITE EXIT existe",
        "502 comando nao implementado",
        "221 ate logo",
        "221 encerrando o servidor",
    ] {
        ha(&cliente, linha);
    }
    let tela = linhas_da_tela();
    exige(&tela, "ftpd: encerrado por SITE EXIT");
    conferir_frames(frames);
}

// ---------------------------------------------------------------------------
// US1: ver e baixar
// ---------------------------------------------------------------------------

#[test_case]
fn pwd_cwd_e_cdup() {
    let frames = preparar();
    empurrar("\n");
    let (relatorio, cliente) = executar(roteiro![
        login(),
        par("PWD", "257"),
        par("CWD /ram/docs", "250"),
        par("PWD", "257"),
        par("CDUP", "250"),
        par("PWD", "257"),
        par("CWD /ram/../disco", "250"),
        par("PWD", "257"),
        par("CWD /DISCO/", "250"),
        par("CWD /ram", "250"),
        par("CWD ola.txt", "550"),
        par("CWD nada", "550"),
        par("CWD /xyz", "550"),
        par("CWD docs", "250"),
        par("PWD", "257"),
        par("CWD ../..", "250"),
        par("PWD", "257"),
        par("CDUP", "250"),
        par("PWD", "257"),
        fim(),
    ]);
    conferir_ok(&relatorio, &cliente);
    let pwd: Vec<&str> =
        cliente.linhas.iter().filter(|l| l.starts_with("257")).map(|l| l.as_str()).collect();
    assert_eq!(
        pwd,
        [
            "257 \"/\" e o diretorio atual",
            "257 \"/ram/docs\" e o diretorio atual",
            "257 \"/ram\" e o diretorio atual",
            "257 \"/disco\" e o diretorio atual",
            "257 \"/ram/docs\" e o diretorio atual",
            "257 \"/\" e o diretorio atual",
            "257 \"/\" e o diretorio atual",
        ]
    );
    ha(&cliente, "550 tipo errado (arquivo ou diretorio)");
    ha(&cliente, "550 nao encontrado");
    conferir_frames(frames);
}

#[test_case]
fn listagens_da_raiz_do_ram_e_do_disco() {
    let frames = preparar();
    empurrar("\n");
    let (relatorio, cliente) = executar(roteiro![
        login(),
        baixar("LIST /"),
        baixar("NLST /"),
        baixar("LIST /ram"),
        baixar("NLST /ram"),
        baixar("LIST -la /disco/docs"),
        baixar("NLST -a"),
        // Um arquivo no lugar de um diretorio: erro e nenhum 150.
        pasv(),
        par("LIST /ram/ola.txt", "550"),
        // O ouvinte de dados foi gasto pelo comando que falhou.
        par("NLST", "425"),
        fim(),
    ]);
    conferir_ok(&relatorio, &cliente);
    let t = &cliente.transferencias;
    assert_eq!(texto(&t[0]), format!("{}{}", LINHA_RAM, LINHA_DISCO));
    assert_eq!(texto(&t[1]), "ram\r\ndisco\r\n");
    let lista_ram = texto(&t[2]);
    assert!(lista_ram.contains("-rw-r--r-- 1 os-rust os-rust 72 Jan  1  1980 ola.txt\r\n"), "{}", lista_ram);
    assert!(lista_ram.contains("-rw-r--r-- 1 os-rust os-rust 0 Jan  1  1980 vazio.txt\r\n"), "{}", lista_ram);
    assert!(lista_ram.contains("drwxr-xr-x 1 os-rust os-rust 0 Jan  1  1980 bin\r\n"), "{}", lista_ram);
    assert!(lista_ram.contains("drwxr-xr-x 1 os-rust os-rust 0 Jan  1  1980 docs\r\n"), "{}", lista_ram);
    let nomes_ram = texto(&t[3]);
    for nome in ["bin\r\n", "docs\r\n", "ola.txt\r\n", "vazio.txt\r\n"] {
        assert!(nomes_ram.contains(nome), "{}", nomes_ram);
    }
    assert!(texto(&t[4]).contains("-rw-r--r-- 1 os-rust os-rust 65537 Jan  1  1980 grande.bin\r\n"));
    // `NLST -a` sem diretorio lista o diretorio atual (a raiz virtual).
    assert_eq!(texto(&t[5]), "ram\r\ndisco\r\n");
    ha(&cliente, "550 tipo errado (arquivo ou diretorio)");
    ha(&cliente, "425 use PASV ou EPSV antes");
    conferir_frames(frames);
}

#[test_case]
fn retr_baixa_arquivos_pequenos_vazios_e_grandes() {
    let frames = preparar();
    empurrar("\n");
    let (relatorio, cliente) = executar(roteiro![
        login(),
        baixar("RETR /ram/ola.txt"),
        baixar("RETR /ram/vazio.txt"),
        baixar("RETR /disco/docs/grande.bin"),
        pasv(),
        par("RETR /ram/nada.txt", "550"),
        pasv(),
        par("RETR /", "550"),
        pasv(),
        par("RETR /ram/docs", "550"),
        par("RETR /ram/ola.txt", "425"),
        par("RETR", "501"),
        par("SIZE /ram/ola.txt", "213"),
        par("SIZE /RAM/Ola.TXT", "213"),
        par("SIZE /ram", "550"),
        par("SIZE /disco", "550"),
        par("SIZE /ram/docs", "550"),
        par("SIZE /ram/nada.txt", "550"),
        par("SIZE /", "550"),
        par("SIZE", "501"),
        fim(),
    ]);
    conferir_ok(&relatorio, &cliente);
    let t = &cliente.transferencias;
    assert_eq!(t[0], OLA_TXT);
    assert_eq!(OLA_TXT.len(), 72);
    assert!(t[1].is_empty());
    assert_eq!(t[2].len(), 65537);
    assert!(t[2].iter().all(|&b| b == 0));
    ha(&cliente, "213 72");
    ha(&cliente, "550 nao encontrado");
    ha(&cliente, "550 a raiz so lista /ram e /disco");
    ha(&cliente, "550 tipo errado (arquivo ou diretorio)");
    ha(&cliente, "425 use PASV ou EPSV antes");
    ha(&cliente, "501 faltou o argumento");
    let tela = linhas_da_tela();
    exige(&tela, "ftpd: enviou /ram/ola.txt (72 bytes)");
    exige(&tela, "ftpd: enviou /ram/vazio.txt (0 bytes)");
    exige(&tela, "ftpd: enviou /disco/docs/grande.bin (65537 bytes)");
    conferir_frames(frames);
}

#[test_case]
fn pasv_duas_vezes_epsv_e_comando_de_dados_sem_pasv() {
    let frames = preparar();
    empurrar("\n");
    let (relatorio, cliente) = executar(roteiro![
        login(),
        par("LIST", "425"),
        par("NLST", "425"),
        par("STOR /ram/sem_pasv.txt", "425"),
        pasv(),
        pasv(),
        vec![cmd("NLST /"), Passo::AbreDados, Passo::Espera("150"), Passo::LeDados, Passo::Espera("226")],
        par("EPSV", "229"),
        par("EPSV", "229"),
        vec![cmd("NLST /"), Passo::AbreDados, Passo::Espera("150"), Passo::LeDados, Passo::Espera("226")],
        fim(),
    ]);
    conferir_ok(&relatorio, &cliente);
    assert_eq!(cliente.transferencias.len(), 2);
    assert_eq!(texto(&cliente.transferencias[0]), "ram\r\ndisco\r\n");
    assert_eq!(texto(&cliente.transferencias[1]), "ram\r\ndisco\r\n");
    conferir_frames(frames);
}

// ---------------------------------------------------------------------------
// US2: enviar
// ---------------------------------------------------------------------------

#[test_case]
fn stor_grava_arquivos_de_varios_tamanhos_e_a_volta_e_identica() {
    let frames = preparar();
    empurrar("\n");
    let tamanhos = [0usize, 1, 1024, 1025, 3000];
    let mut passos = login();
    for n in tamanhos {
        passos.extend(enviar(&format!("STOR /disco/t{}.bin", n), padrao(n)));
        if n == 0 || n >= 1025 {
            passos.extend(enviar(&format!("STOR /ram/u{}.bin", n), padrao(n)));
        }
    }
    // A ordem das transferencias lidas: cada tamanho no /disco e, para os
    // tamanhos 0, 1025 e 3000, tambem no /ram.
    let mut esperados: Vec<usize> = Vec::new();
    for n in tamanhos {
        passos.extend(baixar(&format!("RETR /disco/t{}.bin", n)));
        esperados.push(n);
        if n == 0 || n >= 1025 {
            passos.extend(baixar(&format!("RETR /ram/u{}.bin", n)));
            esperados.push(n);
        }
    }
    passos.extend(fim());
    let (relatorio, cliente) = executar(passos);
    conferir_ok(&relatorio, &cliente);
    assert_eq!(cliente.transferencias.len(), esperados.len());
    for (lido, n) in cliente.transferencias.iter().zip(&esperados) {
        assert_eq!(*lido, padrao(*n), "{} bytes", n);
    }
    let tela = linhas_da_tela();
    exige(&tela, "ftpd: recebeu /ram/u3000.bin (3000 bytes)");
    // O que o FTP gravou o prompt enxerga.
    exige(&rodar_comando("ls /disco"), "t3000.bin");
    exige(&rodar_comando("ls /ram"), "u1025.bin");
    conferir_frames(frames);
}

#[test_case]
fn stor_recusa_nome_repetido_nome_ruim_raiz_e_pasta_inexistente() {
    let frames = preparar();
    empurrar("\n");
    let (relatorio, cliente) = executar(roteiro![
        login(),
        pasv(),
        par("STOR /ram/ola.txt", "550"),
        pasv(),
        par("STOR /disco/nome_muito_longo.txt", "553"),
        pasv(),
        par("STOR /x.txt", "550"),
        pasv(),
        par("STOR /ram/naoexist/a.txt", "550"),
        par("STOR /ram/sem_pasv.txt", "425"),
        pasv(),
        par("STOR", "501"),
        baixar("RETR /ram/ola.txt"),
        baixar("NLST /ram"),
        baixar("NLST /disco"),
        fim(),
    ]);
    conferir_ok(&relatorio, &cliente);
    ha(&cliente, "550 ja existe");
    ha(&cliente, "553 caminho invalido");
    ha(&cliente, "550 a raiz so lista /ram e /disco");
    ha(&cliente, "550 nao encontrado");
    // O arquivo antigo ficou intacto e nada novo apareceu.
    assert_eq!(cliente.transferencias[0], OLA_TXT);
    assert!(!texto(&cliente.transferencias[1]).contains("sem_pasv"));
    assert!(!texto(&cliente.transferencias[2]).contains("nome_muito"));
    nao_ha(&cliente, "150 abrindo a conexao de dados\r\n550");
    conferir_frames(frames);
}

#[test_case]
fn stor_em_pasta_cheia_da_552_sem_deixar_arquivo_parcial() {
    let frames = preparar();
    empurrar("\n");
    let mut passos = roteiro![login(), par("MKD /ram/cheia", "257")];
    for i in 0..14 {
        passos.extend(enviar(&format!("STOR /ram/cheia/f{}.txt", i), padrao(10 + i)));
    }
    passos.extend(pasv());
    passos.extend(par("STOR /ram/cheia/extra.txt", "552"));
    passos.extend(baixar("NLST /ram/cheia"));
    passos.extend(fim());
    let (relatorio, cliente) = executar(passos);
    conferir_ok(&relatorio, &cliente);
    ha(&cliente, "552 sem espaco");
    let nomes: Vec<&str> = texto(&cliente.transferencias[0]).split("\r\n").filter(|n| !n.is_empty()).collect();
    assert_eq!(nomes.len(), 14, "{:?}", nomes);
    assert!(!nomes.contains(&"extra.txt"), "{:?}", nomes);
    conferir_frames(frames);
}

#[test_case]
fn um_executavel_enviado_pelo_ftp_roda_com_run() {
    let frames = preparar();
    empurrar("\n");
    empurrar("\n");
    let (relatorio, cliente) = executar(roteiro![
        login(),
        baixar("RETR /ram/bin/hello"),
        pasv(),
        vec![cmd("STOR /ram/bin/oi"), Passo::AbreDados, Passo::Espera("150"), Passo::EnviaDadosDe(0), Passo::Espera("226")],
        baixar("RETR /ram/bin/oi"),
        fim(),
    ]);
    conferir_ok(&relatorio, &cliente);
    assert!(!cliente.transferencias[0].is_empty());
    assert_eq!(cliente.transferencias[0], cliente.transferencias[1], "o ELF voltou diferente");
    // Depois que o ftpd termina, o programa enviado roda.
    exige(&rodar_comando("run /ram/bin/oi"), "Ola do ring 3!");
    conferir_fim(frames);
}

// ---------------------------------------------------------------------------
// US3: organizar
// ---------------------------------------------------------------------------

#[test_case]
fn mkd_rmd_e_dele_com_cada_erro() {
    let frames = preparar();
    empurrar("\n");
    let longo = format!("MKD /ram/{}", ["abcdefgh"; 8].join("/"));
    let (relatorio, cliente) = executar(roteiro![
        login(),
        par("MKD /ram/novo", "257"),
        par("MKD /ram/novo", "550"),
        par("MKD /RAM/Novo2", "257"),
        par("RMD /ram/novo", "250"),
        par("RMD /ram/docs", "550"),
        par("DELE /ram/docs", "550"),
        enviar("STOR /ram/novo2/a.txt", padrao(5)),
        par("RMD /ram/novo2", "550"),
        par("DELE /ram/novo2/a.txt", "250"),
        pasv(),
        par("RETR /ram/novo2/a.txt", "550"),
        par("DELE /ram/nada.txt", "550"),
        par("RMD /ram/nada", "550"),
        par("RMD /ram/ola.txt", "550"),
        par("MKD", "501"),
        par("DELE", "501"),
        par("MKD /disco/docs", "550"),
        par("RMD /disco", "550"),
        par("DELE /", "550"),
        par("MKD /", "550"),
        par("MKD /ram/nome_muito_longo", "553"),
        par(&longo, "553"),
        par("RMD /ram/novo2", "250"),
        fim(),
    ].into_iter().collect::<Vec<_>>());
    conferir_ok(&relatorio, &cliente);
    for linha in [
        "257 \"/ram/novo\" criado",
        "257 \"/ram/novo2\" criado",
        "550 ja existe",
        "250 removido",
        "550 diretorio nao vazio",
        "550 tipo errado (arquivo ou diretorio)",
        "250 apagado",
        "550 nao encontrado",
        "550 em uso",
        "550 a raiz so lista /ram e /disco",
        "553 caminho invalido",
        "553 caminho longo demais",
    ] {
        ha(&cliente, linha);
    }
    conferir_frames(frames);
}

// ---------------------------------------------------------------------------
// US4: falhas e abusos
// ---------------------------------------------------------------------------

#[test_case]
fn comandos_juntos_partidos_e_so_com_lf_recebem_uma_resposta_cada() {
    let frames = preparar();
    empurrar("\n");
    let (relatorio, cliente) = executar(roteiro![
        login(),
        vec![
            Passo::Envia(b"NOOP\r\nNOOP\r\n".to_vec()),
            Passo::Espera("200"),
            Passo::Espera("200"),
            Passo::Envia(b"NO".to_vec()),
            Passo::Dorme(5),
            Passo::Envia(b"OP\r\n".to_vec()),
            Passo::Espera("200"),
            Passo::Envia(b"NOOP\n".to_vec()),
            Passo::Espera("200"),
            Passo::Envia(b"NOOP\nPWD\r\nNOOP\n".to_vec()),
            Passo::Espera("200"),
            Passo::Espera("257"),
            Passo::Espera("200"),
        ],
        fim(),
    ]);
    conferir_ok(&relatorio, &cliente);
    // 220, 331, 230, seis 200, um 257 e o 221: nenhuma resposta a mais.
    assert_eq!(cliente.linhas.len(), 3 + 6 + 1 + 1, "{:?}", cliente.linhas);
    conferir_frames(frames);
}

#[test_case]
fn linha_de_512_bytes_sem_fim_da_421_e_a_de_512_com_fim_passa() {
    let frames = preparar();
    empurrar("\n");
    let mut sem_fim = vec![b'A'; 512];
    let mut com_fim = vec![b'B'; 510];
    com_fim.extend_from_slice(b"\r\n");
    let (relatorio, cliente) = executar(roteiro![
        login(),
        vec![Passo::Envia(com_fim), Passo::Espera("502")],
        vec![Passo::Envia(core::mem::take(&mut sem_fim)), Passo::Espera("421 linha longa demais"), Passo::EsperaFim, Passo::FechaControle],
        login(),
        fim(),
    ]);
    conferir_ok(&relatorio, &cliente);
    conferir_frames(frames);
}

#[test_case]
fn cliente_que_fecha_o_controle_sem_quit_nao_prende_nada() {
    let frames = preparar();
    empurrar("\n");
    let (relatorio, cliente) = executar(roteiro![login(), vec![Passo::FechaControle], login(), fim()]);
    conferir_ok(&relatorio, &cliente);
    exige(&linhas_da_tela(), "ftpd: sessao encerrada");
    conferir_fim(frames);
}

#[test_case]
fn cliente_que_fecha_tudo_no_meio_de_um_retr_nao_prende_nada() {
    let frames = preparar();
    empurrar("\n");
    let (relatorio, cliente) = executar(roteiro![
        login(),
        pasv(),
        vec![
            cmd("RETR /ram/ola.txt"),
            Passo::AbreDados,
            Passo::Espera("150"),
            // O cliente nao le nada. Com um arquivo maior que os buffers do
            // TCP o par ficaria preso para sempre: o kernel nao manda RST ao
            // fechar um socket com dados por ler, o que o computador de
            // verdade faz.
            Passo::FechaDados,
            Passo::FechaControle,
        ],
        login(),
        fim(),
    ]);
    conferir_ok(&relatorio, &cliente);
    conferir_fim(frames);
}

#[test_case]
fn cliente_que_fecha_tudo_no_meio_de_um_stor_nao_prende_nada() {
    let frames = preparar();
    empurrar("\n");
    let (relatorio, cliente) = executar(roteiro![
        login(),
        pasv(),
        // O ftpd entende o fim da conexao de dados como o fim do arquivo: a
        // rede nao distingue uma queda de um fim.
        vec![
            cmd("STOR /ram/parcial.bin"),
            Passo::AbreDados,
            Passo::Espera("150"),
            Passo::EnviaSemFechar(padrao(2000)),
            Passo::Dorme(3),
            Passo::FechaDados,
            Passo::FechaControle,
        ],
        login(),
        par("DELE /ram/parcial.bin", "250"),
        fim(),
    ]);
    conferir_ok(&relatorio, &cliente);
    conferir_fim(frames);
}

#[test_case]
fn cinquenta_sessoes_seguidas_nao_vazam_nada() {
    let frames = preparar();
    empurrar("\n");
    let mut passos = Vec::new();
    for _ in 0..50 {
        passos.extend(login());
        passos.extend(baixar("NLST /"));
        passos.extend(par("QUIT", "221"));
        passos.extend([Passo::EsperaFim, Passo::FechaControle]);
    }
    passos.extend(login());
    passos.extend(fim());
    let (relatorio, cliente) = executar_com(&["ftpd"], passos, 6000);
    conferir_ok(&relatorio, &cliente);
    assert_eq!(cliente.transferencias.len(), 50);
    conferir_fim(frames);
    // Nenhum arquivo ficou aberto: o prompt apaga e lista sem erro.
    exige(&rodar_comando("ls /ram"), "ola.txt");
}

#[test_case]
fn dois_ftpd_o_segundo_recebe_porta_em_uso() {
    let frames = preparar();
    empurrar("\n\n");
    let (relatorio, cliente) =
        executar_com(&["ftpd", "ftpd"], roteiro![login(), fim()], 2000);
    assert!(cliente.erro.is_none(), "{:?}", cliente.erro);
    let mut codigos: Vec<u64> = relatorio
        .finished
        .iter()
        .map(|f| match f.termination {
            Termination::Exit { code } => code,
            outro => panic!("termino inesperado: {:?}", outro),
        })
        .collect();
    codigos.sort();
    assert_eq!(codigos, [0, 2], "{:?}", relatorio.finished);
    exige(&linhas_da_tela(), "ftpd: erro de rede: porta em uso");
    conferir_fim(frames);
}

// ---------------------------------------------------------------------------
// US5: parar o servidor
// ---------------------------------------------------------------------------

#[test_case]
fn site_exit_devolve_o_prompt() {
    let frames = preparar();
    empurrar("\n");
    let (relatorio, cliente) = executar(roteiro![login(), par("SITE EXIT", "221")]);
    conferir_ok(&relatorio, &cliente);
    exige(&linhas_da_tela(), "ftpd: encerrado por SITE EXIT");
    exige(&rodar_comando("echo vivo"), "vivo");
    conferir_frames(frames);
}

// ---------------------------------------------------------------------------
// Versão e documentos
// ---------------------------------------------------------------------------

#[test_case]
fn a_versao_0_12_1_o_guia_os_readme_e_os_changelog() {
    assert_eq!(os_rust::VERSION, "os-rust v0.12.1");
    exige(&rodar_comando("sobre"), "os-rust v0.12.1");

    // O guia em portugues carrega o programa literalmente; o ingles, o titulo.
    let guia = include_str!("../GUIA_DO_PROGRAMADOR.md");
    let fonte = include_str!("../programs/src/bin/ftpd.rs");
    assert!(guia.contains(fonte.trim_end()), "o guia nao tem o codigo do ftpd literalmente");
    assert!(guia.contains("Programa de exemplo 15: ftpd"));
    assert!(include_str!("../PROGRAMMERS_GUIDE.md").contains("Example program 15: ftpd"));

    // `ftpd` aparece uma vez e em ordem na lista de `run`.
    let nomes: Vec<&str> = user::program_names().collect();
    assert_eq!(nomes.iter().filter(|n| **n == "ftpd").count(), 1);
    let mut ordenados = nomes.clone();
    ordenados.sort();
    assert_eq!(nomes, ordenados);

    for (nome, readme) in [
        ("README.md", include_str!("../README.md")),
        ("README.en.md", include_str!("../README.en.md")),
    ] {
        let qemu = readme
            .lines()
            .find(|l| l.contains("hostfwd=tcp:127.0.0.1:2121-:21"))
            .unwrap_or_else(|| panic!("{}: falta a linha do QEMU do FTP", nome));
        assert!(qemu.starts_with("qemu-system-x86_64 "), "{}: {}", nome, qemu);
        assert!(qemu.contains("hostfwd=tcp:127.0.0.1:2100-:2100"), "{}: {}", nome, qemu);
        assert!(qemu.contains("index=0") && qemu.contains("index=1"), "{}: {}", nome, qemu);
        assert!(!qemu.contains("snapshot=on"), "{}: {}", nome, qemu);
        for curl in [
            "curl ftp://127.0.0.1:2121/",
            "curl ftp://127.0.0.1:2121/disco/",
            "curl -T nota.txt ftp://127.0.0.1:2121/disco/nota.txt",
            "curl -o copia.txt ftp://127.0.0.1:2121/disco/nota.txt",
            "curl -Q \"DELE /disco/nota.txt\" ftp://127.0.0.1:2121/",
        ] {
            assert!(readme.lines().any(|l| l.trim() == curl), "{}: falta {:?}", nome, curl);
        }
        for ftplib in ["connect(\"127.0.0.1\", 2121)", "login()", "retrlines(\"LIST /disco\")", "storbinary", "retrbinary", "delete", "sendcmd(\"SITE EXIT\")"] {
            assert!(readme.contains(ftplib), "{}: falta {:?}", nome, ftplib);
        }
    }
    assert!(include_str!("../README.md").contains("### Servidor FTP"));
    assert!(include_str!("../README.en.md").contains("### FTP server"));
    assert!(include_str!("../CHANGELOG.md").contains("[0.12.1]"));
    assert!(include_str!("../CHANGELOG.en.md").contains("[0.12.1]"));
    assert!(include_str!("../WALKTHROUGH.md").contains("O FTP por cima dos sockets"));
    assert!(include_str!("../WALKTHROUGH.en.md").contains("FTP on top of the sockets"));
}
