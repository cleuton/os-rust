//! A pilha de rede do os-rust: Ethernet, ARP, IPv4, ICMP (eco) e TCP, mais um
//! loopback interno, tudo escrito aqui, sem crate de rede.
//!
//! # O caminho de um quadro
//!
//! ```text
//! placa (RTL8139)  ->  Ethernet  ->  ARP            (responde, aprende MACs)
//!                                \->  IPv4  ->  ICMP (eco)
//!                                          \->  TCP  ->  socket do programa
//! ```
//!
//! A resposta desce o mesmo caminho: o TCP entrega um segmento ao IP, o IP
//! escolhe o próximo salto (loopback, vizinho ou gateway), o ARP acha o MAC e a
//! placa transmite. Para endereços `127.x.x.x` o pacote nem chega à Ethernet:
//! volta para a própria pilha por uma fila (`LoopQueue`), e por isso dois
//! programas conversam por TCP mesmo **sem placa**.
//!
//! # Quando a pilha anda
//!
//! Por **polling**, como o ATA e o RTC: `net::poll()` recebe os quadros que a
//! placa já tem, esvazia o loopback e vence os temporizadores. Roda só em
//! pontos seguros, sempre com as interrupções desligadas, e **nunca** dentro de
//! um handler de IRQ (handlers em ring 0 não alocam nem pegam lock): na espera
//! ociosa do prompt, no escalonador (bloqueio, `yield`, tick vindo de ring 3,
//! volta do laço ocioso) e dentro das syscalls e comandos de rede.
//!
//! A pilha é genérica sobre a placa (`Nic`) e recebe o tempo (`now`, em ticks)
//! como argumento, para que os testes a exercitem com uma placa falsa e um
//! relógio injetado, sem hardware e sem esperar segundos de verdade.
//!
//! # Travas
//!
//! A ordem é sempre `SCHED` (escalonador) e depois `STACK` (esta pilha). Quem
//! segura `STACK` nunca chama o escalonador.

pub mod arp;
pub mod checksum;
pub mod eth;
pub mod icmp;
pub mod ip;
pub mod socket;
pub mod tcp;

use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;
use x86_64::VirtAddr;

use abi::{
    ERR_ADDRINUSE, ERR_BADF, ERR_CONNREFUSED, ERR_CONNRESET, ERR_MFILE, ERR_NOBUFS,
    ERR_NODEV, ERR_TIMEDOUT, ERR_TYPE,
};

use self::socket::{NetOp, PendingNet, SocketTable};
use self::tcp::{NetErr, State, Tcp, Tx};
use crate::rtl8139::{self, Card, CardState};
use crate::serial_println;
use crate::timer;

/// A placa, vista pela pilha.
pub trait Nic {
    /// A placa existe e está pronta?
    fn present(&self) -> bool;
    fn mac(&self) -> [u8; 6];
    /// Envia um quadro; `false` se foi descartado.
    fn transmit(&mut self, frame: &[u8]) -> bool;
    /// Copia o próximo quadro para `buf`. O segundo valor é quantos quadros a
    /// placa descartou por erro nesta chamada.
    fn receive(&mut self, buf: &mut [u8]) -> (Option<usize>, u32);
}

/// Endereço, máscara e gateway.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub ip: [u8; 4],
    pub mask: [u8; 4],
    pub gateway: [u8; 4],
}

/// Os valores da rede de usuário do QEMU.
pub const DEFAULT_CONFIG: Config =
    Config { ip: [10, 0, 2, 15], mask: [255, 255, 255, 0], gateway: [10, 0, 2, 2] };

/// Contadores de quadros, para o comando `rede`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub rx: u32,
    pub tx: u32,
    pub dropped: u32,
}

/// Interpreta `a.b.c.d` (quatro números de 0 a 255, sem espaços).
pub fn parse_ipv4(text: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut parts = text.split('.');
    for byte in out.iter_mut() {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *byte = part.parse::<u16>().ok().and_then(|n| u8::try_from(n).ok())?;
    }
    parts.next().is_none().then_some(out)
}

/// Confere uma configuração. Em caso de erro devolve qual dos três argumentos
/// (0 = ip, 1 = máscara, 2 = gateway) é o problema.
pub fn valid_config(ip: [u8; 4], mask: [u8; 4], gateway: [u8; 4]) -> Result<(), usize> {
    if ip == [0; 4] || ip == [255; 4] || ip[0] == 127 || ip[0] >= 224 {
        return Err(0);
    }
    let m = u32::from_be_bytes(mask);
    // Contígua: uns seguidos de zeros, com pelo menos uma sub-rede de 4 endereços.
    if m.leading_ones() + m.trailing_zeros() != 32 || m.leading_ones() == 0 || m.trailing_zeros() < 2 {
        return Err(1);
    }
    if !ip::in_subnet(gateway, ip, mask) || gateway == ip || gateway == [0; 4] {
        return Err(2);
    }
    Ok(())
}

/// Um pacote IP esperando o ARP descobrir o MAC do próximo salto.
struct PendingPacket {
    packet: [u8; ip::MTU],
    len: usize,
    next_hop: [u8; 4],
    tries: u8,
    deadline: u64,
}

/// Quantas vezes o ARP pergunta e de quanto em quanto tempo (ticks).
const ARP_TRIES: u8 = 3;
const ARP_INTERVAL: u64 = 100;
/// O identificador dos pedidos de eco do `ping`.
pub const PING_ID: u16 = 0x05AA;
/// Os dados de um pedido de eco.
pub const PING_DATA_LEN: usize = 32;

/// Tudo da pilha que não é TCP: a placa, o IP, o ARP e o loopback. Também é o
/// `Tx` do TCP (entrega cada segmento ao IP).
pub struct Link<N: Nic> {
    pub nic: N,
    pub config: Config,
    pub arp: arp::Cache,
    pub loopq: ip::LoopQueue,
    pub stats: Stats,
    /// O tick atual, atualizado a cada entrada na pilha.
    pub now: u64,
    pending: Option<PendingPacket>,
    ip_id: u16,
    /// Rascunho para montar o quadro de saída (Ethernet + IP + conteúdo).
    frame: [u8; eth::HEADER_LEN + ip::MTU],
    /// A última resposta de eco: identificador, sequência e tick de chegada.
    pub echo_reply: Option<(u16, u16, u64)>,
}

impl<N: Nic> Link<N> {
    pub const fn new(nic: N) -> Link<N> {
        Link {
            nic,
            config: DEFAULT_CONFIG,
            arp: arp::Cache::new(),
            loopq: ip::LoopQueue::new(),
            stats: Stats { rx: 0, tx: 0, dropped: 0 },
            now: 0,
            pending: None,
            ip_id: 0,
            frame: [0; eth::HEADER_LEN + ip::MTU],
            echo_reply: None,
        }
    }

    /// O endereço de origem de um pacote para `dst`: `127.0.0.1` no loopback,
    /// o IP configurado nos demais casos.
    pub fn source_for(&self, dst: [u8; 4]) -> [u8; 4] {
        if ip::is_loopback(dst) {
            [127, 0, 0, 1]
        } else {
            self.config.ip
        }
    }

    /// Pode alcançar `dst`? O loopback sempre; o resto só com a placa.
    pub fn reachable(&self, dst: [u8; 4]) -> bool {
        ip::is_loopback(dst) || self.nic.present()
    }

    /// Monta um pacote IP e o entrega ao roteamento. Quem manda é o chamador
    /// (`src`); aqui só se escolhe o caminho.
    pub fn send_ip(&mut self, src: [u8; 4], dst: [u8; 4], protocol: u8, payload: &[u8]) {
        let total = ip::HEADER_LEN + payload.len();
        if total > ip::MTU {
            return;
        }
        self.ip_id = self.ip_id.wrapping_add(1);
        let id = self.ip_id;
        let start = eth::HEADER_LEN;
        ip::write_header(&mut self.frame[start..], src, dst, protocol, id, payload.len());
        self.frame[start + ip::HEADER_LEN..start + total].copy_from_slice(payload);

        match ip::route(dst, self.config.ip, self.config.mask) {
            ip::Route::Loopback => {
                if !self.loopq.push(&self.frame[start..start + total]) {
                    self.stats.dropped += 1;
                }
            }
            route => {
                if !self.nic.present() {
                    return;
                }
                let broadcast = dst == [255; 4] || dst == ip::subnet_broadcast(self.config.ip, self.config.mask);
                let next_hop = if route == ip::Route::Direct { dst } else { self.config.gateway };
                let mac = if broadcast { Some(eth::BROADCAST) } else { self.arp.lookup(next_hop) };
                match mac {
                    Some(mac) => {
                        let local = self.nic.mac();
                        eth::write_header(&mut self.frame, mac, local, eth::ETHERTYPE_IPV4);
                        self.transmit_frame(eth::HEADER_LEN + total);
                    }
                    None => self.queue_for_arp(start, total, next_hop),
                }
            }
        }
    }

    fn transmit_frame(&mut self, len: usize) {
        if self.nic.transmit(&self.frame[..len]) {
            self.stats.tx += 1;
        } else {
            self.stats.dropped += 1;
        }
    }

    /// Guarda o pacote (só um por vez) e pergunta quem tem o IP do próximo salto.
    fn queue_for_arp(&mut self, start: usize, total: usize, next_hop: [u8; 4]) {
        if self.pending.is_some() {
            // O TCP retransmite o que for descartado aqui.
            self.stats.dropped += 1;
            return;
        }
        let mut packet = [0u8; ip::MTU];
        packet[..total].copy_from_slice(&self.frame[start..start + total]);
        self.pending = Some(PendingPacket {
            packet,
            len: total,
            next_hop,
            tries: 1,
            deadline: self.now + ARP_INTERVAL,
        });
        self.send_arp_request(next_hop);
    }

    fn send_arp_request(&mut self, target: [u8; 4]) {
        let mac = self.nic.mac();
        let packet = arp::Packet {
            op: arp::OP_REQUEST,
            sender_mac: mac,
            sender_ip: self.config.ip,
            target_mac: [0; 6],
            target_ip: target,
        };
        self.send_arp(&packet, eth::BROADCAST);
    }

    fn send_arp(&mut self, packet: &arp::Packet, dst_mac: [u8; 6]) {
        let mut out = [0u8; eth::HEADER_LEN + arp::PACKET_LEN];
        eth::write_header(&mut out, dst_mac, self.nic.mac(), eth::ETHERTYPE_ARP);
        arp::write(packet, &mut out[eth::HEADER_LEN..]);
        if self.nic.transmit(&out) {
            self.stats.tx += 1;
        } else {
            self.stats.dropped += 1;
        }
    }

    /// Repete o pedido de ARP do pacote pendente; depois de 3, desiste.
    fn arp_tick(&mut self) {
        let now = self.now;
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        if now < pending.deadline {
            return;
        }
        if pending.tries >= ARP_TRIES {
            self.pending = None;
            self.stats.dropped += 1;
            return;
        }
        pending.tries += 1;
        pending.deadline = now + ARP_INTERVAL;
        let target = pending.next_hop;
        self.send_arp_request(target);
    }

    /// Depois de aprender um MAC, envia o pacote que o esperava.
    fn flush_pending(&mut self) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        let Some(mac) = self.arp.lookup(pending.next_hop) else {
            return;
        };
        let Some(pending) = self.pending.take() else {
            return;
        };
        let start = eth::HEADER_LEN;
        let local = self.nic.mac();
        eth::write_header(&mut self.frame, mac, local, eth::ETHERTYPE_IPV4);
        self.frame[start..start + pending.len].copy_from_slice(&pending.packet[..pending.len]);
        self.transmit_frame(start + pending.len);
    }
}

impl<N: Nic> Tx for Link<N> {
    fn send(&mut self, src: [u8; 4], dst: [u8; 4], segment: &[u8]) {
        self.send_ip(src, dst, ip::PROTO_TCP, segment);
    }
}

/// A pilha inteira.
pub struct Stack<N: Nic> {
    pub link: Link<N>,
    pub tcp: Tcp,
}

impl<N: Nic> Stack<N> {
    pub const fn new(nic: N) -> Stack<N> {
        Stack { link: Link::new(nic), tcp: Tcp::new() }
    }

    /// Volta ao estado de boot, menos a placa (os testes recomeçam daqui).
    pub fn reset(&mut self) {
        self.link.config = DEFAULT_CONFIG;
        self.link.arp.clear();
        self.link.loopq.clear();
        self.link.stats = Stats::default();
        self.link.pending = None;
        self.link.echo_reply = None;
        self.tcp.reset();
    }

    /// Anda a pilha: recebe quadros, esvazia o loopback, vence os temporizadores.
    /// Repete enquanto houver trabalho (uma resposta pelo loopback pode gerar
    /// outro pacote), com um limite de voltas.
    pub fn poll(&mut self, now: u64) {
        self.link.now = now;
        let mut buf = [0u8; eth::HEADER_LEN + ip::MTU];
        for _ in 0..8 {
            let mut activity = false;
            for _ in 0..16 {
                if !self.link.nic.present() {
                    break;
                }
                let (frame, dropped) = self.link.nic.receive(&mut buf);
                self.link.stats.dropped += dropped;
                match frame {
                    Some(len) => {
                        activity = true;
                        self.link.stats.rx += 1;
                        self.handle_frame(&buf[..len]);
                    }
                    None => break,
                }
            }
            for _ in 0..2 * ip::LOOP_SLOTS {
                match self.link.loopq.pop(&mut buf) {
                    Some(len) => {
                        activity = true;
                        self.handle_ip(&buf[..len]);
                    }
                    None => break,
                }
            }
            self.link.arp_tick();
            self.tcp.on_tick(now, &mut self.link);
            if !activity && self.link.loopq.is_empty() {
                break;
            }
        }
    }

    fn handle_frame(&mut self, frame: &[u8]) {
        let Some(parsed) = eth::parse(frame) else {
            self.link.stats.dropped += 1;
            return;
        };
        if parsed.dst != self.link.nic.mac() && parsed.dst != eth::BROADCAST {
            self.link.stats.dropped += 1;
            return;
        }
        match parsed.ethertype {
            eth::ETHERTYPE_ARP => self.handle_arp(parsed.payload),
            eth::ETHERTYPE_IPV4 => self.handle_ip(parsed.payload),
            _ => self.link.stats.dropped += 1,
        }
    }

    fn handle_arp(&mut self, bytes: &[u8]) {
        let Some(packet) = arp::parse(bytes) else {
            self.link.stats.dropped += 1;
            return;
        };
        // Só interessa o que fala conosco: aprende o remetente de pedidos e de
        // respostas dirigidos ao nosso IP.
        if packet.target_ip != self.link.config.ip {
            return;
        }
        self.link.arp.learn(packet.sender_ip, packet.sender_mac);
        if packet.op == arp::OP_REQUEST {
            let reply = arp::Packet {
                op: arp::OP_REPLY,
                sender_mac: self.link.nic.mac(),
                sender_ip: self.link.config.ip,
                target_mac: packet.sender_mac,
                target_ip: packet.sender_ip,
            };
            self.link.send_arp(&reply, packet.sender_mac);
        }
        self.link.flush_pending();
    }

    fn handle_ip(&mut self, bytes: &[u8]) {
        let packet = match ip::parse(bytes) {
            Ok(packet) => packet,
            Err(_) => {
                self.link.stats.dropped += 1;
                return;
            }
        };
        let config = self.link.config;
        let for_us = packet.dst == config.ip
            || ip::is_loopback(packet.dst)
            || packet.dst == ip::subnet_broadcast(config.ip, config.mask)
            || packet.dst == [255; 4];
        if !for_us {
            self.link.stats.dropped += 1;
            return;
        }
        match packet.protocol {
            ip::PROTO_ICMP => self.handle_icmp(packet.src, packet.dst, packet.payload),
            ip::PROTO_TCP => {
                let now = self.link.now;
                self.tcp.on_segment(packet.src, packet.dst, packet.payload, now, &mut self.link)
            }
            _ => self.link.stats.dropped += 1,
        }
    }

    fn handle_icmp(&mut self, src: [u8; 4], dst: [u8; 4], bytes: &[u8]) {
        let Some(echo) = icmp::parse(bytes) else {
            self.link.stats.dropped += 1;
            return;
        };
        match echo.kind {
            icmp::ECHO_REQUEST => {
                // Responde com os mesmos dados, vindo do endereço que foi chamado.
                let mut out = [0u8; icmp::HEADER_LEN + ip::MTU - ip::HEADER_LEN - icmp::HEADER_LEN];
                if echo.data.len() > out.len() - icmp::HEADER_LEN {
                    self.link.stats.dropped += 1;
                    return;
                }
                let len = icmp::write_echo(&mut out, icmp::ECHO_REPLY, echo.id, echo.seq, echo.data);
                let from = if ip::is_loopback(dst) || dst == self.link.config.ip {
                    dst
                } else {
                    self.link.config.ip
                };
                self.link.send_ip(from, src, ip::PROTO_ICMP, &out[..len]);
            }
            icmp::ECHO_REPLY => {
                self.link.echo_reply = Some((echo.id, echo.seq, self.link.now));
            }
            _ => {}
        }
    }

    /// Envia um pedido de eco de 32 bytes. `false` se o destino não é
    /// alcançável (sem placa e fora do loopback).
    pub fn send_echo(&mut self, dst: [u8; 4], id: u16, seq: u16, now: u64) -> bool {
        self.link.now = now;
        if !self.link.reachable(dst) {
            return false;
        }
        let mut data = [0u8; PING_DATA_LEN];
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = b'a' + (i % 23) as u8;
        }
        let mut out = [0u8; icmp::HEADER_LEN + PING_DATA_LEN];
        let len = icmp::write_echo(&mut out, icmp::ECHO_REQUEST, id, seq, &data);
        self.link.echo_reply = None;
        let src = self.link.source_for(dst);
        self.link.send_ip(src, dst, ip::PROTO_ICMP, &out[..len]);
        true
    }
}

// ---------------------------------------------------------------------------
// A placa do kernel e a pilha global
// ---------------------------------------------------------------------------

/// A RTL8139 do kernel (ou a falta dela). `enabled` existe para os testes
/// desligarem a placa na pilha e provarem que o loopback continua funcionando.
pub struct KernelNic {
    state: CardState,
    enabled: bool,
}

impl KernelNic {
    pub const fn new() -> KernelNic {
        KernelNic { state: CardState::Absent, enabled: true }
    }

    fn card(&mut self) -> Option<&mut Card> {
        match (&mut self.state, self.enabled) {
            (CardState::Ready(card), true) => Some(card),
            _ => None,
        }
    }
}

impl Default for KernelNic {
    fn default() -> Self {
        KernelNic::new()
    }
}

impl Nic for KernelNic {
    fn present(&self) -> bool {
        self.enabled && matches!(self.state, CardState::Ready(_))
    }

    fn mac(&self) -> [u8; 6] {
        match &self.state {
            CardState::Ready(card) => card.mac(),
            _ => [0; 6],
        }
    }

    fn transmit(&mut self, frame: &[u8]) -> bool {
        self.card().is_some_and(|card| card.transmit(frame))
    }

    fn receive(&mut self, buf: &mut [u8]) -> (Option<usize>, u32) {
        match self.card() {
            Some(card) => card.receive(buf),
            None => (None, 0),
        }
    }
}

static STACK: Mutex<Stack<KernelNic>> = Mutex::new(Stack::new(KernelNic::new()));

/// Roda `f` com a pilha do kernel e o tick atual, com as interrupções
/// desligadas (o tick é lido e a pilha usada sem que nada as interrompa).
fn with_stack<R>(f: impl FnOnce(&mut Stack<KernelNic>, u64) -> R) -> R {
    without_interrupts(|| {
        let mut stack = STACK.lock();
        f(&mut stack, timer::ticks())
    })
}

/// Acha e inicializa a placa. Chamada uma vez, por `lib::init`, depois de
/// `memory::init`.
pub fn init(physical_memory_offset: u64) {
    let state = rtl8139::init(VirtAddr::new(physical_memory_offset));
    match &state {
        CardState::Absent => serial_println!("[boot] rede: sem placa RTL8139"),
        CardState::Unavailable(reason) => serial_println!("[boot] rede: placa indisponivel ({})", reason),
        CardState::Ready(card) => {
            let m = card.mac();
            serial_println!(
                "[boot] rede: RTL8139 mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                m[0], m[1], m[2], m[3], m[4], m[5]
            );
        }
    }
    with_stack(|stack, _| stack.link.nic.state = state);
}

/// Anda a pilha. Segura chamar em qualquer ponto seguro do kernel, com as
/// interrupções ligadas ou não.
pub fn poll() {
    with_stack(|stack, now| stack.poll(now));
}

/// O que `rede` mostra na primeira linha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardStatus {
    Absent,
    Unavailable(&'static str),
    Ready([u8; 6]),
}

pub fn card_status() -> CardStatus {
    with_stack(|stack, _| {
        if !stack.link.nic.enabled {
            return CardStatus::Absent;
        }
        match &stack.link.nic.state {
            CardState::Absent => CardStatus::Absent,
            CardState::Unavailable(reason) => CardStatus::Unavailable(reason),
            CardState::Ready(card) => CardStatus::Ready(card.mac()),
        }
    })
}

/// Liga ou desliga a placa **na pilha** (a placa continua lá). Existe só para os
/// testes provarem o comportamento sem placa; sempre religar no fim.
#[doc(hidden)]
pub fn set_card_enabled(enabled: bool) {
    with_stack(|stack, _| stack.link.nic.enabled = enabled);
}

pub fn config() -> Config {
    with_stack(|stack, _| stack.link.config)
}

/// Troca a configuração (só em memória). Esvazia o ARP. Em caso de erro devolve
/// qual argumento é inválido e nada muda.
pub fn configure(ip: [u8; 4], mask: [u8; 4], gateway: [u8; 4]) -> Result<(), usize> {
    valid_config(ip, mask, gateway)?;
    with_stack(|stack, _| {
        stack.link.config = Config { ip, mask, gateway };
        stack.link.arp.clear();
        stack.link.pending = None;
    });
    Ok(())
}

/// Volta à configuração padrão (os testes usam).
#[doc(hidden)]
pub fn reset_config() {
    with_stack(|stack, _| {
        stack.link.config = DEFAULT_CONFIG;
        stack.link.arp.clear();
        stack.link.pending = None;
    });
}

pub fn stats() -> Stats {
    with_stack(|stack, _| stack.link.stats)
}

/// Quantos blocos de conexão TCP estão em uso.
pub fn tcp_in_use() -> usize {
    with_stack(|stack, _| stack.tcp.in_use())
}

/// Dá para alcançar `dst`? O loopback sempre; o resto só com a placa.
pub fn can_reach(dst: [u8; 4]) -> bool {
    with_stack(|stack, _| stack.link.reachable(dst))
}

/// Envia um pedido de eco do `ping`. `Err` se não há como alcançar o destino.
pub fn ping_send(dst: [u8; 4], seq: u16) -> Result<(), ()> {
    with_stack(|stack, now| stack.send_echo(dst, PING_ID, seq, now).then_some(()).ok_or(()))
}

/// O tick em que chegou a resposta do pedido `seq`, se já chegou.
pub fn ping_reply(seq: u16) -> Option<u64> {
    with_stack(|stack, _| match stack.link.echo_reply {
        Some((id, s, tick)) if id == PING_ID && s == seq => Some(tick),
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// Sockets: o que as syscalls usam
// ---------------------------------------------------------------------------

/// O resultado de uma syscall de rede: pronto, ou a tarefa precisa esperar.
pub enum Outcome {
    Done(i64),
    Block(PendingNet),
}

fn net_err_code(err: NetErr) -> i64 {
    match err {
        NetErr::TimedOut => ERR_TIMEDOUT,
        NetErr::ConnReset => ERR_CONNRESET,
        NetErr::ConnRefused => ERR_CONNREFUSED,
    }
}

/// Milissegundos em ticks, arredondando para cima (resolução de 10 ms).
fn ms_to_ticks(ms: u64) -> u64 {
    ms.div_ceil(10)
}

/// Fecha o bloco de conexão (a rotina de `SYS_TCP_CLOSE`).
pub(crate) fn close_conn(conn: usize) {
    with_stack(|stack, now| {
        let Stack { link, tcp } = stack;
        link.now = now;
        tcp.close(conn, now, link);
    });
}

/// `SYS_TCP_LISTEN`, depois das verificações de argumentos.
pub fn sys_listen(table: &mut SocketTable, port: u16) -> i64 {
    if table.is_full() {
        return ERR_MFILE;
    }
    let result = with_stack(|stack, _| stack.tcp.listen(port));
    match result {
        Ok(conn) => table.insert(conn).map_or(ERR_MFILE, |s| s as i64),
        Err(tcp::Error::AddrInUse) => ERR_ADDRINUSE,
        Err(tcp::Error::NoBufs) => ERR_NOBUFS,
    }
}

/// Um socket existente e o tipo dele: `Err` com o erro da syscall.
fn lookup(table: &SocketTable, sock: u64, listener: bool) -> Result<usize, i64> {
    let socket = usize::try_from(sock).ok().and_then(|i| table.get(i)).ok_or(ERR_BADF)?;
    let is_listener = with_stack(|stack, _| stack.tcp.tcbs[socket.conn].state == State::Listen);
    if is_listener != listener {
        return Err(ERR_TYPE);
    }
    Ok(socket.conn)
}

fn timeout_ticks(table: &SocketTable, sock: usize) -> u64 {
    table.get(sock).map_or(0, |s| ms_to_ticks(s.timeout_ms))
}

/// `SYS_TCP_ACCEPT`.
pub fn sys_accept(table: &mut SocketTable, sock: u64, now: u64) -> Outcome {
    if let Err(code) = lookup(table, sock, true) {
        return Outcome::Done(code);
    }
    if table.is_full() {
        return Outcome::Done(ERR_MFILE);
    }
    let sock = sock as usize;
    let pending = PendingNet {
        op: NetOp::Accept,
        sock,
        ptr: 0,
        len: 0,
        deadline: now + timeout_ticks(table, sock),
    };
    poll();
    settle(table, pending, now)
}

/// `SYS_TCP_CONNECT`.
pub fn sys_connect(table: &mut SocketTable, ip: [u8; 4], port: u16, now: u64) -> Outcome {
    if table.is_full() {
        return Outcome::Done(ERR_MFILE);
    }
    let started = with_stack(|stack, now| {
        let Stack { link, tcp } = stack;
        link.now = now;
        if !tcp.tcbs.iter().any(|t| !t.used || t.state == State::TimeWait) {
            return Err(ERR_NOBUFS);
        }
        if !link.reachable(ip) {
            return Err(ERR_NODEV);
        }
        let local = link.source_for(ip);
        tcp.connect(local, ip, port, now, link).map_err(|_| ERR_NOBUFS)
    });
    let conn = match started {
        Ok(conn) => conn,
        Err(code) => return Outcome::Done(code),
    };
    let Some(sock) = table.insert(conn) else {
        close_conn(conn);
        return Outcome::Done(ERR_MFILE);
    };
    let pending = PendingNet { op: NetOp::Connect, sock, ptr: 0, len: 0, deadline: u64::MAX };
    poll();
    settle(table, pending, now)
}

/// `SYS_TCP_SEND`, com `len > 0` e o intervalo do usuário já validado.
pub fn sys_send(table: &mut SocketTable, sock: u64, ptr: u64, len: u64, now: u64) -> Outcome {
    if let Err(code) = lookup(table, sock, false) {
        return Outcome::Done(code);
    }
    if len == 0 {
        return Outcome::Done(0);
    }
    let sock = sock as usize;
    let pending = PendingNet {
        op: NetOp::Send,
        sock,
        ptr,
        len,
        deadline: now + timeout_ticks(table, sock),
    };
    poll();
    settle(table, pending, now)
}

/// `SYS_TCP_RECV`, com `len > 0` e o intervalo do usuário já validado.
pub fn sys_recv(table: &mut SocketTable, sock: u64, ptr: u64, len: u64, now: u64) -> Outcome {
    if let Err(code) = lookup(table, sock, false) {
        return Outcome::Done(code);
    }
    if len == 0 {
        return Outcome::Done(0);
    }
    let sock = sock as usize;
    let pending = PendingNet {
        op: NetOp::Recv,
        sock,
        ptr,
        len,
        deadline: now + timeout_ticks(table, sock),
    };
    poll();
    settle(table, pending, now)
}

/// `SYS_TCP_CLOSE`.
pub fn sys_close(table: &mut SocketTable, sock: u64) -> i64 {
    let Some(socket) = usize::try_from(sock).ok().and_then(|i| table.take(i)) else {
        return ERR_BADF;
    };
    close_conn(socket.conn);
    0
}

/// `SYS_TCP_TIMEOUT`.
pub fn sys_timeout(table: &mut SocketTable, sock: u64, ms: u64) -> i64 {
    match usize::try_from(sock).ok().and_then(|i| table.get_mut(i)) {
        Some(socket) => {
            socket.timeout_ms = ms;
            0
        }
        None => ERR_BADF,
    }
}

/// Tenta atender o pedido já; se não der, a tarefa espera.
fn settle(table: &mut SocketTable, pending: PendingNet, now: u64) -> Outcome {
    match attempt(table, &pending, now) {
        Some(result) => Outcome::Done(result),
        None => Outcome::Block(pending),
    }
}

/// O pedido pode andar agora (ou o prazo venceu)? Não mexe em nada: o
/// escalonador pergunta isto com o espaço de endereçamento de outra tarefa
/// ativo.
pub fn ready(table: &SocketTable, pending: &PendingNet, now: u64) -> bool {
    let Some(socket) = table.get(pending.sock) else {
        return true;
    };
    let expired = now >= pending.deadline;
    with_stack(|stack, _| {
        let t = &stack.tcp.tcbs[socket.conn];
        match pending.op {
            NetOp::Accept => t.accept[0].is_some() || t.state != State::Listen || expired,
            NetOp::Connect => t.state != State::SynSent,
            NetOp::Send => {
                t.err.is_some()
                    || !matches!(t.state, State::Established | State::CloseWait)
                    || t.snd.free() > 0
                    || expired
            }
            NetOp::Recv => {
                !t.rcv.is_empty() || t.err.is_some() || t.peer_fin || t.state == State::Closed || expired
            }
        }
    })
}

/// Executa o pedido se ele pode andar: devolve o resultado da syscall. `None`
/// se ainda precisa esperar. Chamada com o espaço de endereçamento do dono
/// ativo (os ponteiros do pedido já foram validados).
pub fn attempt(table: &mut SocketTable, pending: &PendingNet, now: u64) -> Option<i64> {
    let Some(socket) = table.get(pending.sock).copied() else {
        return Some(ERR_BADF);
    };
    let conn = socket.conn;
    let expired = now >= pending.deadline;
    match pending.op {
        NetOp::Accept => {
            let child = with_stack(|stack, _| {
                let t = &stack.tcp.tcbs[conn];
                if t.state != State::Listen {
                    return Err(ERR_BADF);
                }
                Ok(stack.tcp.accept(conn))
            });
            match child {
                Err(code) => Some(code),
                Ok(Some(child)) => match table.insert(child) {
                    Some(slot) => Some(slot as i64),
                    None => {
                        close_conn(child);
                        Some(ERR_MFILE)
                    }
                },
                Ok(None) => expired.then_some(ERR_TIMEDOUT),
            }
        }
        NetOp::Connect => {
            let state = with_stack(|stack, _| {
                let t = &stack.tcp.tcbs[conn];
                (t.state, t.err)
            });
            match state {
                (State::SynSent, _) => None,
                (State::Closed, err) => {
                    // A conexão não saiu: libera o socket e devolve o motivo.
                    if let Some(socket) = table.take(pending.sock) {
                        close_conn(socket.conn);
                    }
                    Some(err.map_or(ERR_CONNRESET, net_err_code))
                }
                _ => Some(pending.sock as i64),
            }
        }
        NetOp::Send => with_stack(|stack, now| {
            let Stack { link, tcp } = stack;
            link.now = now;
            let t = &tcp.tcbs[conn];
            if let Some(err) = t.err {
                return Some(net_err_code(err));
            }
            if !matches!(t.state, State::Established | State::CloseWait) {
                return Some(ERR_CONNRESET);
            }
            let n = (pending.len as usize).min(t.snd.free());
            if n == 0 {
                return expired.then_some(ERR_TIMEDOUT);
            }
            // SAFETY: a syscall validou `ptr..ptr + len` (região do usuário,
            // mapeada e acessível), `n <= len`, e o espaço de endereçamento da
            // tarefa dona está ativo; ninguém mais escreve ali durante a
            // syscall (uma CPU, interrupções desligadas).
            let data = unsafe { core::slice::from_raw_parts(pending.ptr as *const u8, n) };
            Some(tcp.send(conn, data, now, link) as i64)
        }),
        NetOp::Recv => with_stack(|stack, now| {
            let Stack { link, tcp } = stack;
            link.now = now;
            let t = &tcp.tcbs[conn];
            if !t.rcv.is_empty() {
                let n = (pending.len as usize).min(t.rcv.len());
                // SAFETY: como em `Send`, mas o kernel escreve: a syscall exigiu
                // o intervalo gravável.
                let out = unsafe { core::slice::from_raw_parts_mut(pending.ptr as *mut u8, n) };
                return Some(tcp.recv(conn, out, now, link) as i64);
            }
            if let Some(err) = t.err {
                return Some(net_err_code(err));
            }
            if t.peer_fin || t.state == State::Closed {
                return Some(0);
            }
            expired.then_some(ERR_TIMEDOUT)
        }),
    }
}

/// Uma placa falsa para os testes: guarda os quadros que a pilha envia e
/// entrega os que o teste injeta. Sem hardware e sem relógio.
#[doc(hidden)]
pub mod fake {
    use super::Nic;

    const SLOTS: usize = 8;
    const FRAME: usize = 1514;

    pub struct FakeNic {
        pub mac: [u8; 6],
        pub present: bool,
        sent: [[u8; FRAME]; SLOTS],
        sent_len: [usize; SLOTS],
        sent_head: usize,
        sent_count: usize,
        rx: [[u8; FRAME]; SLOTS],
        rx_len: [usize; SLOTS],
        rx_head: usize,
        rx_count: usize,
    }

    impl FakeNic {
        pub const fn new(mac: [u8; 6]) -> FakeNic {
            FakeNic {
                mac,
                present: true,
                sent: [[0; FRAME]; SLOTS],
                sent_len: [0; SLOTS],
                sent_head: 0,
                sent_count: 0,
                rx: [[0; FRAME]; SLOTS],
                rx_len: [0; SLOTS],
                rx_head: 0,
                rx_count: 0,
            }
        }

        /// Um quadro que a "placa" recebeu da rede.
        pub fn inject(&mut self, frame: &[u8]) {
            assert!(self.rx_count < SLOTS && frame.len() <= FRAME);
            let slot = (self.rx_head + self.rx_count) % SLOTS;
            self.rx[slot][..frame.len()].copy_from_slice(frame);
            self.rx_len[slot] = frame.len();
            self.rx_count += 1;
        }

        /// O quadro mais antigo que a pilha enviou, ainda não retirado.
        pub fn take_sent(&mut self, out: &mut [u8]) -> Option<usize> {
            if self.sent_count == 0 {
                return None;
            }
            let len = self.sent_len[self.sent_head];
            out[..len].copy_from_slice(&self.sent[self.sent_head][..len]);
            self.sent_head = (self.sent_head + 1) % SLOTS;
            self.sent_count -= 1;
            Some(len)
        }

        pub fn sent_count(&self) -> usize {
            self.sent_count
        }

        pub fn clear(&mut self) {
            self.sent_head = 0;
            self.sent_count = 0;
            self.rx_head = 0;
            self.rx_count = 0;
            self.present = true;
        }
    }

    impl Nic for FakeNic {
        fn present(&self) -> bool {
            self.present
        }

        fn mac(&self) -> [u8; 6] {
            self.mac
        }

        fn transmit(&mut self, frame: &[u8]) -> bool {
            if self.sent_count == SLOTS {
                return false;
            }
            let slot = (self.sent_head + self.sent_count) % SLOTS;
            self.sent[slot][..frame.len()].copy_from_slice(frame);
            self.sent_len[slot] = frame.len();
            self.sent_count += 1;
            true
        }

        fn receive(&mut self, buf: &mut [u8]) -> (Option<usize>, u32) {
            if self.rx_count == 0 {
                return (None, 0);
            }
            let len = self.rx_len[self.rx_head];
            buf[..len].copy_from_slice(&self.rx[self.rx_head][..len]);
            self.rx_head = (self.rx_head + 1) % SLOTS;
            self.rx_count -= 1;
            (Some(len), 0)
        }
    }
}
