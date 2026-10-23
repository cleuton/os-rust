//! Testes do Marco 12 com os programas `servidor` e `cliente` ponta a ponta
//! (loopback, sem rede), o vazamento de sockets em 50 ciclos, a identificação
//! da versão e os documentos (contrato, guia, README, argumentos do QEMU). Os
//! programas leem o teclado pelo caminho real (`SYS_READ_LINE`): os testes
//! "digitam" empurrando scancodes na fila do kernel.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os_rust::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use os_rust::user::{self, Finished, RunReport, Termination};
use os_rust::{interrupts, keyboard, memory, net, shell, vga_buffer};

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
// Ajudantes
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

fn termino_de<'a>(relatorio: &'a RunReport, nome: &str) -> &'a Finished {
    relatorio
        .finished
        .iter()
        .find(|f| f.name == nome)
        .unwrap_or_else(|| panic!("{} nao aparece nos terminos: {:?}", nome, relatorio.finished))
}

fn saiu_com(f: &Finished, codigo: u64) -> bool {
    f.termination == Termination::Exit { code: codigo }
}

/// Espera (com a pilha andando) até a tabela de conexões esvaziar, no máximo
/// `limite_ticks`. Devolve quantos blocos restam.
fn esperar_pilha_vazia(limite_ticks: u64) -> usize {
    let fim = os_rust::timer::ticks() + limite_ticks;
    loop {
        x86_64::instructions::interrupts::disable();
        net::poll();
        let em_uso = net::tcp_in_use();
        if em_uso == 0 || os_rust::timer::ticks() >= fim {
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

// ---------------------------------------------------------------------------
// servidor e cliente
// ---------------------------------------------------------------------------

#[test_case]
fn servidor_e_cliente_conversam_pelo_loopback() {
    let frames = preparar();
    empurrar("\n");
    let relatorio = user::run_all(&["servidor", "cliente"], None).expect("run servidor cliente");
    assert!(saiu_com(termino_de(&relatorio, "servidor"), 0), "{:?}", relatorio.finished);
    assert!(saiu_com(termino_de(&relatorio, "cliente"), 0), "{:?}", relatorio.finished);
    let tela = linhas_da_tela();
    exige(&tela, "servidor: ouvindo na porta 8000");
    exige(&tela, "cliente: endereco do servidor (vazio = 127.0.0.1):");
    exige(&tela, "servidor: recebi 33 bytes");
    exige(&tela, "HTTP/1.0 200 OK");
    exige(&tela, "Content-Type: text/plain");
    exige(&tela, "Content-Length: 23");
    exige(&tela, "ola da rede do os-rust");
    exige(&tela, "cliente: 88 bytes recebidos");
    exige(&tela, "servidor: respondi 23 bytes de corpo");
    // Nenhum `\r` chega à tela (a tela troca bytes fora de ASCII por '?').
    assert!(!tela.iter().any(|l| l.contains('?')), "{:?}", tela);
    // O fim da conexão termina em segundo plano, em até 3 s.
    assert_eq!(esperar_pilha_vazia(300), 0);
    assert_eq!(memory::frames_outstanding(), frames);
}

#[test_case]
fn cliente_sem_servidor_e_recusado_em_menos_de_um_segundo() {
    let frames = preparar();
    empurrar("\n");
    let antes = os_rust::timer::ticks();
    let relatorio = user::run_all(&["cliente"], None).expect("run cliente");
    let ticks = os_rust::timer::ticks() - antes;
    assert!(saiu_com(termino_de(&relatorio, "cliente"), 1), "{:?}", relatorio.finished);
    exige(&linhas_da_tela(), "cliente: nao conectou em 127.0.0.1:8000: recusado");
    assert!(ticks < 100, "o loopback responde RST na hora, mas levou {} ticks", ticks);
    assert_eq!(esperar_pilha_vazia(300), 0);
    assert_eq!(memory::frames_outstanding(), frames);
}

#[test_case]
fn cliente_com_endereco_invalido_sai_com_codigo_2() {
    let frames = preparar();
    empurrar("abc\n");
    let relatorio = user::run_all(&["cliente"], None).expect("run cliente");
    assert!(saiu_com(termino_de(&relatorio, "cliente"), 2), "{:?}", relatorio.finished);
    exige(&linhas_da_tela(), "cliente: endereco invalido: abc");
    assert_eq!(esperar_pilha_vazia(300), 0);
    assert_eq!(memory::frames_outstanding(), frames);
}

#[test_case]
fn cliente_no_gateway_sem_servidor_no_host_recusa_ou_esgota_o_tempo() {
    let frames = preparar();
    empurrar("10.0.2.2\n");
    let relatorio = user::run_all(&["cliente"], None).expect("run cliente");
    let fim = termino_de(&relatorio, "cliente");
    // O teste nao pode depender do que o computador de verdade tem na porta
    // 8000: se houver um servidor la, o cliente conecta e sai com 0.
    let tela = linhas_da_tela();
    if saiu_com(fim, 1) {
        assert!(
            tem(&tela, "cliente: nao conectou em 10.0.2.2:8000: recusado")
                || tem(&tela, "cliente: nao conectou em 10.0.2.2:8000: tempo esgotado"),
            "{:?}",
            tela
        );
    } else {
        assert!(saiu_com(fim, 0), "{:?}", relatorio.finished);
    }
    assert_eq!(esperar_pilha_vazia(600), 0);
    assert_eq!(memory::frames_outstanding(), frames);
}

#[test_case]
fn cinquenta_execucoes_seguidas_nao_vazam_nada() {
    let frames = preparar();
    for ciclo in 0..50 {
        reset_teclado();
        empurrar("\n");
        let relatorio = user::run_all(&["servidor", "cliente"], None).expect("run servidor cliente");
        assert!(saiu_com(termino_de(&relatorio, "servidor"), 0), "ciclo {}: {:?}", ciclo, relatorio.finished);
        assert!(saiu_com(termino_de(&relatorio, "cliente"), 0), "ciclo {}: {:?}", ciclo, relatorio.finished);
    }
    assert_eq!(esperar_pilha_vazia(300), 0, "conexoes presas");
    assert_eq!(memory::frames_outstanding(), frames, "frames presos");
}

// ---------------------------------------------------------------------------
// Versão, contrato, guia, README e argumentos do QEMU
// ---------------------------------------------------------------------------

fn rodar_comando(comando: &str) -> Vec<String> {
    vga_buffer::clear_screen();
    digitar_no_prompt(comando);
    linhas_da_tela()
}

#[test_case]
fn a_versao_0_12_0_aparece_e_o_que_existia_continua_igual() {
    assert_eq!(os_rust::VERSION, "os-rust v0.12.0");
    exige(&rodar_comando("sobre"), "os-rust v0.12.0");
    // Um comando de cada marco anterior segue funcionando.
    exige(&rodar_comando("ls /ram"), "ola.txt");
    exige(&rodar_comando("cat /ram/ola.txt"), "ola");
    exige(&rodar_comando("data"), "UTC");
    exige(&rodar_comando("mkdir /ram/rede_tmp"), "mkdir: criado /ram/rede_tmp");
    exige(&rodar_comando("rmdir /ram/rede_tmp"), "rmdir: removido /ram/rede_tmp");
    exige(&rodar_comando("run hello"), "Ola do ring 3!");
    exige(&rodar_comando("echo vivo"), "vivo");
}

const CONTRATO: &str = include_str!("../SYSCALLS.md");
const CONTRATO_EN: &str = include_str!("../SYSCALLS.en.md");

#[test_case]
fn o_contrato_concorda_com_as_constantes_da_rede() {
    use abi::*;
    assert!(CONTRATO.contains("**Versão do contrato**: 8"));
    assert!(CONTRATO.contains("os-rust 0.12.0 (Marco 12)"));
    assert!(CONTRATO.contains("fora de 1 a 23"));
    assert!(CONTRATO_EN.contains("**Contract version**: 8"));
    assert!(CONTRATO_EN.contains("os-rust 0.12.0"));
    assert!(CONTRATO_EN.contains("outside 1 to 23"));
    let syscalls = [
        (SYS_TCP_LISTEN, "SYS_TCP_LISTEN", 17),
        (SYS_TCP_ACCEPT, "SYS_TCP_ACCEPT", 18),
        (SYS_TCP_CONNECT, "SYS_TCP_CONNECT", 19),
        (SYS_TCP_SEND, "SYS_TCP_SEND", 20),
        (SYS_TCP_RECV, "SYS_TCP_RECV", 21),
        (SYS_TCP_CLOSE, "SYS_TCP_CLOSE", 22),
        (SYS_TCP_TIMEOUT, "SYS_TCP_TIMEOUT", 23),
    ];
    for (valor, nome, esperado) in syscalls {
        assert_eq!(valor, esperado, "{}", nome);
        let linha = alloc::format!("| {} | `{}` |", valor, nome);
        assert!(CONTRATO.contains(&linha) && CONTRATO_EN.contains(&linha), "{}", nome);
    }
    let erros = [
        (ERR_TIMEDOUT, "ERR_TIMEDOUT", -17),
        (ERR_CONNREFUSED, "ERR_CONNREFUSED", -18),
        (ERR_CONNRESET, "ERR_CONNRESET", -19),
        (ERR_ADDRINUSE, "ERR_ADDRINUSE", -20),
        (ERR_NOBUFS, "ERR_NOBUFS", -21),
    ];
    for (valor, nome, esperado) in erros {
        assert_eq!(valor, esperado, "{}", nome);
        let linha = alloc::format!("| `{}` | `{}` |", nome, valor);
        assert!(CONTRATO.contains(&linha) && CONTRATO_EN.contains(&linha), "{}", nome);
    }
    assert_eq!((MAX_SOCKETS, MAX_TCP_CONNECTIONS, TCP_MAX_TIMEOUT_MS), (4, 8, 60_000));
}

#[test_case]
fn o_guia_do_programador_contem_literalmente_o_servidor_e_o_cliente() {
    let guia = include_str!("../GUIA_DO_PROGRAMADOR.md");
    for fonte in [
        include_str!("../programs/src/bin/servidor.rs"),
        include_str!("../programs/src/bin/cliente.rs"),
    ] {
        assert!(guia.contains(fonte.trim_end()), "o guia nao tem o codigo do programa literalmente");
    }
    assert!(guia.contains("Rede (TCP)"));
    let en = include_str!("../PROGRAMMERS_GUIDE.md");
    assert!(en.contains("Network (TCP)"));
}

#[test_case]
fn servidor_e_cliente_aparecem_unicos_e_em_ordem_na_lista_de_run() {
    let nomes: Vec<&str> = user::program_names().collect();
    for nome in ["servidor", "cliente"] {
        assert_eq!(nomes.iter().filter(|n| **n == nome).count(), 1, "{}", nome);
    }
    let mut ordenados = nomes.clone();
    ordenados.sort();
    assert_eq!(nomes, ordenados);
}

#[test_case]
fn o_readme_ensina_a_rede_no_qemu_nos_dois_idiomas() {
    for (nome, readme) in [
        ("README.md", include_str!("../README.md")),
        ("README.en.md", include_str!("../README.en.md")),
    ] {
        let qemu = readme
            .lines()
            .find(|l| l.starts_with("qemu-system-x86_64 ") && l.contains("-nic user,model=rtl8139,hostfwd="))
            .unwrap_or_else(|| panic!("{}: falta a linha do QEMU com hostfwd", nome));
        assert!(qemu.contains("index=0"), "{}: {}", nome, qemu);
        assert!(readme.lines().any(|l| l.trim() == "python3 -m http.server 8000"), "{}", nome);
        assert!(readme.lines().any(|l| l.trim() == "curl http://127.0.0.1:8080/"), "{}", nome);
        let linha_12 = readme.lines().find(|l| l.starts_with("| 12.")).expect("linha do Marco 12");
        assert!(linha_12.ends_with("Concluído |") || linha_12.ends_with("Completed |"), "{}: {}", nome, linha_12);
        assert!(!linha_12.contains("UDP"), "{}: {}", nome, linha_12);
        assert!(readme.contains("0.12.0"), "{}", nome);
    }
}

#[test_case]
fn o_cargo_toml_traz_a_placa_e_mantem_o_snapshot() {
    let toml = include_str!("../Cargo.toml");
    let run = toml.lines().find(|l| l.starts_with("run-args")).expect("run-args");
    assert!(run.contains("\"-nic\", \"user,model=rtl8139\""), "{}", run);
    assert!(run.contains("snapshot=on"), "{}", run);
    assert!(!run.contains("hostfwd"), "o cargo run nao abre porta no computador: {}", run);
    // test-args: da linha `test-args` ate o `]` que fecha.
    let inicio = toml.find("test-args").expect("test-args");
    let fim = toml[inicio..].find(']').expect("fim de test-args") + inicio;
    let teste = &toml[inicio..fim];
    assert!(teste.contains("user,model=rtl8139,guestfwd=tcp:10.0.2.100:7-cmd:cat"), "{}", teste);
    assert!(!teste.contains("hostfwd"), "{}", teste);
    assert_eq!(teste.matches("snapshot=on").count(), 2, "os dois discos continuam com snapshot=on");
    assert!(toml.contains("version = \"0.12.0\""));
}
