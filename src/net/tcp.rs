//! TCP, simplificado para caber numa aula e continuar correto.
//!
//! O TCP entrega uma sequência de bytes, em ordem e sem perdas, por cima de um
//! IP que perde, duplica e reordena pacotes. Cada byte tem um **número de
//! sequência**; quem recebe confirma ("ACK") até onde recebeu; quem envia
//! guarda o que ainda não foi confirmado e reenvia se a confirmação demora.
//!
//! O que esta versão faz, e o que deixa de fazer de propósito:
//!
//! - **Para-e-espera**: no máximo **um segmento** sem confirmação por conexão
//!   (até 1024 bytes). Quando ele é confirmado, sai o próximo. É lento e
//!   simples de provar.
//! - **Só em ordem**: um segmento fora de ordem ou repetido é descartado e o
//!   último byte em ordem é confirmado de novo; o par reenvia.
//! - **Retransmissão** depois de 100 ticks (1 s), no máximo 5 vezes; depois a
//!   conexão cai (`TimedOut`).
//! - **Janela**: o que anunciamos é o espaço livre do buffer de recepção
//!   (2048 bytes); respeitamos a janela do par e, com janela zero, sondamos
//!   com 1 byte a cada segundo.
//! - Os estados da norma (`LISTEN`, `SYN_SENT`, ..., `TIME_WAIT`), menos a
//!   abertura simultânea. `TIME_WAIT` dura 1 s em vez de minutos.
//! - Nada de opções, congestionamento, nem remontagem fora de ordem.
//!
//! Este módulo não conhece a rede nem o relógio: recebe segmentos e o tick
//! atual, e entrega o que quer enviar a um `Tx`. Por isso os testes o exercitam
//! sem hardware, injetando segmentos e ticks.
//!
//! Tudo mora em memória estática: 8 blocos de conexão (`Tcb`), cada um com dois
//! buffers circulares de 2048 bytes.

use super::checksum;
use abi::MAX_TCP_CONNECTIONS;

/// Maior quantidade de dados num segmento.
pub const MSS: usize = 1024;
/// Tamanho de cada buffer (envio e recepção) de uma conexão.
pub const BUF: usize = 2048;
/// Depois de quantos ticks sem confirmação o segmento é reenviado (1 s).
pub const RTX_TICKS: u64 = 100;
/// Quantas retransmissões antes de a conexão cair.
pub const MAX_RTX: u8 = 5;
/// Quantos SYN o `connect` envia no total (um por segundo).
pub const SYN_TRIES: u8 = 3;
/// Duração do `TIME_WAIT`, em ticks (1 s).
pub const TIME_WAIT_TICKS: u64 = 100;
/// Conexões já estabelecidas, ainda não aceitas, que um `listen` guarda.
pub const ACCEPT_QUEUE: usize = 2;
/// Cabeçalho TCP sem opções.
pub const HEADER_LEN: usize = 20;
/// Faixa das portas locais escolhidas pelo `connect`.
const EPHEMERAL_FIRST: u16 = 49152;

pub const FIN: u8 = 0x01;
pub const SYN: u8 = 0x02;
pub const RST: u8 = 0x04;
pub const PSH: u8 = 0x08;
pub const ACK: u8 = 0x10;

/// Quem recebe os segmentos que o TCP quer enviar (a camada IP, ou um
/// dispositivo falso nos testes). `segment` é o cabeçalho TCP mais os dados,
/// com a soma de verificação pronta.
pub trait Tx {
    fn send(&mut self, src: [u8; 4], dst: [u8; 4], segment: &[u8]);
}

/// Os estados de uma conexão (RFC 793).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

/// Por que uma conexão caiu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetErr {
    TimedOut,
    ConnReset,
    ConnRefused,
}

/// Erros de `listen` e `connect`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    AddrInUse,
    NoBufs,
}

/// `a < b` na aritmética de números de sequência (com envolvimento): a
/// diferença, vista como inteiro com sinal, é negativa.
pub fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

pub fn seq_le(a: u32, b: u32) -> bool {
    a == b || seq_lt(a, b)
}

/// Buffer circular de bytes.
pub struct Ring {
    buf: [u8; BUF],
    head: usize,
    len: usize,
}

impl Ring {
    pub const fn new() -> Ring {
        Ring { buf: [0; BUF], head: 0, len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn free(&self) -> usize {
        BUF - self.len
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// Acrescenta o que couber; devolve quantos bytes entraram.
    pub fn push(&mut self, data: &[u8]) -> usize {
        let n = data.len().min(self.free());
        for (i, &byte) in data[..n].iter().enumerate() {
            self.buf[(self.head + self.len + i) % BUF] = byte;
        }
        self.len += n;
        n
    }

    /// Copia para `out` os primeiros bytes a partir de `offset`, sem retirá-los.
    pub fn copy_out(&self, offset: usize, out: &mut [u8]) {
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = self.buf[(self.head + offset + i) % BUF];
        }
    }

    /// Descarta os `n` primeiros bytes.
    pub fn consume(&mut self, n: usize) {
        let n = n.min(self.len);
        self.head = (self.head + n) % BUF;
        self.len -= n;
    }

    /// Retira até `out.len()` bytes para `out`; devolve quantos.
    pub fn pop(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len);
        self.copy_out(0, &mut out[..n]);
        self.consume(n);
        n
    }
}

impl Default for Ring {
    fn default() -> Self {
        Ring::new()
    }
}

/// O bloco de controle de uma conexão (ou de um `listen`).
pub struct Tcb {
    /// Em uso (mesmo que o estado seja `Closed`: uma conexão que caiu continua
    /// do dono até ele chamar `close`).
    pub used: bool,
    pub state: State,
    pub local_ip: [u8; 4],
    pub local_port: u16,
    pub remote_ip: [u8; 4],
    pub remote_port: u16,
    pub iss: u32,
    pub snd_una: u32,
    pub snd_nxt: u32,
    pub snd_wnd: u16,
    pub irs: u32,
    pub rcv_nxt: u32,
    /// Dados que o programa mandou e o par ainda não confirmou (mais os que
    /// ainda não saíram).
    pub snd: Ring,
    /// Dados recebidos em ordem, ainda não lidos pelo programa.
    pub rcv: Ring,
    /// Quantos bytes do começo de `snd` estão no ar (no máximo `MSS`).
    pub in_flight: usize,
    pub rtx_deadline: u64,
    pub rtx_count: u8,
    pub tw_deadline: u64,
    pub fin_queued: bool,
    pub fin_sent: bool,
    pub fin_acked: bool,
    pub peer_fin: bool,
    pub ack_pending: bool,
    /// Há um programa dono (um socket). Conexões ainda na fila de aceitação, ou
    /// já fechadas pelo programa, não têm dono.
    pub owned: bool,
    pub err: Option<NetErr>,
    /// Ordem de criação, para escolher o `TIME_WAIT` mais antigo.
    pub born: u64,
    /// Só num `Listen`: as conexões prontas, à espera de `accept`.
    pub accept: [Option<usize>; ACCEPT_QUEUE],
    /// Nas conexões nascidas de um `listen`: o bloco do `listen`.
    pub parent: Option<usize>,
}

impl Tcb {
    pub const fn new() -> Tcb {
        Tcb {
            used: false,
            state: State::Closed,
            local_ip: [0; 4],
            local_port: 0,
            remote_ip: [0; 4],
            remote_port: 0,
            iss: 0,
            snd_una: 0,
            snd_nxt: 0,
            snd_wnd: 0,
            irs: 0,
            rcv_nxt: 0,
            snd: Ring::new(),
            rcv: Ring::new(),
            in_flight: 0,
            rtx_deadline: 0,
            rtx_count: 0,
            tw_deadline: 0,
            fin_queued: false,
            fin_sent: false,
            fin_acked: false,
            peer_fin: false,
            ack_pending: false,
            owned: false,
            err: None,
            born: 0,
            accept: [None; ACCEPT_QUEUE],
            parent: None,
        }
    }

    /// Devolve o bloco ao estado de livre, campo a campo (os buffers de 2 KiB
    /// não precisam ser zerados, e copiar um `Tcb` novo gastaria 4 KiB de pilha).
    pub fn clear(&mut self) {
        self.used = false;
        self.state = State::Closed;
        self.snd.clear();
        self.rcv.clear();
        self.in_flight = 0;
        self.rtx_deadline = 0;
        self.rtx_count = 0;
        self.tw_deadline = 0;
        self.fin_queued = false;
        self.fin_sent = false;
        self.fin_acked = false;
        self.peer_fin = false;
        self.ack_pending = false;
        self.owned = false;
        self.err = None;
        self.accept = [None; ACCEPT_QUEUE];
        self.parent = None;
        self.snd_wnd = 0;
    }

    fn is_synchronized(&self) -> bool {
        matches!(
            self.state,
            State::Established
                | State::FinWait1
                | State::FinWait2
                | State::CloseWait
                | State::Closing
                | State::LastAck
                | State::TimeWait
        )
    }
}

impl Default for Tcb {
    fn default() -> Self {
        Tcb::new()
    }
}

/// Um segmento recebido, já conferido (soma de verificação e deslocamento).
pub struct Segment<'a> {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub window: u16,
    pub payload: &'a [u8],
}

/// Interpreta um segmento: confere o tamanho, o deslocamento dos dados (as
/// opções são puladas) e a soma de verificação com o pseudocabeçalho.
pub fn parse<'a>(src_ip: [u8; 4], dst_ip: [u8; 4], bytes: &'a [u8]) -> Option<Segment<'a>> {
    if bytes.len() < HEADER_LEN {
        return None;
    }
    let offset = usize::from(bytes[12] >> 4) * 4;
    if offset < HEADER_LEN || offset > bytes.len() {
        return None;
    }
    let pseudo = pseudo_header(src_ip, dst_ip, bytes.len());
    if !checksum::verify(&[&pseudo, bytes]) {
        return None;
    }
    Some(Segment {
        src_port: u16::from_be_bytes([bytes[0], bytes[1]]),
        dst_port: u16::from_be_bytes([bytes[2], bytes[3]]),
        seq: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        ack: u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
        flags: bytes[13],
        window: u16::from_be_bytes([bytes[14], bytes[15]]),
        payload: &bytes[offset..],
    })
}

fn pseudo_header(src: [u8; 4], dst: [u8; 4], tcp_len: usize) -> [u8; 12] {
    let mut p = [0u8; 12];
    p[0..4].copy_from_slice(&src);
    p[4..8].copy_from_slice(&dst);
    p[9] = super::ip::PROTO_TCP;
    p[10..12].copy_from_slice(&(tcp_len as u16).to_be_bytes());
    p
}

/// Escreve o cabeçalho de um segmento em `out[..HEADER_LEN]`; os `data_len`
/// bytes de dados já devem estar em `out[HEADER_LEN..]`. Calcula a soma de
/// verificação e devolve o tamanho total.
#[allow(clippy::too_many_arguments)]
fn finish(
    out: &mut [u8],
    src: [u8; 4],
    dst: [u8; 4],
    ports: (u16, u16),
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    data_len: usize,
) -> usize {
    out[0..2].copy_from_slice(&ports.0.to_be_bytes());
    out[2..4].copy_from_slice(&ports.1.to_be_bytes());
    out[4..8].copy_from_slice(&seq.to_be_bytes());
    out[8..12].copy_from_slice(&ack.to_be_bytes());
    out[12] = 5 << 4;
    out[13] = flags;
    out[14..16].copy_from_slice(&window.to_be_bytes());
    out[16..20].copy_from_slice(&[0, 0, 0, 0]);
    let len = HEADER_LEN + data_len;
    let pseudo = pseudo_header(src, dst, len);
    let sum = checksum::sum(&[&pseudo, &out[..len]]);
    out[16..18].copy_from_slice(&sum.to_be_bytes());
    len
}

/// Monta um segmento completo em `out` (cabeçalho, dados e soma de verificação)
/// e devolve o tamanho. Os testes usam para fingir ser o outro lado.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn build_segment(
    out: &mut [u8],
    src: [u8; 4],
    dst: [u8; 4],
    ports: (u16, u16),
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    payload: &[u8],
) -> usize {
    out[HEADER_LEN..HEADER_LEN + payload.len()].copy_from_slice(payload);
    finish(out, src, dst, ports, seq, ack, flags, window, payload.len())
}

/// Envia um segmento da conexão `t` com os dados `snd[0..data_len]`.
fn emit(t: &Tcb, flags: u8, seq: u32, data_len: usize, tx: &mut dyn Tx) {
    let mut seg = [0u8; HEADER_LEN + MSS];
    if data_len > 0 {
        t.snd.copy_out(0, &mut seg[HEADER_LEN..HEADER_LEN + data_len]);
    }
    let ack = if flags & ACK != 0 { t.rcv_nxt } else { 0 };
    let len = finish(
        &mut seg,
        t.local_ip,
        t.remote_ip,
        (t.local_port, t.remote_port),
        seq,
        ack,
        flags,
        t.rcv.free() as u16,
        data_len,
    );
    tx.send(t.local_ip, t.remote_ip, &seg[..len]);
}

/// Responde a um segmento que não pertence a nenhuma conexão com um RST, pela
/// regra da RFC 793 (nunca responde a um RST).
fn reply_rst(local: [u8; 4], remote: [u8; 4], seg: &Segment, tx: &mut dyn Tx) {
    if seg.flags & RST != 0 {
        return;
    }
    let mut out = [0u8; HEADER_LEN];
    let (seq, ack, flags) = if seg.flags & ACK != 0 {
        (seg.ack, 0, RST)
    } else {
        let mut length = seg.payload.len() as u32;
        if seg.flags & SYN != 0 {
            length += 1;
        }
        if seg.flags & FIN != 0 {
            length += 1;
        }
        (0, seg.seq.wrapping_add(length), RST | ACK)
    };
    let len = finish(
        &mut out,
        local,
        remote,
        (seg.dst_port, seg.src_port),
        seq,
        ack,
        flags,
        0,
        0,
    );
    tx.send(local, remote, &out[..len]);
}

/// Os 8 blocos de conexão.
pub struct Tcp {
    pub tcbs: [Tcb; MAX_TCP_CONNECTIONS],
    isn_counter: u32,
    next_port: u16,
    born_counter: u64,
}

const FREE_TCB: Tcb = Tcb::new();

impl Tcp {
    pub const fn new() -> Tcp {
        Tcp {
            tcbs: [FREE_TCB; MAX_TCP_CONNECTIONS],
            isn_counter: 0,
            next_port: EPHEMERAL_FIRST,
            born_counter: 0,
        }
    }

    /// Libera todos os blocos (os testes recomeçam daqui).
    pub fn reset(&mut self) {
        for t in self.tcbs.iter_mut() {
            t.clear();
        }
        self.next_port = EPHEMERAL_FIRST;
    }

    /// Quantos blocos estão em uso (o número que o comando `rede` mostra).
    pub fn in_use(&self) -> usize {
        self.tcbs.iter().filter(|t| t.used).count()
    }

    /// Um bloco livre; se não há, toma o `TIME_WAIT` mais antigo (atalho
    /// documentado); senão `None`.
    fn alloc(&mut self) -> Option<usize> {
        let index = match self.tcbs.iter().position(|t| !t.used) {
            Some(i) => i,
            None => {
                let oldest = (0..self.tcbs.len())
                    .filter(|&i| self.tcbs[i].state == State::TimeWait)
                    .min_by_key(|&i| self.tcbs[i].born)?;
                self.release(oldest);
                oldest
            }
        };
        self.born_counter += 1;
        let t = &mut self.tcbs[index];
        t.clear();
        t.used = true;
        t.born = self.born_counter;
        Some(index)
    }

    /// Libera o bloco, tirando-o antes da fila de aceitação de qualquer `listen`.
    fn release(&mut self, index: usize) {
        for t in self.tcbs.iter_mut() {
            for slot in t.accept.iter_mut() {
                if *slot == Some(index) {
                    *slot = None;
                }
            }
        }
        self.tcbs[index].clear();
    }

    fn new_iss(&mut self, now: u64) -> u32 {
        self.isn_counter = self.isn_counter.wrapping_add(1);
        (now as u32).wrapping_mul(64_000).wrapping_add(self.isn_counter)
    }

    fn port_in_use(&self, port: u16) -> bool {
        self.tcbs.iter().any(|t| t.used && t.local_port == port)
    }

    /// Abre um `listen` na porta.
    pub fn listen(&mut self, port: u16) -> Result<usize, Error> {
        if self.tcbs.iter().any(|t| t.used && t.state == State::Listen && t.local_port == port) {
            return Err(Error::AddrInUse);
        }
        let index = self.alloc().ok_or(Error::NoBufs)?;
        let t = &mut self.tcbs[index];
        t.state = State::Listen;
        t.local_port = port;
        t.owned = true;
        Ok(index)
    }

    /// Começa uma conexão ativa: manda o SYN. O chamador espera o estado mudar.
    pub fn connect(
        &mut self,
        local_ip: [u8; 4],
        remote_ip: [u8; 4],
        remote_port: u16,
        now: u64,
        tx: &mut dyn Tx,
    ) -> Result<usize, Error> {
        let index = self.alloc().ok_or(Error::NoBufs)?;
        // Porta local: a próxima da faixa efêmera que não esteja em uso.
        let mut port = self.next_port;
        for _ in 0..(65536 - EPHEMERAL_FIRST as u32) {
            if !self.port_in_use(port) {
                break;
            }
            port = if port == 65535 { EPHEMERAL_FIRST } else { port + 1 };
        }
        self.next_port = if port == 65535 { EPHEMERAL_FIRST } else { port + 1 };
        let iss = self.new_iss(now);
        let t = &mut self.tcbs[index];
        t.state = State::SynSent;
        t.local_ip = local_ip;
        t.local_port = port;
        t.remote_ip = remote_ip;
        t.remote_port = remote_port;
        t.iss = iss;
        t.snd_una = iss;
        t.snd_nxt = iss.wrapping_add(1);
        t.owned = true;
        t.rtx_deadline = now + RTX_TICKS;
        t.rtx_count = 0;
        emit(t, SYN, iss, 0, tx);
        Ok(index)
    }

    /// Tira a próxima conexão pronta da fila do `listen` e a entrega ao
    /// programa (passa a ter dono).
    pub fn accept(&mut self, listener: usize) -> Option<usize> {
        let l = &mut self.tcbs[listener];
        let child = l.accept[0]?;
        l.accept[0] = l.accept[1];
        l.accept[1] = None;
        // Aceita: passa a ser do programa e deixa de depender do `listen`
        // (fechar o `listen` depois não deve derrubá-la).
        self.tcbs[child].owned = true;
        self.tcbs[child].parent = None;
        Some(child)
    }

    /// Copia para o buffer de envio o que couber de `data`. Não transmite
    /// além do que o para-e-espera permite.
    pub fn send(&mut self, index: usize, data: &[u8], now: u64, tx: &mut dyn Tx) -> usize {
        let t = &mut self.tcbs[index];
        if !matches!(t.state, State::Established | State::CloseWait) || t.fin_queued {
            return 0;
        }
        let n = t.snd.push(data);
        pump(t, now, tx);
        n
    }

    /// Retira dados recebidos para `out`.
    pub fn recv(&mut self, index: usize, out: &mut [u8], now: u64, tx: &mut dyn Tx) -> usize {
        let t = &mut self.tcbs[index];
        let free_before = t.rcv.free();
        let n = t.rcv.pop(out);
        // Se a janela anunciada estava pequena, avisa o par de que reabriu.
        if n > 0 && free_before < MSS {
            t.ack_pending = true;
        }
        pump(t, now, tx);
        n
    }

    /// O programa fechou o socket.
    pub fn close(&mut self, index: usize, now: u64, tx: &mut dyn Tx) {
        let state = self.tcbs[index].state;
        if !self.tcbs[index].used {
            return;
        }
        match state {
            State::Listen => {
                // Aborta as conexões que ninguém aceitou.
                for child in 0..self.tcbs.len() {
                    if self.tcbs[child].parent == Some(index) && self.tcbs[child].used {
                        let c = &self.tcbs[child];
                        if c.state != State::Closed {
                            emit(c, RST | ACK, c.snd_nxt, 0, tx);
                        }
                        self.release(child);
                    }
                }
                self.release(index);
            }
            State::Closed | State::SynSent => self.release(index),
            State::SynReceived | State::Established | State::CloseWait => {
                let t = &mut self.tcbs[index];
                t.owned = false;
                t.fin_queued = true;
                pump(t, now, tx);
            }
            _ => self.tcbs[index].owned = false,
        }
    }

    /// A conexão caiu: se há dono, guarda o erro para a próxima chamada dele;
    /// senão libera o bloco.
    fn fail(&mut self, index: usize, err: NetErr) {
        if self.tcbs[index].owned {
            let t = &mut self.tcbs[index];
            t.state = State::Closed;
            t.err = Some(err);
            t.in_flight = 0;
            t.snd.clear();
            t.ack_pending = false;
        } else {
            self.release(index);
        }
    }

    /// Trata um segmento recebido (`src_ip` e `dst_ip` são os do pacote IP).
    pub fn on_segment(
        &mut self,
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        bytes: &[u8],
        now: u64,
        tx: &mut dyn Tx,
    ) {
        let Some(seg) = parse(src_ip, dst_ip, bytes) else {
            return;
        };
        let conn = (0..self.tcbs.len()).find(|&i| {
            let t = &self.tcbs[i];
            t.used
                && !matches!(t.state, State::Closed | State::Listen)
                && t.local_port == seg.dst_port
                && t.remote_ip == src_ip
                && t.remote_port == seg.src_port
        });
        if let Some(index) = conn {
            if self.tcbs[index].state == State::SynSent {
                self.on_syn_sent(index, &seg, now, tx);
            } else {
                self.on_synchronized(index, &seg, now, tx);
            }
            return;
        }
        // Sem conexão: um SYN puro para um `listen` abre uma; o resto leva RST.
        if seg.flags & (SYN | ACK | RST) == SYN {
            let listener = self.tcbs.iter().position(|t| {
                t.used && t.state == State::Listen && t.local_port == seg.dst_port
            });
            if let Some(listener) = listener {
                self.on_listen_syn(listener, src_ip, dst_ip, &seg, now, tx);
                return;
            }
        }
        reply_rst(dst_ip, src_ip, &seg, tx);
    }

    fn on_listen_syn(
        &mut self,
        listener: usize,
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        seg: &Segment,
        now: u64,
        tx: &mut dyn Tx,
    ) {
        // A fila guarda no máximo `ACCEPT_QUEUE` conexões, contando as que
        // ainda estão no aperto de mão. Além disso o SYN é ignorado: o par o
        // reenvia.
        let pending = self
            .tcbs
            .iter()
            .filter(|t| t.used && t.parent == Some(listener) && t.state != State::Closed)
            .count();
        if pending >= ACCEPT_QUEUE {
            return;
        }
        let iss = self.new_iss(now);
        let Some(index) = self.alloc() else {
            return;
        };
        let t = &mut self.tcbs[index];
        t.state = State::SynReceived;
        t.local_ip = dst_ip;
        t.local_port = seg.dst_port;
        t.remote_ip = src_ip;
        t.remote_port = seg.src_port;
        t.iss = iss;
        t.snd_una = iss;
        t.snd_nxt = iss.wrapping_add(1);
        t.irs = seg.seq;
        t.rcv_nxt = seg.seq.wrapping_add(1);
        t.snd_wnd = seg.window;
        t.parent = Some(listener);
        t.rtx_deadline = now + RTX_TICKS;
        t.rtx_count = 0;
        emit(t, SYN | ACK, iss, 0, tx);
    }

    fn on_syn_sent(&mut self, index: usize, seg: &Segment, now: u64, tx: &mut dyn Tx) {
        let t = &mut self.tcbs[index];
        let ack_ok = seg.flags & ACK != 0 && seg.ack == t.iss.wrapping_add(1);
        if seg.flags & ACK != 0 && !ack_ok {
            // Confirmação de algo que não enviamos: RST, se não for um RST.
            reply_rst(t.local_ip, t.remote_ip, seg, tx);
            return;
        }
        if seg.flags & RST != 0 {
            if ack_ok {
                self.fail(index, NetErr::ConnRefused);
            }
            return;
        }
        if seg.flags & SYN != 0 && ack_ok {
            t.irs = seg.seq;
            t.rcv_nxt = seg.seq.wrapping_add(1);
            t.snd_una = seg.ack;
            t.snd_wnd = seg.window;
            t.state = State::Established;
            t.rtx_count = 0;
            t.ack_pending = true;
            pump(t, now, tx);
        }
        // SYN sem ACK seria abertura simultânea: não é tratada.
    }

    fn on_synchronized(&mut self, index: usize, seg: &Segment, now: u64, tx: &mut dyn Tx) {
        // RST: derruba a conexão se o número de sequência cabe na janela.
        if seg.flags & RST != 0 {
            let t = &self.tcbs[index];
            let window = t.rcv.free().max(1) as u32;
            let in_window = seq_le(t.rcv_nxt, seg.seq) && seq_lt(seg.seq, t.rcv_nxt.wrapping_add(window));
            if in_window {
                match t.state {
                    State::SynReceived | State::TimeWait => self.release(index),
                    _ => self.fail(index, NetErr::ConnReset),
                }
            }
            return;
        }
        if seg.flags & SYN != 0 {
            // SYN repetido no aperto de mão: reenvia o SYN+ACK. Em outros
            // estados é ignorado.
            let t = &self.tcbs[index];
            if t.state == State::SynReceived && seg.seq == t.irs {
                emit(t, SYN | ACK, t.iss, 0, tx);
            }
            return;
        }
        if seg.flags & ACK == 0 {
            return;
        }

        if self.tcbs[index].state == State::SynReceived {
            let t = &mut self.tcbs[index];
            if seg.ack != t.iss.wrapping_add(1) {
                reply_rst(t.local_ip, t.remote_ip, seg, tx);
                return;
            }
            t.snd_una = seg.ack;
            t.snd_wnd = seg.window;
            t.rtx_count = 0;
            t.state = State::Established;
            // Entra na fila do `listen`; se ele já fechou, desfaz.
            let parent = t.parent;
            let queued = parent.is_some_and(|p| {
                if self.tcbs[p].used && self.tcbs[p].state == State::Listen {
                    let slot = self.tcbs[p].accept.iter().position(|s| s.is_none());
                    if let Some(slot) = slot {
                        self.tcbs[p].accept[slot] = Some(index);
                        return true;
                    }
                }
                false
            });
            if !queued {
                let t = &self.tcbs[index];
                emit(t, RST | ACK, t.snd_nxt, 0, tx);
                self.release(index);
                return;
            }
        } else if !self.process_ack(index, seg, now) {
            return;
        }

        let t = &mut self.tcbs[index];
        let can_receive = matches!(t.state, State::Established | State::FinWait1 | State::FinWait2);
        let mut ack_needed = false;
        let mut all_taken = true;
        if !seg.payload.is_empty() {
            ack_needed = true;
            if can_receive && seg.seq == t.rcv_nxt {
                let n = t.rcv.push(seg.payload);
                t.rcv_nxt = t.rcv_nxt.wrapping_add(n as u32);
                all_taken = n == seg.payload.len();
            } else {
                // Fora de ordem, repetido, ou depois do FIN do par: só
                // reconfirma o último byte em ordem.
                all_taken = false;
            }
        }
        if seg.flags & FIN != 0 {
            ack_needed = true;
            let fin_seq = seg.seq.wrapping_add(seg.payload.len() as u32);
            if all_taken && fin_seq == t.rcv_nxt && !t.peer_fin {
                t.peer_fin = true;
                t.rcv_nxt = t.rcv_nxt.wrapping_add(1);
                match t.state {
                    State::Established => t.state = State::CloseWait,
                    State::FinWait1 => {
                        t.state = State::Closing;
                    }
                    State::FinWait2 => {
                        t.state = State::TimeWait;
                        t.tw_deadline = now + TIME_WAIT_TICKS;
                    }
                    _ => {}
                }
            } else if t.state == State::TimeWait {
                t.tw_deadline = now + TIME_WAIT_TICKS;
            }
        }
        if ack_needed {
            t.ack_pending = true;
        }
        pump(t, now, tx);
    }

    /// Processa o campo de confirmação. Devolve `false` se o segmento não deve
    /// ser processado mais (ou se a conexão acabou).
    fn process_ack(&mut self, index: usize, seg: &Segment, now: u64) -> bool {
        let t = &mut self.tcbs[index];
        if seq_lt(t.snd_nxt, seg.ack) {
            // Confirma o que nunca enviamos.
            t.ack_pending = true;
            return false;
        }
        if seq_lt(t.snd_una, seg.ack) {
            let mut acked = seg.ack.wrapping_sub(t.snd_una) as usize;
            let data = acked.min(t.in_flight);
            t.snd.consume(data);
            t.in_flight -= data;
            acked -= data;
            if acked > 0 && t.fin_sent && !t.fin_acked {
                t.fin_acked = true;
            }
            t.snd_una = seg.ack;
            t.rtx_count = 0;
            t.rtx_deadline = now + RTX_TICKS;
        }
        if !seq_lt(seg.ack, t.snd_una) {
            t.snd_wnd = seg.window;
        }
        if t.fin_acked {
            match t.state {
                State::FinWait1 => t.state = State::FinWait2,
                State::Closing => {
                    t.state = State::TimeWait;
                    t.tw_deadline = now + TIME_WAIT_TICKS;
                }
                State::LastAck => {
                    self.release(index);
                    return false;
                }
                _ => {}
            }
        }
        true
    }

    /// Avança os temporizadores: retransmissão, `SYN`, `TIME_WAIT`, e envia o
    /// que estiver pendente.
    pub fn on_tick(&mut self, now: u64, tx: &mut dyn Tx) {
        for index in 0..self.tcbs.len() {
            if !self.tcbs[index].used {
                continue;
            }
            match self.tcbs[index].state {
                State::Closed | State::Listen => {}
                State::SynSent => {
                    let t = &mut self.tcbs[index];
                    if now >= t.rtx_deadline {
                        if t.rtx_count + 1 >= SYN_TRIES {
                            self.fail(index, NetErr::TimedOut);
                        } else {
                            t.rtx_count += 1;
                            t.rtx_deadline = now + RTX_TICKS;
                            emit(t, SYN, t.iss, 0, tx);
                        }
                    }
                }
                State::SynReceived => {
                    let t = &mut self.tcbs[index];
                    if now >= t.rtx_deadline {
                        if t.rtx_count >= MAX_RTX {
                            self.release(index);
                        } else {
                            t.rtx_count += 1;
                            t.rtx_deadline = now + RTX_TICKS;
                            emit(t, SYN | ACK, t.iss, 0, tx);
                        }
                    }
                }
                State::TimeWait => {
                    if now >= self.tcbs[index].tw_deadline {
                        self.release(index);
                    }
                }
                _ => {
                    let t = &mut self.tcbs[index];
                    let outstanding = t.in_flight > 0 || (t.fin_sent && !t.fin_acked);
                    if outstanding && now >= t.rtx_deadline {
                        // Com a janela do par fechada, o segmento no ar é uma
                        // sonda de 1 byte: reenviá-la não conta para a queda.
                        let probing = t.in_flight > 0 && t.snd_wnd == 0;
                        if !probing && t.rtx_count >= MAX_RTX {
                            self.fail(index, NetErr::TimedOut);
                            continue;
                        }
                        if !probing {
                            t.rtx_count += 1;
                        }
                        t.rtx_deadline = now + RTX_TICKS;
                        if t.in_flight > 0 {
                            emit(t, PSH | ACK, t.snd_una, t.in_flight, tx);
                        } else {
                            emit(t, FIN | ACK, t.snd_nxt.wrapping_sub(1), 0, tx);
                        }
                    }
                    pump(&mut self.tcbs[index], now, tx);
                }
            }
        }
    }
}

impl Default for Tcp {
    fn default() -> Self {
        Tcp::new()
    }
}

/// Envia o que o para-e-espera permite: o próximo segmento de dados (se nada
/// está no ar), o FIN (depois de todos os dados confirmados) e a confirmação
/// pendente.
fn pump(t: &mut Tcb, now: u64, tx: &mut dyn Tx) {
    if matches!(t.state, State::Established | State::CloseWait) {
        if t.in_flight == 0 && !t.snd.is_empty() {
            let window = usize::from(t.snd_wnd);
            // Janela zero: um byte de sonda.
            let n = if window == 0 { 1 } else { t.snd.len().min(MSS).min(window) };
            t.in_flight = n;
            emit(t, PSH | ACK, t.snd_nxt, n, tx);
            t.snd_nxt = t.snd_nxt.wrapping_add(n as u32);
            t.rtx_deadline = now + RTX_TICKS;
            t.rtx_count = 0;
            t.ack_pending = false;
        }
        if t.fin_queued && !t.fin_sent && t.snd.is_empty() && t.in_flight == 0 {
            emit(t, FIN | ACK, t.snd_nxt, 0, tx);
            t.fin_sent = true;
            t.snd_nxt = t.snd_nxt.wrapping_add(1);
            t.rtx_deadline = now + RTX_TICKS;
            t.rtx_count = 0;
            t.ack_pending = false;
            t.state = if t.state == State::Established { State::FinWait1 } else { State::LastAck };
        }
    }
    if t.ack_pending && t.is_synchronized() {
        emit(t, ACK, t.snd_nxt, 0, tx);
        t.ack_pending = false;
    }
}
