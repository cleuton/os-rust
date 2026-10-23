//! Testes do Marco 12 com a **placa de verdade** do QEMU (RTL8139 com a rede
//! de usuário): o PCI acha a placa, `rede` e `ping` pelo prompt, o gateway
//! `10.0.2.2` do QEMU respondendo ARP e eco, o eco de TCP pelo `guestfwd`, e o
//! comportamento sem placa (desligada na pilha por um ajudante de teste).
//! Cada `ping` dura até 4 s de relógio real, por isso os testes de rede ficam
//! distribuídos em vários binários (cada um cabe nos 60 s do `test-timeout`).

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
use os_rust::net::{self, CardStatus};
use os_rust::{pci, shell, vga_buffer};

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

fn rodar_comando(comando: &str) -> Vec<String> {
    vga_buffer::clear_screen();
    digitar_no_prompt(comando);
    linhas_da_tela()
}

fn tem(linhas: &[String], texto: &str) -> bool {
    linhas.iter().any(|linha| linha.contains(texto))
}

fn exige(linhas: &[String], texto: &str) {
    assert!(tem(linhas, texto), "a tela nao tem {:?}: {:?}", texto, linhas);
}

/// A linha (sem o eco do comando) que começa por `prefixo`.
fn linha_com<'a>(linhas: &'a [String], prefixo: &str) -> &'a str {
    linhas
        .iter()
        .find(|l| l.starts_with(prefixo))
        .unwrap_or_else(|| panic!("nenhuma linha comeca por {:?}: {:?}", prefixo, linhas))
}

// ---------------------------------------------------------------------------
// A placa e o comando `rede`
// ---------------------------------------------------------------------------

#[test_case]
fn o_pci_acha_a_rtl8139_e_o_mac_e_o_do_qemu() {
    assert!(pci::find(0x10EC, 0x8139).is_some());
    assert!(pci::find(0x1234, 0x5678).is_none());
    assert_eq!(net::card_status(), CardStatus::Ready([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]));
}

#[test_case]
fn rede_escreve_as_quatro_linhas() {
    let tela = rodar_comando("rede");
    exige(&tela, "placa: RTL8139 mac 52:54:00:12:34:56");
    exige(&tela, "ip: 10.0.2.15 mascara: 255.255.255.0 gateway: 10.0.2.2");
    let quadros = linha_com(&tela, "quadros: recebidos=");
    assert!(quadros.contains(" enviados=") && quadros.contains(" descartados="), "{}", quadros);
    exige(&tela, "tcp: 0 de 8 conexoes em uso");
    // Exatamente quatro linhas de resultado depois do eco do comando.
    let primeira = tela.iter().position(|l| l.starts_with("placa:")).unwrap();
    assert_eq!(tela[primeira..].len(), 5, "as 4 linhas e o prompt: {:?}", tela);
}

#[test_case]
fn ping_no_gateway_recebe_quatro_de_quatro() {
    let tela = rodar_comando("ping 10.0.2.2");
    exige(&tela, "ping 10.0.2.2: 4 pedidos");
    for seq in 1..=4 {
        let prefixo = alloc::format!("resposta de 10.0.2.2: seq={} tempo", seq);
        linha_com(&tela, &prefixo);
    }
    exige(&tela, "ping: 4 enviados, 4 recebidos");
    // A pilha aprendeu o MAC do gateway pelo ARP.
    let tela = rodar_comando("rede");
    let quadros = linha_com(&tela, "quadros: recebidos=");
    assert!(!quadros.contains("recebidos=0 "), "{}", quadros);
}

#[test_case]
fn ping_no_loopback_responde_na_hora_com_a_placa() {
    let tela = rodar_comando("ping 127.0.0.1");
    for seq in 1..=4 {
        let linha = alloc::format!("resposta de 127.0.0.1: seq={} tempo<10 ms", seq);
        exige(&tela, &linha);
    }
    exige(&tela, "ping: 4 enviados, 4 recebidos");
}

#[test_case]
fn ping_num_endereco_sem_dono_nao_tem_resposta() {
    let tela = rodar_comando("ping 10.0.2.77");
    for seq in 1..=4 {
        let linha = alloc::format!("sem resposta de 10.0.2.77: seq={}", seq);
        exige(&tela, &linha);
    }
    exige(&tela, "ping: 4 enviados, 0 recebidos");
}

#[test_case]
fn ping_com_argumentos_ruins_mostra_o_uso_e_o_prompt_responde() {
    exige(&rodar_comando("ping"), "uso: ping <ip>");
    exige(&rodar_comando("ping abc"), "ping: endereco invalido: abc");
    exige(&rodar_comando("ping 300.1.1.1"), "ping: endereco invalido: 300.1.1.1");
    exige(&rodar_comando("ping 1.2.3.4 5.6.7.8"), "uso: ping <ip>");
    exige(&rodar_comando("echo vivo"), "vivo");
}

#[test_case]
fn help_lista_rede_e_ping() {
    let tela = rodar_comando("help");
    exige(&tela, "rede - ");
    exige(&tela, "ping - ");
}

#[test_case]
fn sem_placa_a_rede_some_mas_o_loopback_continua() {
    net::set_card_enabled(false);
    let tela = rodar_comando("rede");
    exige(&tela, "placa: nenhuma (RTL8139 nao encontrada)");
    exige(&tela, "ip: 10.0.2.15 mascara: 255.255.255.0 gateway: 10.0.2.2");
    exige(&tela, "tcp: 0 de 8 conexoes em uso");
    exige(&rodar_comando("ping 10.0.2.2"), "ping: sem placa de rede");
    let tela = rodar_comando("ping 127.0.0.1");
    exige(&tela, "ping: 4 enviados, 4 recebidos");
    net::set_card_enabled(true);
    exige(&rodar_comando("rede"), "placa: RTL8139 mac 52:54:00:12:34:56");
}

// ---------------------------------------------------------------------------
// Configuração
// ---------------------------------------------------------------------------

#[test_case]
fn rede_com_tres_argumentos_troca_a_configuracao() {
    let tela = rodar_comando("rede 192.168.0.50 255.255.255.0 192.168.0.1");
    exige(&tela, "rede: configurado ip 192.168.0.50 mascara 255.255.255.0 gateway 192.168.0.1");
    exige(&rodar_comando("rede"), "ip: 192.168.0.50 mascara: 255.255.255.0 gateway: 192.168.0.1");
    net::reset_config();
    exige(&rodar_comando("rede"), "ip: 10.0.2.15 mascara: 255.255.255.0 gateway: 10.0.2.2");
}

#[test_case]
fn rede_recusa_argumentos_invalidos_sem_mudar_nada() {
    let casos = [
        ("rede 1.2.3 255.255.255.0 10.0.2.2", "1.2.3"),
        ("rede 0.0.0.0 255.255.255.0 10.0.2.2", "0.0.0.0"),
        ("rede 255.255.255.255 255.255.255.0 10.0.2.2", "255.255.255.255"),
        ("rede 127.0.0.1 255.255.255.0 127.0.0.2", "127.0.0.1"),
        ("rede 224.0.0.1 255.255.255.0 224.0.0.2", "224.0.0.1"),
        ("rede 10.0.2.15 255.0.255.0 10.0.2.2", "255.0.255.0"),
        ("rede 10.0.2.15 255.255.255.0 10.0.3.1", "10.0.3.1"),
        ("rede 10.0.2.15 255.255.255.0 abc", "abc"),
    ];
    for (comando, texto) in casos {
        let esperado = alloc::format!("rede: endereco invalido: {}", texto);
        exige(&rodar_comando(comando), &esperado);
        exige(&rodar_comando("rede"), "ip: 10.0.2.15 mascara: 255.255.255.0 gateway: 10.0.2.2");
    }
    exige(&rodar_comando("rede 1.2.3.4"), "uso: rede [<ip> <mascara> <gateway>]");
    exige(&rodar_comando("rede 1.2.3.4 255.255.255.0"), "uso: rede [<ip> <mascara> <gateway>]");
    exige(&rodar_comando("rede a b c d"), "uso: rede [<ip> <mascara> <gateway>]");
}

// ---------------------------------------------------------------------------
// TCP pela placa: o eco do `guestfwd`
// ---------------------------------------------------------------------------

use os_rust::net::socket::SocketTable;
use os_rust::net::Outcome;

/// Termina uma operação de socket feita direto no kernel: anda a pilha e dorme
/// até o pedido poder andar. Quem chama ainda precisa dos ponteiros de kernel,
/// que a pilha trata como os do usuário (a validação é da syscall).
fn concluir(table: &mut SocketTable, outcome: Outcome) -> i64 {
    match outcome {
        Outcome::Done(result) => result,
        Outcome::Block(pending) => loop {
            x86_64::instructions::interrupts::disable();
            net::poll();
            let now = os_rust::timer::ticks();
            if let Some(result) = net::attempt(table, &pending, now) {
                x86_64::instructions::interrupts::enable();
                return result;
            }
            x86_64::instructions::interrupts::enable_and_hlt();
        },
    }
}

#[test_case]
fn eco_de_tcp_pelo_guestfwd_do_qemu() {
    let mut table = SocketTable::new();
    let agora = os_rust::timer::ticks();
    let outcome = net::sys_connect(&mut table, [10, 0, 2, 100], 7, agora);
    let sock = concluir(&mut table, outcome);
    assert!(sock >= 0, "connect: {}", sock);
    let enviado = *b"ola";
    let outcome = net::sys_send(&mut table, sock as u64, enviado.as_ptr() as u64, 3, os_rust::timer::ticks());
    assert_eq!(concluir(&mut table, outcome), 3);
    let mut recebido = [0u8; 16];
    let outcome = net::sys_recv(&mut table, sock as u64, recebido.as_mut_ptr() as u64, 16, os_rust::timer::ticks());
    let n = concluir(&mut table, outcome);
    assert_eq!(n, 3);
    assert_eq!(&recebido[..3], b"ola");
    assert_eq!(net::sys_close(&mut table, sock as u64), 0);
    drop(table);
    // O fim da conexão termina em segundo plano.
    let fim = os_rust::timer::ticks() + 600;
    loop {
        x86_64::instructions::interrupts::disable();
        net::poll();
        let em_uso = net::tcp_in_use();
        x86_64::instructions::interrupts::enable();
        if em_uso == 0 || os_rust::timer::ticks() > fim {
            assert_eq!(em_uso, 0);
            break;
        }
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}
