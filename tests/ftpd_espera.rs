//! Os testes do `ftpd` que gastam tempo de relógio de verdade esperando um
//! limite do servidor: 30 s sem nenhum cliente e 10 s de uma conexão de dados
//! que ninguém abre. Ficam num binário à parte para caber folgados nos 60 s do
//! `test-timeout`. O cliente é o mesmo roteiro de `tests/ftpd.rs`, copiado
//! aqui porque cada arquivo de `tests/` é um kernel próprio.

#![allow(dead_code)]
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
// Tempos do servidor
// ---------------------------------------------------------------------------

#[test_case]
fn ftpd_sozinho_desiste_depois_de_30_segundos() {
    let frames = preparar();
    empurrar("\n");
    let antes = timer::ticks();
    let relatorio = user::run_all(&["ftpd"], None).expect("run ftpd");
    let ticks = timer::ticks() - antes;
    let fim = termino_de(&relatorio, "ftpd");
    assert!(fim.len() == 1 && saiu_com(fim[0], 0), "{:?}", relatorio.finished);
    let tela = linhas_da_tela();
    exige(&tela, "ftpd: ouvindo na porta 21 (dados na porta 2100), anunciando 127.0.0.1");
    exige(&tela, "ftpd: ninguem se conectou em 30 s, encerrando");
    // 30 s = 3000 ticks, com uma pequena folga para cima.
    assert!((3000..3100).contains(&ticks), "demorou {} ticks", ticks);
    // O kernel dormiu, em vez de girar: poucas voltas ociosas por tick.
    assert!(relatorio.idle_loops as u64 <= ticks + 5, "{} voltas", relatorio.idle_loops);
    conferir_fim(frames);
}

#[test_case]
fn conexao_de_dados_que_ninguem_abre_da_425_em_10_segundos_e_apaga_o_stor() {
    let frames = preparar();
    empurrar("\n");
    let (relatorio, cliente) = executar(roteiro![
        login(),
        pasv(),
        // Nao ha AbreDados: o cliente nunca conecta nos dados.
        vec![
            cmd("STOR /ram/ftpd_t.txt"),
            Passo::Espera("150"),
            Passo::Espera("425 o cliente nao abriu a conexao de dados"),
        ],
        // O arquivo criado antes do 150 foi apagado.
        baixar("NLST /ram"),
        // O ouvinte de dados tambem foi fechado.
        par("NLST", "425"),
        fim(),
    ]);
    conferir_ok(&relatorio, &cliente);
    // login() tem 6 passos e pasv() 2: o STOR e o passo 8, o 150 o 9 e o 425 o 10.
    let espera = cliente.ticks[10] - cliente.ticks[9];
    assert!((1000..1100).contains(&espera), "esperou {} ticks", espera);
    assert!(!texto(&cliente.transferencias[0]).contains("ftpd_t"), "o arquivo ficou");
    conferir_fim(frames);
}

fn texto(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).expect("a transferencia nao e texto")
}
