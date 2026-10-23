//! Testes da pilha de rede **sem hardware** (Marco 12): o checksum, cada
//! camada montada e interpretada byte a byte, o cache de ARP, o roteamento, o
//! loopback e a máquina de estados do TCP. A pilha roda com uma placa falsa
//! (`FakeNic`) e o relógio é um argumento (`now`), então a retransmissão exata
//! em 100 ticks e a queda depois da quinta provam-se sem esperar um segundo de
//! verdade. Cada arquivo de `tests/` é um kernel próprio; este fica fora do
//! binário `--lib` porque aquele já está perto do limite de tempo.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os_rust::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::vec::Vec;
use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use os_rust::net::fake::FakeNic;
use os_rust::net::tcp::{self, NetErr, State, Tx, ACK, FIN, PSH, RST, SYN};
use os_rust::net::{arp, checksum, eth, icmp, ip, parse_ipv4, valid_config, Stack};
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
// Ajudantes
// ---------------------------------------------------------------------------

const LOCAL_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
const PEER_MAC: [u8; 6] = [0x52, 0x55, 0x0a, 0x00, 0x02, 0x02];
const LOCAL_IP: [u8; 4] = [10, 0, 2, 15];
const PEER_IP: [u8; 4] = [10, 0, 2, 2];

static STACK: Mutex<Stack<FakeNic>> = Mutex::new(Stack::new(FakeNic::new(LOCAL_MAC)));

/// Roda `f` com a pilha de teste, recomeçando do zero.
fn fresh<R>(f: impl FnOnce(&mut Stack<FakeNic>) -> R) -> R {
    let mut stack = STACK.lock();
    stack.reset();
    stack.link.nic.clear();
    f(&mut stack)
}

fn take(stack: &mut Stack<FakeNic>) -> Option<Vec<u8>> {
    let mut buf = [0u8; 1514];
    let len = stack.link.nic.take_sent(&mut buf)?;
    Some(buf[..len].to_vec())
}

/// Monta um quadro Ethernet + IPv4.
fn ip_frame(dst_mac: [u8; 6], src_mac: [u8; 6], src: [u8; 4], dst: [u8; 4], proto: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = alloc::vec![0u8; eth::HEADER_LEN + ip::HEADER_LEN + payload.len()];
    eth::write_header(&mut frame, dst_mac, src_mac, eth::ETHERTYPE_IPV4);
    ip::write_header(&mut frame[eth::HEADER_LEN..], src, dst, proto, 1, payload.len());
    frame[eth::HEADER_LEN + ip::HEADER_LEN..].copy_from_slice(payload);
    frame
}

fn arp_frame(op: u16, dst_mac: [u8; 6], sender_mac: [u8; 6], sender_ip: [u8; 4], target_ip: [u8; 4]) -> Vec<u8> {
    let mut frame = alloc::vec![0u8; eth::HEADER_LEN + arp::PACKET_LEN];
    eth::write_header(&mut frame, dst_mac, sender_mac, eth::ETHERTYPE_ARP);
    let packet = arp::Packet {
        op,
        sender_mac,
        sender_ip,
        target_mac: if op == arp::OP_REPLY { dst_mac } else { [0; 6] },
        target_ip,
    };
    arp::write(&packet, &mut frame[eth::HEADER_LEN..]);
    frame
}

fn echo_request(id: u16, seq: u16, data: &[u8]) -> Vec<u8> {
    let mut out = alloc::vec![0u8; icmp::HEADER_LEN + data.len()];
    let len = icmp::write_echo(&mut out, icmp::ECHO_REQUEST, id, seq, data);
    out.truncate(len);
    out
}

/// Recalcula a soma do cabeçalho IPv4 depois de uma adulteração.
fn fix_ip_checksum(packet: &mut [u8]) {
    packet[10] = 0;
    packet[11] = 0;
    let sum = checksum::sum(&[&packet[..ip::HEADER_LEN]]);
    packet[10..12].copy_from_slice(&sum.to_be_bytes());
}

// ---------------------------------------------------------------------------
// Checksum, endereços e configuração
// ---------------------------------------------------------------------------

#[test_case]
fn checksum_confere_os_vetores_da_rfc_1071() {
    // RFC 1071, seção 3: a soma de 00 01 f2 03 f4 f5 f6 f7 é ddf2, ou seja, a
    // soma de verificação (invertida) é 220d.
    let bytes = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
    assert_eq!(checksum::sum(&[&bytes]), 0x220d);
    // Os pedaços podem vir separados, até no meio de uma palavra.
    assert_eq!(checksum::sum(&[&bytes[..3], &bytes[3..]]), 0x220d);
    // Tamanho ímpar: o último byte vale como se tivesse um zero depois.
    assert_eq!(checksum::sum(&[&[0x12, 0x34, 0x56]]), !(0x1234u16.wrapping_add(0x5600)));
    // Soma que dá vai-um: ffff + 0002 = 1_0001 -> 0002 (vai-um dobrado), invertida.
    assert_eq!(checksum::sum(&[&[0xff, 0xff, 0x00, 0x02]]), !0x0002u16);
    // Conferir um pacote com a soma gravada dá zero.
    let mut with_sum = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7, 0x22, 0x0d];
    assert!(checksum::verify(&[&with_sum]));
    with_sum[0] ^= 1;
    assert!(!checksum::verify(&[&with_sum]));
}

#[test_case]
fn checksum_do_cabecalho_ipv4_conhecido() {
    // 4500 0073 0000 4000 4011 ???? c0a8 0001 c0a8 00c7 -> soma b861.
    let header = [
        0x45, 0x00, 0x00, 0x73, 0x00, 0x00, 0x40, 0x00, 0x40, 0x11, 0x00, 0x00, 0xc0, 0xa8, 0x00,
        0x01, 0xc0, 0xa8, 0x00, 0xc7,
    ];
    assert_eq!(checksum::sum(&[&header]), 0xb861);
}

#[test_case]
fn enderecos_e_configuracao() {
    assert_eq!(parse_ipv4("10.0.2.15"), Some([10, 0, 2, 15]));
    for ruim in ["", "abc", "1.2.3", "1.2.3.4.5", "300.1.1.1", "1.2.3.-4", "1..3.4", " 1.2.3.4", "1.2.3.4 "] {
        assert_eq!(parse_ipv4(ruim), None, "{:?}", ruim);
    }
    let mask = [255, 255, 255, 0];
    assert_eq!(valid_config([192, 168, 0, 50], mask, [192, 168, 0, 1]), Ok(()));
    // ip inválido (argumento 0)
    for ip in [[0, 0, 0, 0], [255, 255, 255, 255], [127, 0, 0, 1], [224, 0, 0, 1], [239, 1, 1, 1]] {
        assert_eq!(valid_config(ip, mask, [10, 0, 2, 2]), Err(0), "{:?}", ip);
    }
    // máscara não contígua ou degenerada (argumento 1)
    for m in [[255, 0, 255, 0], [0, 255, 255, 255], [0, 0, 0, 0], [255, 255, 255, 255], [255, 255, 255, 253]] {
        assert_eq!(valid_config([10, 0, 2, 15], m, [10, 0, 2, 2]), Err(1), "{:?}", m);
    }
    // gateway fora da sub-rede, igual ao ip, ou zero (argumento 2)
    assert_eq!(valid_config([10, 0, 2, 15], mask, [10, 0, 3, 1]), Err(2));
    assert_eq!(valid_config([10, 0, 2, 15], mask, [10, 0, 2, 15]), Err(2));
    assert_eq!(valid_config([10, 0, 2, 15], mask, [0, 0, 0, 0]), Err(2));
}

// ---------------------------------------------------------------------------
// Camadas: Ethernet, ARP, IPv4, ICMP
// ---------------------------------------------------------------------------

#[test_case]
fn ethernet_ida_e_volta_e_rejeicoes() {
    let mut out = [0u8; 20];
    eth::write_header(&mut out, [1, 2, 3, 4, 5, 6], [7, 8, 9, 10, 11, 12], eth::ETHERTYPE_ARP);
    out[14..].copy_from_slice(&[9; 6]);
    let frame = eth::parse(&out).expect("quadro valido");
    assert_eq!(frame.dst, [1, 2, 3, 4, 5, 6]);
    assert_eq!(frame.src, [7, 8, 9, 10, 11, 12]);
    assert_eq!(frame.ethertype, 0x0806);
    assert_eq!(frame.payload, &[9; 6]);
    assert!(eth::parse(&out[..13]).is_none());
}

#[test_case]
fn arp_ida_e_volta_byte_a_byte() {
    let packet = arp::Packet {
        op: arp::OP_REQUEST,
        sender_mac: LOCAL_MAC,
        sender_ip: LOCAL_IP,
        target_mac: [0; 6],
        target_ip: PEER_IP,
    };
    let mut out = [0u8; arp::PACKET_LEN];
    arp::write(&packet, &mut out);
    assert_eq!(&out[..8], &[0, 1, 8, 0, 6, 4, 0, 1]);
    assert_eq!(&out[8..14], &LOCAL_MAC);
    assert_eq!(&out[14..18], &LOCAL_IP);
    assert_eq!(&out[24..28], &PEER_IP);
    assert_eq!(arp::parse(&out), Some(packet));
    // Formatos que não são Ethernet/IPv4 são recusados.
    let mut bad = out;
    bad[1] = 6; // hardware diferente
    assert!(arp::parse(&bad).is_none());
    let mut bad = out;
    bad[5] = 16; // tamanho de protocolo
    assert!(arp::parse(&bad).is_none());
    let mut bad = out;
    bad[7] = 9; // operação desconhecida
    assert!(arp::parse(&bad).is_none());
    assert!(arp::parse(&out[..27]).is_none());
}

#[test_case]
fn cache_de_arp_aprende_substitui_a_mais_antiga_e_esvazia() {
    let mut cache = arp::Cache::new();
    for i in 0..8u8 {
        cache.learn([10, 0, 2, i], [i; 6]);
    }
    assert_eq!(cache.len(), 8);
    // Aprender de novo atualiza sem ocupar outra entrada.
    cache.learn([10, 0, 2, 3], [0xAA; 6]);
    assert_eq!(cache.len(), 8);
    assert_eq!(cache.lookup([10, 0, 2, 3]), Some([0xAA; 6]));
    // A nona entrada tira a mais antiga: a de .0 (a .3 foi renovada).
    cache.learn([10, 0, 2, 99], [9; 6]);
    assert_eq!(cache.len(), 8);
    assert_eq!(cache.lookup([10, 0, 2, 0]), None);
    assert_eq!(cache.lookup([10, 0, 2, 1]), Some([1; 6]));
    assert_eq!(cache.lookup([10, 0, 2, 99]), Some([9; 6]));
    cache.clear();
    assert!(cache.is_empty());
}

#[test_case]
fn ipv4_monta_o_cabecalho_e_rejeita_cada_defeito() {
    let payload = [1u8, 2, 3, 4, 5];
    let mut packet = alloc::vec![0u8; ip::HEADER_LEN + payload.len()];
    ip::write_header(&mut packet, PEER_IP, LOCAL_IP, ip::PROTO_ICMP, 7, payload.len());
    packet[ip::HEADER_LEN..].copy_from_slice(&payload);
    assert_eq!(&packet[..2], &[0x45, 0x00]);
    assert_eq!(u16::from_be_bytes([packet[2], packet[3]]) as usize, packet.len());
    assert_eq!(&packet[6..8], &[0x40, 0x00]); // DF
    assert_eq!(packet[8], 64); // TTL
    assert_eq!(packet[9], ip::PROTO_ICMP);
    let parsed = ip::parse(&packet).expect("valido");
    assert_eq!((parsed.src, parsed.dst, parsed.protocol), (PEER_IP, LOCAL_IP, ip::PROTO_ICMP));
    assert_eq!(parsed.payload, &payload);

    // O preenchimento do Ethernet depois do comprimento total é cortado.
    let mut padded = packet.clone();
    padded.extend_from_slice(&[0; 20]);
    assert_eq!(ip::parse(&padded).unwrap().payload, &payload);

    assert_eq!(ip::parse(&packet[..19]), Err(ip::IpError::TooShort));
    let mut bad = packet.clone();
    bad[0] = 0x65;
    assert_eq!(ip::parse(&bad), Err(ip::IpError::BadVersion));
    let mut bad = packet.clone();
    bad[0] = 0x44;
    assert_eq!(ip::parse(&bad), Err(ip::IpError::BadHeaderLength));
    let mut bad = packet.clone();
    bad[2..4].copy_from_slice(&200u16.to_be_bytes());
    assert_eq!(ip::parse(&bad), Err(ip::IpError::BadTotalLength));
    let mut bad = packet.clone();
    bad[4] ^= 1; // muda o id sem refazer a soma
    assert_eq!(ip::parse(&bad), Err(ip::IpError::BadChecksum));
    let mut bad = packet.clone();
    bad[6] = 0x60; // DF + MF
    fix_ip_checksum(&mut bad);
    assert_eq!(ip::parse(&bad), Err(ip::IpError::Fragment));
    let mut bad = packet.clone();
    bad[7] = 0x08; // deslocamento diferente de zero
    fix_ip_checksum(&mut bad);
    assert_eq!(ip::parse(&bad), Err(ip::IpError::Fragment));
}

#[test_case]
fn icmp_eco_ida_e_volta_e_soma_errada() {
    let data = *b"abcdefgh";
    let request = echo_request(0x1234, 9, &data);
    let echo = icmp::parse(&request).expect("valido");
    assert_eq!((echo.kind, echo.id, echo.seq, echo.data), (icmp::ECHO_REQUEST, 0x1234, 9, &data[..]));
    let mut bad = request.clone();
    bad[8] ^= 1;
    assert!(icmp::parse(&bad).is_none());
    // Tipos que não são eco são ignorados (aqui, destino inalcançável).
    let mut other = request.clone();
    other[0] = 3;
    other[2] = 0;
    other[3] = 0;
    let sum = checksum::sum(&[&other]);
    other[2..4].copy_from_slice(&sum.to_be_bytes());
    assert!(icmp::parse(&other).is_none());
}

#[test_case]
fn arp_pedido_para_o_nosso_ip_e_respondido_e_aprendido() {
    fresh(|s| {
        s.link.nic.inject(&arp_frame(arp::OP_REQUEST, eth::BROADCAST, PEER_MAC, PEER_IP, LOCAL_IP));
        s.poll(0);
        let reply = take(s).expect("resposta ARP");
        let frame = eth::parse(&reply).unwrap();
        assert_eq!((frame.dst, frame.src, frame.ethertype), (PEER_MAC, LOCAL_MAC, eth::ETHERTYPE_ARP));
        let packet = arp::parse(frame.payload).unwrap();
        assert_eq!(packet.op, arp::OP_REPLY);
        assert_eq!((packet.sender_mac, packet.sender_ip), (LOCAL_MAC, LOCAL_IP));
        assert_eq!((packet.target_mac, packet.target_ip), (PEER_MAC, PEER_IP));
        assert_eq!(s.link.arp.lookup(PEER_IP), Some(PEER_MAC));
        assert!(take(s).is_none());
    });
}

#[test_case]
fn arp_pedido_para_outro_ip_e_ignorado_e_quadro_alheio_e_descartado() {
    fresh(|s| {
        s.link.nic.inject(&arp_frame(arp::OP_REQUEST, eth::BROADCAST, PEER_MAC, PEER_IP, [10, 0, 2, 99]));
        s.poll(0);
        assert!(take(s).is_none());
        assert_eq!(s.link.arp.lookup(PEER_IP), None);
        // Quadro para o MAC de outra máquina: descartado e contado.
        let frame = ip_frame([1, 2, 3, 4, 5, 6], PEER_MAC, PEER_IP, LOCAL_IP, ip::PROTO_ICMP, &echo_request(1, 1, b"x"));
        let before = s.link.stats.dropped;
        s.link.nic.inject(&frame);
        s.poll(1);
        assert_eq!(s.link.stats.dropped, before + 1);
        assert!(take(s).is_none());
    });
}

#[test_case]
fn eco_recebido_e_respondido_com_os_mesmos_dados() {
    fresh(|s| {
        s.link.arp.learn(PEER_IP, PEER_MAC);
        let data = b"0123456789abcdef";
        s.link.nic.inject(&ip_frame(LOCAL_MAC, PEER_MAC, PEER_IP, LOCAL_IP, ip::PROTO_ICMP, &echo_request(0x42, 5, data)));
        s.poll(0);
        let reply = take(s).expect("resposta de eco");
        let frame = eth::parse(&reply).unwrap();
        assert_eq!((frame.dst, frame.src, frame.ethertype), (PEER_MAC, LOCAL_MAC, eth::ETHERTYPE_IPV4));
        let packet = ip::parse(frame.payload).unwrap();
        assert_eq!((packet.src, packet.dst, packet.protocol), (LOCAL_IP, PEER_IP, ip::PROTO_ICMP));
        let echo = icmp::parse(packet.payload).unwrap();
        assert_eq!((echo.kind, echo.id, echo.seq, echo.data), (icmp::ECHO_REPLY, 0x42, 5, &data[..]));
    });
}

#[test_case]
fn pacotes_invalidos_sao_descartados_e_contados_sem_resposta() {
    fresh(|s| {
        s.link.arp.learn(PEER_IP, PEER_MAC);
        let good = ip_frame(LOCAL_MAC, PEER_MAC, PEER_IP, LOCAL_IP, ip::PROTO_ICMP, &echo_request(1, 1, b"abcd"));
        let mut cases: Vec<Vec<u8>> = Vec::new();
        let mut bad = good.clone();
        bad[eth::HEADER_LEN] = 0x65; // versão
        cases.push(bad);
        let mut bad = good.clone();
        bad[eth::HEADER_LEN + 4] ^= 1; // soma do IP
        cases.push(bad);
        let mut bad = good.clone();
        bad[eth::HEADER_LEN + 6] = 0x20; // MF
        fix_ip_checksum(&mut bad[eth::HEADER_LEN..]);
        cases.push(bad);
        let mut bad = good.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1; // soma do ICMP
        cases.push(bad);
        // Destino que não é o nosso.
        cases.push(ip_frame(LOCAL_MAC, PEER_MAC, PEER_IP, [10, 0, 2, 77], ip::PROTO_ICMP, &echo_request(1, 1, b"abcd")));
        // Protocolo que a pilha não conhece (UDP).
        cases.push(ip_frame(LOCAL_MAC, PEER_MAC, PEER_IP, LOCAL_IP, 17, &[0; 8]));
        // Quadro curto demais.
        cases.push(alloc::vec![0u8; 5]);
        let count = cases.len() as u32;
        for case in &cases {
            s.link.nic.inject(case);
            s.poll(0);
        }
        assert_eq!(s.link.stats.dropped, count);
        assert!(take(s).is_none());
    });
}

#[test_case]
fn roteamento_resolve_por_arp_no_proximo_salto_e_desiste_depois_de_tres() {
    fresh(|s| {
        // Na sub-rede: pergunta pelo próprio destino.
        assert!(s.send_echo([10, 0, 2, 77], 1, 1, 0));
        let request = take(s).expect("pedido ARP");
        let frame = eth::parse(&request).unwrap();
        assert_eq!((frame.dst, frame.ethertype), (eth::BROADCAST, eth::ETHERTYPE_ARP));
        assert_eq!(arp::parse(frame.payload).unwrap().target_ip, [10, 0, 2, 77]);
        // Um segundo pacote enquanto espera é descartado (só um pendente).
        let dropped = s.link.stats.dropped;
        assert!(s.send_echo([10, 0, 2, 77], 1, 2, 1));
        assert_eq!(s.link.stats.dropped, dropped + 1);
        assert!(take(s).is_none());
        // A cada 100 ticks pergunta de novo; são 3 perguntas no total.
        s.poll(99);
        assert!(take(s).is_none());
        s.poll(100);
        assert!(take(s).is_some());
        s.poll(200);
        assert!(take(s).is_some());
        s.poll(299);
        assert!(take(s).is_none());
        let dropped = s.link.stats.dropped;
        s.poll(300);
        assert!(take(s).is_none());
        assert_eq!(s.link.stats.dropped, dropped + 1);
        // Livre de novo: fora da sub-rede pergunta pelo gateway.
        assert!(s.send_echo([8, 8, 8, 8], 1, 3, 400));
        let request = take(s).expect("pedido ARP do gateway");
        let frame = eth::parse(&request).unwrap();
        assert_eq!(arp::parse(frame.payload).unwrap().target_ip, PEER_IP);
    });
}

#[test_case]
fn resposta_do_arp_libera_o_pacote_que_esperava() {
    fresh(|s| {
        assert!(s.send_echo(PEER_IP, 7, 3, 0));
        assert!(take(s).is_some()); // pedido ARP
        s.link.nic.inject(&arp_frame(arp::OP_REPLY, LOCAL_MAC, PEER_MAC, PEER_IP, LOCAL_IP));
        s.poll(1);
        let sent = take(s).expect("o eco saiu depois da resposta");
        let frame = eth::parse(&sent).unwrap();
        assert_eq!((frame.dst, frame.src, frame.ethertype), (PEER_MAC, LOCAL_MAC, eth::ETHERTYPE_IPV4));
        let packet = ip::parse(frame.payload).unwrap();
        let echo = icmp::parse(packet.payload).unwrap();
        assert_eq!((echo.kind, echo.id, echo.seq, echo.data.len()), (icmp::ECHO_REQUEST, 7, 3, 32));
    });
}

#[test_case]
fn loopback_responde_sem_placa_e_sem_quadros() {
    fresh(|s| {
        s.link.nic.present = false;
        assert!(s.send_echo([127, 0, 0, 1], 0x55, 2, 5));
        s.poll(5);
        assert_eq!(s.link.echo_reply, Some((0x55, 2, 5)));
        assert_eq!(s.link.stats.tx, 0);
        assert!(take(s).is_none());
        // Fora do loopback, sem placa, não dá.
        assert!(!s.send_echo(PEER_IP, 1, 1, 6));
    });
}

#[test_case]
fn fila_do_loopback_guarda_oito_e_descarta_o_resto() {
    fresh(|s| {
        for _ in 0..9 {
            s.link.send_ip([127, 0, 0, 1], [127, 0, 0, 1], ip::PROTO_ICMP, &[0; 8]);
        }
        assert_eq!(s.link.loopq.len(), 8);
        assert_eq!(s.link.stats.dropped, 1);
    });
}

// ---------------------------------------------------------------------------
// TCP: a máquina de estados, com um "par" feito à mão
// ---------------------------------------------------------------------------

/// Captura os segmentos que o TCP envia.
struct Capture {
    sent: Vec<(Vec<u8>, [u8; 4], [u8; 4])>,
}

impl Capture {
    fn new() -> Capture {
        Capture { sent: Vec::new() }
    }

    fn len(&self) -> usize {
        self.sent.len()
    }

    /// O segmento `i` interpretado (com os campos que os testes conferem).
    fn seg(&self, i: usize) -> (u8, u32, u32, u16, Vec<u8>) {
        let (bytes, src, dst) = &self.sent[i];
        let parsed = tcp::parse(*src, *dst, bytes).expect("segmento valido");
        (parsed.flags, parsed.seq, parsed.ack, parsed.window, parsed.payload.to_vec())
    }

    fn last(&self) -> (u8, u32, u32, u16, Vec<u8>) {
        self.seg(self.sent.len() - 1)
    }

    fn clear(&mut self) {
        self.sent.clear();
    }
}

impl Tx for Capture {
    fn send(&mut self, src: [u8; 4], dst: [u8; 4], segment: &[u8]) {
        self.sent.push((segment.to_vec(), src, dst));
    }
}

const ME: [u8; 4] = [10, 0, 2, 15];
const HOST: [u8; 4] = [10, 0, 2, 2];

/// Injeta um segmento "do par" (HOST:`peer_port` para ME:`my_port`).
#[allow(clippy::too_many_arguments)]
fn inject(t: &mut tcp::Tcp, cap: &mut Capture, now: u64, peer_port: u16, my_port: u16, seq: u32, ack: u32, flags: u8, window: u16, payload: &[u8]) {
    let mut buf = alloc::vec![0u8; tcp::HEADER_LEN + payload.len()];
    let len = tcp::build_segment(&mut buf, HOST, ME, (peer_port, my_port), seq, ack, flags, window, payload);
    t.on_segment(HOST, ME, &buf[..len], now, cap);
}

/// Abre uma conexão ativa até `Established`. Devolve (índice, iss, irs).
fn establish_active(t: &mut tcp::Tcp, cap: &mut Capture, now: u64) -> (usize, u32, u32) {
    cap.clear();
    let index = t.connect(ME, HOST, 80, now, cap).expect("connect");
    let (flags, iss, _, _, _) = cap.seg(0);
    assert_eq!(flags, SYN);
    let irs = 5000;
    let port = t.tcbs[index].local_port;
    inject(t, cap, now, 80, port, irs, iss.wrapping_add(1), SYN | ACK, 4096, &[]);
    assert_eq!(t.tcbs[index].state, State::Established);
    cap.clear();
    (index, iss, irs)
}

#[test_case]
fn tcp_aperto_de_mao_ativo_e_troca_de_dados_nos_dois_sentidos() {
    let mut t = tcp::Tcp::new();
    let mut cap = Capture::new();
    let index = t.connect(ME, HOST, 80, 10, &mut cap).unwrap();
    assert_eq!(cap.len(), 1);
    let (flags, iss, ack, _, payload) = cap.seg(0);
    assert_eq!((flags, ack, payload.len()), (SYN, 0, 0));
    assert_eq!(t.tcbs[index].state, State::SynSent);
    assert!(t.tcbs[index].local_port >= 49152);
    let port = t.tcbs[index].local_port;
    // SYN+ACK do par -> Established, e o terceiro segmento do aperto (ACK).
    inject(&mut t, &mut cap, 11, 80, port, 5000, iss + 1, SYN | ACK, 4096, &[]);
    assert_eq!(t.tcbs[index].state, State::Established);
    let (flags, seq, ack, window, _) = cap.last();
    assert_eq!((flags, seq, ack, window), (ACK, iss + 1, 5001, 2048));
    cap.clear();

    // Dados daqui para lá: um segmento, com PSH|ACK.
    assert_eq!(t.send(index, b"ola", 12, &mut cap), 3);
    let (flags, seq, ack, _, payload) = cap.last();
    assert_eq!((flags, seq, ack, payload.as_slice()), (PSH | ACK, iss + 1, 5001, &b"ola"[..]));
    // O par confirma: o buffer de envio esvazia.
    inject(&mut t, &mut cap, 13, 80, port, 5001, iss + 4, ACK, 4096, &[]);
    assert_eq!(t.tcbs[index].in_flight, 0);
    assert!(t.tcbs[index].snd.is_empty());
    cap.clear();

    // Dados de lá para cá: confirmados na hora, e o programa lê.
    inject(&mut t, &mut cap, 14, 80, port, 5001, iss + 4, PSH | ACK, 4096, b"mundo");
    let (flags, _, ack, window, _) = cap.last();
    assert_eq!((flags, ack, window), (ACK, 5006, 2048 - 5));
    let mut out = [0u8; 16];
    assert_eq!(t.recv(index, &mut out, 15, &mut cap), 5);
    assert_eq!(&out[..5], b"mundo");
}

#[test_case]
fn tcp_aperto_de_mao_passivo_e_fila_de_aceitacao() {
    let mut t = tcp::Tcp::new();
    let mut cap = Capture::new();
    let listener = t.listen(8000).unwrap();
    assert_eq!(t.listen(8000), Err(tcp::Error::AddrInUse));
    inject(&mut t, &mut cap, 0, 40000, 8000, 700, 0, SYN, 4096, &[]);
    let (flags, iss, ack, _, _) = cap.last();
    assert_eq!((flags, ack), (SYN | ACK, 701));
    let child = t.tcbs.iter().position(|c| c.state == State::SynReceived).unwrap();
    assert_eq!(t.accept(listener), None, "ainda no aperto de mao");
    inject(&mut t, &mut cap, 1, 40000, 8000, 701, iss + 1, ACK, 4096, &[]);
    assert_eq!(t.tcbs[child].state, State::Established);
    assert_eq!(t.accept(listener), Some(child));
    assert!(t.tcbs[child].owned);

    // A fila guarda 2: o terceiro SYN é ignorado (o par o retransmite).
    inject(&mut t, &mut cap, 2, 40001, 8000, 10, 0, SYN, 4096, &[]);
    inject(&mut t, &mut cap, 2, 40002, 8000, 20, 0, SYN, 4096, &[]);
    cap.clear();
    inject(&mut t, &mut cap, 2, 40003, 8000, 30, 0, SYN, 4096, &[]);
    assert_eq!(cap.len(), 0, "o terceiro SYN e ignorado");
}

#[test_case]
fn tcp_segmento_fora_de_ordem_e_duplicado_so_reconfirma() {
    let mut t = tcp::Tcp::new();
    let mut cap = Capture::new();
    let (index, iss, irs) = establish_active(&mut t, &mut cap, 0);
    let port = t.tcbs[index].local_port;
    // Adiantado: nada entregue, ACK do último byte em ordem.
    inject(&mut t, &mut cap, 1, 80, port, irs + 1 + 100, iss + 1, ACK, 4096, b"adiantado");
    assert!(t.tcbs[index].rcv.is_empty());
    let (flags, _, ack, _, _) = cap.last();
    assert_eq!((flags, ack), (ACK, irs + 1));
    cap.clear();
    // Em ordem: entregue.
    inject(&mut t, &mut cap, 2, 80, port, irs + 1, iss + 1, ACK, 4096, b"abc");
    assert_eq!(t.tcbs[index].rcv.len(), 3);
    cap.clear();
    // Duplicado: não entrega de novo, reconfirma.
    inject(&mut t, &mut cap, 3, 80, port, irs + 1, iss + 1, ACK, 4096, b"abc");
    assert_eq!(t.tcbs[index].rcv.len(), 3);
    let (_, _, ack, _, _) = cap.last();
    assert_eq!(ack, irs + 4);
}

#[test_case]
fn tcp_retransmite_em_100_ticks_exatos_cinco_vezes_e_depois_cai() {
    let mut t = tcp::Tcp::new();
    let mut cap = Capture::new();
    let (index, iss, _) = establish_active(&mut t, &mut cap, 1000);
    assert_eq!(t.send(index, b"dados", 1000, &mut cap), 5);
    assert_eq!(cap.len(), 1);
    let original = cap.sent[0].0.clone();
    for k in 1..=5u64 {
        t.on_tick(1000 + 100 * k - 1, &mut cap);
        assert_eq!(cap.len() as u64, k, "nada antes de 100 ticks (k = {})", k);
        t.on_tick(1000 + 100 * k, &mut cap);
        assert_eq!(cap.len() as u64, k + 1, "reenvio em 100 ticks (k = {})", k);
        // O mesmo segmento, byte a byte.
        assert_eq!(cap.sent[k as usize].0, original);
    }
    assert_eq!(t.tcbs[index].state, State::Established);
    // A sexta expiração derruba a conexão.
    t.on_tick(1000 + 600, &mut cap);
    assert_eq!(cap.len(), 6, "nenhum reenvio alem do quinto");
    assert_eq!(t.tcbs[index].state, State::Closed);
    assert_eq!(t.tcbs[index].err, Some(NetErr::TimedOut));
    assert!(t.tcbs[index].used, "do dono ate o close");
    let _ = iss;
    t.close(index, 1700, &mut cap);
    assert_eq!(t.in_use(), 0);
}

#[test_case]
fn tcp_connect_manda_tres_syn_um_por_segundo_e_cai_no_terceiro_segundo() {
    let mut t = tcp::Tcp::new();
    let mut cap = Capture::new();
    let index = t.connect(ME, [10, 0, 2, 77], 80, 0, &mut cap).unwrap();
    assert_eq!(cap.len(), 1);
    t.on_tick(99, &mut cap);
    assert_eq!(cap.len(), 1);
    t.on_tick(100, &mut cap);
    assert_eq!(cap.len(), 2);
    t.on_tick(200, &mut cap);
    assert_eq!(cap.len(), 3);
    assert_eq!(t.tcbs[index].state, State::SynSent);
    t.on_tick(299, &mut cap);
    assert_eq!(t.tcbs[index].state, State::SynSent);
    t.on_tick(300, &mut cap);
    assert_eq!(cap.len(), 3, "so tres SYN");
    assert_eq!(t.tcbs[index].err, Some(NetErr::TimedOut));
    // Os três SYN são o mesmo segmento.
    assert_eq!(cap.sent[0].0, cap.sent[1].0);
    assert_eq!(cap.sent[1].0, cap.sent[2].0);
}

#[test_case]
fn tcp_connect_recusado_por_rst() {
    let mut t = tcp::Tcp::new();
    let mut cap = Capture::new();
    let index = t.connect(ME, HOST, 80, 0, &mut cap).unwrap();
    let (_, iss, _, _, _) = cap.seg(0);
    let port = t.tcbs[index].local_port;
    inject(&mut t, &mut cap, 1, 80, port, 0, iss + 1, RST | ACK, 0, &[]);
    assert_eq!(t.tcbs[index].err, Some(NetErr::ConnRefused));
    assert_eq!(t.tcbs[index].state, State::Closed);
}

#[test_case]
fn tcp_syn_sem_listen_e_segmento_sem_conexao_levam_rst() {
    let mut t = tcp::Tcp::new();
    let mut cap = Capture::new();
    // SYN para porta sem listen: RST+ACK com ack = seq + 1.
    inject(&mut t, &mut cap, 0, 1234, 9999, 100, 0, SYN, 4096, &[]);
    let (flags, seq, ack, _, _) = cap.last();
    assert_eq!((flags, seq, ack), (RST | ACK, 0, 101));
    cap.clear();
    // Segmento com ACK para ninguém: RST com seq = ack do segmento.
    inject(&mut t, &mut cap, 0, 1234, 9999, 100, 777, ACK, 4096, b"xx");
    let (flags, seq, _, _, _) = cap.last();
    assert_eq!((flags, seq), (RST, 777));
    cap.clear();
    // Segmento sem ACK com dados: RST+ACK com ack = seq + tamanho.
    inject(&mut t, &mut cap, 0, 1234, 9999, 100, 0, PSH, 4096, b"xyz");
    let (flags, _, ack, _, _) = cap.last();
    assert_eq!((flags, ack), (RST | ACK, 103));
    cap.clear();
    // Um RST nunca recebe RST.
    inject(&mut t, &mut cap, 0, 1234, 9999, 100, 0, RST, 0, &[]);
    assert_eq!(cap.len(), 0);
    // Soma de verificação errada: descartado em silêncio.
    let mut buf = [0u8; 20];
    let len = tcp::build_segment(&mut buf, HOST, ME, (1234, 9999), 1, 0, SYN, 0, &[]);
    buf[16] ^= 0xFF;
    t.on_segment(HOST, ME, &buf[..len], 0, &mut cap);
    assert_eq!(cap.len(), 0);
}

#[test_case]
fn tcp_rst_derruba_a_conexao_em_cada_estado() {
    // Established -> ConnReset (o dono vê o erro).
    let mut t = tcp::Tcp::new();
    let mut cap = Capture::new();
    let (index, iss, irs) = establish_active(&mut t, &mut cap, 0);
    let port = t.tcbs[index].local_port;
    inject(&mut t, &mut cap, 1, 80, port, irs + 1, iss + 1, RST | ACK, 0, &[]);
    assert_eq!(t.tcbs[index].state, State::Closed);
    assert_eq!(t.tcbs[index].err, Some(NetErr::ConnReset));

    // RST com número fora da janela é ignorado.
    let mut t = tcp::Tcp::new();
    let (index, iss, irs) = establish_active(&mut t, &mut cap, 0);
    let port = t.tcbs[index].local_port;
    inject(&mut t, &mut cap, 1, 80, port, irs + 1 + 100_000, iss + 1, RST, 0, &[]);
    assert_eq!(t.tcbs[index].state, State::Established);

    // SynReceived: o bloco some em silêncio.
    let mut t = tcp::Tcp::new();
    t.listen(8000).unwrap();
    inject(&mut t, &mut cap, 0, 40000, 8000, 700, 0, SYN, 4096, &[]);
    assert_eq!(t.in_use(), 2);
    inject(&mut t, &mut cap, 1, 40000, 8000, 701, 0, RST, 0, &[]);
    assert_eq!(t.in_use(), 1);

    // CloseWait, FinWait2 e TimeWait.
    for target in [State::CloseWait, State::FinWait2, State::TimeWait] {
        let mut t = tcp::Tcp::new();
        let (index, iss, irs) = establish_active(&mut t, &mut cap, 0);
        let port = t.tcbs[index].local_port;
        match target {
            State::CloseWait => {
                inject(&mut t, &mut cap, 1, 80, port, irs + 1, iss + 1, FIN | ACK, 4096, &[]);
            }
            State::FinWait2 => {
                t.close(index, 1, &mut cap);
                inject(&mut t, &mut cap, 2, 80, port, irs + 1, iss + 2, ACK, 4096, &[]);
            }
            _ => {
                t.close(index, 1, &mut cap);
                inject(&mut t, &mut cap, 2, 80, port, irs + 1, iss + 2, FIN | ACK, 4096, &[]);
            }
        }
        assert_eq!(t.tcbs[index].state, target);
        inject(&mut t, &mut cap, 3, 80, port, irs + 2, iss + 2, RST, 0, &[]);
        match target {
            State::CloseWait => {
                assert_eq!(t.tcbs[index].err, Some(NetErr::ConnReset));
            }
            _ => {
                // Sem dono (o programa fechou): o bloco é liberado.
                assert!(!t.tcbs[index].used, "{:?}", target);
            }
        }
    }
}

#[test_case]
fn tcp_fechamento_ativo_ate_time_wait_de_100_ticks() {
    let mut t = tcp::Tcp::new();
    let mut cap = Capture::new();
    let (index, iss, irs) = establish_active(&mut t, &mut cap, 0);
    let port = t.tcbs[index].local_port;
    t.close(index, 10, &mut cap);
    let (flags, seq, ack, _, _) = cap.last();
    assert_eq!((flags, seq, ack), (FIN | ACK, iss + 1, irs + 1));
    assert_eq!(t.tcbs[index].state, State::FinWait1);
    inject(&mut t, &mut cap, 11, 80, port, irs + 1, iss + 2, ACK, 4096, &[]);
    assert_eq!(t.tcbs[index].state, State::FinWait2);
    cap.clear();
    inject(&mut t, &mut cap, 12, 80, port, irs + 1, iss + 2, FIN | ACK, 4096, &[]);
    assert_eq!(t.tcbs[index].state, State::TimeWait);
    let (flags, _, ack, _, _) = cap.last();
    assert_eq!((flags, ack), (ACK, irs + 2));
    // TIME_WAIT dura exatamente 100 ticks.
    t.on_tick(12 + 99, &mut cap);
    assert_eq!(t.tcbs[index].state, State::TimeWait);
    assert_eq!(t.in_use(), 1);
    t.on_tick(12 + 100, &mut cap);
    assert_eq!(t.in_use(), 0);
}

#[test_case]
fn tcp_fechamento_passivo_e_fechamento_simultaneo() {
    // O par fecha primeiro: CloseWait, depois LastAck, depois o bloco some.
    let mut t = tcp::Tcp::new();
    let mut cap = Capture::new();
    let (index, iss, irs) = establish_active(&mut t, &mut cap, 0);
    let port = t.tcbs[index].local_port;
    inject(&mut t, &mut cap, 1, 80, port, irs + 1, iss + 1, FIN | ACK, 4096, &[]);
    assert_eq!(t.tcbs[index].state, State::CloseWait);
    // O programa ainda lê o fim: recv devolve 0 (nada resta) e peer_fin vale.
    assert!(t.tcbs[index].peer_fin);
    t.close(index, 2, &mut cap);
    assert_eq!(t.tcbs[index].state, State::LastAck);
    let (flags, seq, _, _, _) = cap.last();
    assert_eq!((flags, seq), (FIN | ACK, iss + 1));
    inject(&mut t, &mut cap, 3, 80, port, irs + 2, iss + 2, ACK, 4096, &[]);
    assert_eq!(t.in_use(), 0);

    // Simultâneo: FIN do par chega antes do ACK do nosso FIN.
    let mut t = tcp::Tcp::new();
    let (index, iss, irs) = establish_active(&mut t, &mut cap, 0);
    let port = t.tcbs[index].local_port;
    t.close(index, 1, &mut cap);
    assert_eq!(t.tcbs[index].state, State::FinWait1);
    inject(&mut t, &mut cap, 2, 80, port, irs + 1, iss + 1, FIN | ACK, 4096, &[]);
    assert_eq!(t.tcbs[index].state, State::Closing);
    inject(&mut t, &mut cap, 3, 80, port, irs + 2, iss + 2, ACK, 4096, &[]);
    assert_eq!(t.tcbs[index].state, State::TimeWait);
}

#[test_case]
fn tcp_envia_um_segmento_por_vez_e_so_depois_o_proximo() {
    let mut t = tcp::Tcp::new();
    let mut cap = Capture::new();
    let (index, iss, _) = establish_active(&mut t, &mut cap, 0);
    let port = t.tcbs[index].local_port;
    let data = alloc::vec![7u8; 1500];
    assert_eq!(t.send(index, &data, 1, &mut cap), 1500);
    assert_eq!(cap.len(), 1, "um segmento so");
    assert_eq!(cap.last().4.len(), 1024);
    inject(&mut t, &mut cap, 2, 80, port, 5001, iss + 1 + 1024, ACK, 4096, &[]);
    assert_eq!(cap.len(), 2);
    assert_eq!(cap.last().4.len(), 476);
    // O buffer de envio é de 2048 bytes: o resto é recusado.
    let many = alloc::vec![1u8; 4000];
    let accepted = t.send(index, &many, 3, &mut cap);
    assert_eq!(accepted, 2048 - 476);
}

#[test_case]
fn tcp_janela_zero_sonda_com_um_byte_sem_derrubar_a_conexao() {
    let mut t = tcp::Tcp::new();
    let mut cap = Capture::new();
    let index = t.connect(ME, HOST, 80, 0, &mut cap).unwrap();
    let (_, iss, _, _, _) = cap.seg(0);
    let port = t.tcbs[index].local_port;
    // O par abre a conexão anunciando janela zero.
    inject(&mut t, &mut cap, 0, 80, port, 5000, iss + 1, SYN | ACK, 0, &[]);
    cap.clear();
    assert_eq!(t.send(index, b"abcdef", 1, &mut cap), 6);
    assert_eq!(cap.last().4, b"a".to_vec(), "sonda de 1 byte");
    // Muitas sondas, bem mais que 5: a conexão não cai.
    for k in 1..=20u64 {
        t.on_tick(1 + 100 * k, &mut cap);
    }
    assert_eq!(t.tcbs[index].state, State::Established);
    assert_eq!(cap.len(), 21);
    // O par confirma o byte e abre a janela: o resto segue.
    inject(&mut t, &mut cap, 2100, 80, port, 5001, iss + 2, ACK, 4096, &[]);
    assert_eq!(cap.last().4, b"bcdef".to_vec());
}

#[test_case]
fn tcp_numeros_de_sequencia_dao_a_volta() {
    let mut t = tcp::Tcp::new();
    let mut cap = Capture::new();
    let (index, _, _) = establish_active(&mut t, &mut cap, 0);
    let port = t.tcbs[index].local_port;
    // Recepção: o par está a 3 bytes de dar a volta.
    t.tcbs[index].rcv_nxt = 0xFFFF_FFFD;
    let next = t.tcbs[index].snd_nxt;
    inject(&mut t, &mut cap, 1, 80, port, 0xFFFF_FFFD, next, ACK, 4096, b"abcde");
    assert_eq!(t.tcbs[index].rcv.len(), 5);
    assert_eq!(t.tcbs[index].rcv_nxt, 2);
    assert_eq!(cap.last().2, 2);
    // Envio: dados que cruzam 2^32.
    cap.clear();
    t.tcbs[index].snd_una = 0xFFFF_FFFE;
    t.tcbs[index].snd_nxt = 0xFFFF_FFFE;
    assert_eq!(t.send(index, b"wxyz", 2, &mut cap), 4);
    assert_eq!(cap.last().1, 0xFFFF_FFFE);
    assert_eq!(t.tcbs[index].snd_nxt, 2);
    inject(&mut t, &mut cap, 3, 80, port, 2, 2, ACK, 4096, &[]);
    assert_eq!(t.tcbs[index].in_flight, 0);
    assert!(tcp::seq_lt(0xFFFF_FFF0, 5));
    assert!(!tcp::seq_lt(5, 0xFFFF_FFF0));
}

#[test_case]
fn tcp_oito_blocos_e_time_wait_mais_antigo_e_tomado() {
    let mut t = tcp::Tcp::new();
    let mut cap = Capture::new();
    for i in 0..8u16 {
        assert!(t.connect(ME, HOST, 100 + i, 0, &mut cap).is_ok());
    }
    assert_eq!(t.in_use(), 8);
    assert_eq!(t.connect(ME, HOST, 200, 0, &mut cap), Err(tcp::Error::NoBufs));
    assert_eq!(t.listen(9000), Err(tcp::Error::NoBufs));
    // Põe dois blocos em TIME_WAIT (o 5 e depois o 2): o mais antigo (menor
    // `born`) é o do índice 2.
    for index in [5usize, 2] {
        let tcb = &mut t.tcbs[index];
        tcb.state = State::TimeWait;
        tcb.tw_deadline = 10_000;
    }
    let before_port = t.tcbs[2].remote_port;
    let new = t.connect(ME, HOST, 300, 1, &mut cap).expect("toma o TIME_WAIT mais antigo");
    assert_eq!(new, 2);
    assert_ne!(t.tcbs[2].remote_port, before_port);
    assert_eq!(t.tcbs[5].state, State::TimeWait);
}

#[test_case]
fn tcp_conexao_aceita_sobrevive_ao_fechamento_do_listen() {
    let mut t = tcp::Tcp::new();
    let mut cap = Capture::new();
    let listener = t.listen(8000).unwrap();
    inject(&mut t, &mut cap, 0, 40000, 8000, 700, 0, SYN, 4096, &[]);
    let (_, iss, _, _, _) = cap.last();
    inject(&mut t, &mut cap, 1, 40000, 8000, 701, iss + 1, ACK, 4096, &[]);
    let child = t.accept(listener).unwrap();
    cap.clear();
    t.close(listener, 2, &mut cap);
    assert_eq!(cap.len(), 0, "nenhum RST: a conexao ja foi aceita");
    assert_eq!(t.tcbs[child].state, State::Established);
    // Uma conexão que ninguém aceitou, essa sim, é abortada com RST.
    let listener = t.listen(8001).unwrap();
    inject(&mut t, &mut cap, 3, 40001, 8001, 10, 0, SYN, 4096, &[]);
    let (_, iss2, _, _, _) = cap.last();
    inject(&mut t, &mut cap, 4, 40001, 8001, 11, iss2 + 1, ACK, 4096, &[]);
    cap.clear();
    t.close(listener, 5, &mut cap);
    assert_eq!(cap.last().0 & RST, RST);
    assert_eq!(t.in_use(), 1, "so a conexao aceita resta");
}

// ---------------------------------------------------------------------------
// Loopback com a pilha inteira (dois sockets de um mesmo teste)
// ---------------------------------------------------------------------------

#[test_case]
fn loopback_dois_sockets_trocam_dados_nos_dois_sentidos() {
    fresh(|s| {
        let Stack { link, tcp } = s;
        link.nic.present = false; // sem placa: só o loopback
        let lo = [127, 0, 0, 1];
        let listener = tcp.listen(8000).unwrap();
        let client = tcp.connect(lo, lo, 8000, 0, link).unwrap();
        s.poll(0);
        assert_eq!(s.tcp.tcbs[client].state, State::Established);
        let server = s.tcp.accept(listener).expect("conexao na fila");
        // Cliente -> servidor.
        let Stack { link, tcp } = s;
        assert_eq!(tcp.send(client, b"ping", 0, link), 4);
        s.poll(0);
        let mut out = [0u8; 16];
        let Stack { link, tcp } = s;
        assert_eq!(tcp.recv(server, &mut out, 0, link), 4);
        assert_eq!(&out[..4], b"ping");
        // Servidor -> cliente.
        assert_eq!(tcp.send(server, b"pong!", 0, link), 5);
        s.poll(0);
        let Stack { link, tcp } = s;
        assert_eq!(tcp.recv(client, &mut out, 0, link), 5);
        assert_eq!(&out[..5], b"pong!");
        // Fechamento dos dois lados e fim do TIME_WAIT.
        tcp.close(client, 1, link);
        s.poll(1);
        let Stack { link, tcp } = s;
        tcp.close(server, 2, link);
        tcp.close(listener, 2, link);
        s.poll(2);
        s.poll(2 + 100);
        assert_eq!(s.tcp.in_use(), 0);
        assert_eq!(s.link.stats.tx, 0, "nada saiu pela placa");
    });
}

#[test_case]
fn loopback_connect_sem_servidor_e_recusado_na_hora() {
    fresh(|s| {
        s.link.nic.present = false;
        let Stack { link, tcp } = s;
        let lo = [127, 0, 0, 1];
        let client = tcp.connect(lo, lo, 9, 0, link).unwrap();
        s.poll(0);
        assert_eq!(s.tcp.tcbs[client].err, Some(NetErr::ConnRefused));
        assert_eq!(s.tcp.tcbs[client].state, State::Closed);
        let Stack { link, tcp } = s;
        tcp.close(client, 0, link);
        assert_eq!(s.tcp.in_use(), 0);
    });
}

// ---------------------------------------------------------------------------
// Robustez: quadros malformados e sequências aleatórias
// ---------------------------------------------------------------------------

#[test_case]
fn quadros_aleatorios_nunca_derrubam_a_pilha() {
    fresh(|s| {
        s.link.arp.learn(PEER_IP, PEER_MAC);
        s.tcp.listen(8000).unwrap();
        // Gerador congruencial com semente fixa: o teste é repetível.
        let mut estado: u32 = 0x1234_5678;
        let mut proximo = || {
            estado = estado.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (estado >> 16) as u8
        };
        for rodada in 0..300u64 {
            let tamanho = usize::from(proximo()) % 120;
            let mut quadro: Vec<u8> = (0..tamanho).map(|_| proximo()).collect();
            // Metade dos quadros ganha um cabeçalho Ethernet/IPv4 plausível, para
            // chegar às camadas de cima.
            if rodada % 2 == 0 && quadro.len() >= 40 {
                quadro[0..6].copy_from_slice(&LOCAL_MAC);
                quadro[12..14].copy_from_slice(&[0x08, 0x00]);
                quadro[14] = 0x45;
                quadro[23] = if rodada % 4 == 0 { 6 } else { 1 };
                let comprimento = (quadro.len() - 14) as u16;
                quadro[16..18].copy_from_slice(&comprimento.to_be_bytes());
                quadro[30..34].copy_from_slice(&LOCAL_IP);
                fix_ip_checksum(&mut quadro[14..]);
            }
            s.link.nic.inject(&quadro);
            s.poll(rodada);
            while take(s).is_some() {}
        }
        // A pilha continua de pé e a configuração intacta.
        assert_eq!(s.link.config.ip, LOCAL_IP);
        assert!(s.link.stats.dropped > 0);
    });
}
