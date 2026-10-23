//! O driver da placa de rede Realtek RTL8139, por polling.
//!
//! É a placa mais simples de programar e a que o QEMU emula com
//! `-nic user,model=rtl8139`. O caminho é o do manual da Realtek:
//!
//! - **Recepção**: a placa copia cada quadro recebido (por DMA) para um anel de
//!   8 KiB na memória, precedido de 4 bytes (`status`, `tamanho`). O driver
//!   guarda onde parou de ler (`rx_off`) e avisa a placa de quanto já leu
//!   (`CAPR`). Com o bit `WRAP` ligado a placa deixa o último quadro passar do
//!   fim do anel, para o driver nunca precisar costurar um quadro partido em
//!   dois; por isso o buffer tem uma folga de 1536 bytes depois dos 8 KiB.
//! - **Transmissão**: quatro buffers de 1536 bytes usados em rodízio. Escreve-se
//!   o endereço físico do buffer e depois o tamanho do quadro; a placa envia e
//!   marca o buffer como livre.
//!
//! A placa faz DMA, então fala em endereços **físicos**: os buffers são
//! `static` alinhados em página, e o endereço físico de cada página vem da
//! tabela de páginas ativa (`memory::translate_addr`). O driver confere que as
//! páginas são contíguas e ficam abaixo de 4 GiB (o anel é programado com 32
//! bits); se não for assim, a placa é declarada indisponível.
//!
//! As interrupções da placa ficam desligadas (`IMR = 0`) e o PIC não é tocado:
//! quem quer saber se chegou algo pergunta (`receive`).

use core::ptr::{addr_of_mut, read_volatile};

use x86_64::instructions::port::Port;
use x86_64::VirtAddr;

use crate::memory;
use crate::pci;

/// Fabricante e modelo da RTL8139 no PCI.
const VENDOR: u16 = 0x10EC;
const DEVICE: u16 = 0x8139;

// Registradores, como deslocamentos da porta base de E/S.
const IDR0: u16 = 0x00;
const TSD0: u16 = 0x10;
const TSAD0: u16 = 0x20;
const RBSTART: u16 = 0x30;
const CMD: u16 = 0x37;
const CAPR: u16 = 0x38;
const CBR: u16 = 0x3A;
const IMR: u16 = 0x3C;
const ISR: u16 = 0x3E;
const RCR: u16 = 0x44;
const CONFIG1: u16 = 0x52;

/// `CMD`: reset por software (o bit volta a zero quando termina).
const CMD_RESET: u8 = 0x10;
/// `CMD`: receptor e transmissor ligados.
const CMD_RX_TX_ENABLE: u8 = 0x0C;
/// `CMD`: o anel de recepção está vazio.
const CMD_BUFFER_EMPTY: u8 = 0x01;
/// `RCR`: aceita difusão (broadcast), quadros para o MAC da placa e deixa o
/// último quadro passar do fim do anel (`WRAP`).
const RCR_VALUE: u32 = 0x08 | 0x02 | 0x80;
/// `TSD`: a placa terminou de enviar e devolveu o buffer ao driver.
const TSD_OWN: u32 = 1 << 13;
/// Cabeçalho que a placa escreve antes de cada quadro: `status` e `tamanho`.
const RX_HEADER: usize = 4;
/// Bit de `status` do quadro: recebido sem erro.
const RX_OK: u16 = 0x0001;

/// Tamanho do anel que a placa percorre.
const RX_RING: usize = 8192;
/// Tamanho reservado ao anel: 8 KiB, 16 bytes de folga do manual, 1536 bytes
/// para o último quadro passar do fim (`WRAP`), arredondado a 3 páginas.
const RX_BUFFER: usize = 3 * 4096;
/// Quantos buffers de transmissão e quanto cada um comporta.
const TX_BUFFERS: usize = 4;
const TX_SIZE: usize = 1536;
/// Menor quadro Ethernet sem o CRC: a placa não completa sozinha.
const MIN_FRAME: usize = 60;
/// Maior quadro (cabeçalho Ethernet mais MTU de 1500).
pub const MAX_FRAME: usize = 1514;

/// Limites de leituras de status (o kernel nunca espera para sempre).
const RESET_MAX_POLLS: usize = 100_000;
const TX_MAX_POLLS: usize = 100_000;

/// A memória da placa: tudo `static` alinhado em página para o DMA.
#[repr(C, align(4096))]
struct Dma {
    rx: [u8; RX_BUFFER],
    tx: [u8; TX_BUFFERS * TX_SIZE],
}

static mut DMA: Dma = Dma { rx: [0; RX_BUFFER], tx: [0; TX_BUFFERS * TX_SIZE] };

/// Páginas que o bloco de DMA ocupa.
const DMA_PAGES: usize = (core::mem::size_of::<Dma>() + 4095) / 4096;

/// A placa pronta para uso.
pub struct Card {
    io: u16,
    mac: [u8; 6],
    rx_off: usize,
    tx_next: usize,
    tx_used: [bool; TX_BUFFERS],
    dma_phys: u64,
}

/// O que a inicialização encontrou.
pub enum CardState {
    /// O PCI não achou uma RTL8139.
    Absent,
    /// Achou, mas não deu para usar (o motivo é texto fixo).
    Unavailable(&'static str),
    /// Pronta.
    Ready(Card),
}

fn in8(port: u16) -> u8 {
    // SAFETY: lê um registrador da RTL8139 na faixa de E/S que o PCI atribuiu
    // à placa (BAR0); não mexe em memória.
    unsafe { Port::<u8>::new(port).read() }
}
fn in16(port: u16) -> u16 {
    // SAFETY: igual a `in8`.
    unsafe { Port::<u16>::new(port).read() }
}
fn in32(port: u16) -> u32 {
    // SAFETY: igual a `in8`.
    unsafe { Port::<u32>::new(port).read() }
}
fn out8(port: u16, value: u8) {
    // SAFETY: escreve num registrador da RTL8139 (faixa de E/S da placa).
    unsafe { Port::<u8>::new(port).write(value) }
}
fn out16(port: u16, value: u16) {
    // SAFETY: igual a `out8`.
    unsafe { Port::<u16>::new(port).write(value) }
}
fn out32(port: u16, value: u32) {
    // SAFETY: igual a `out8`; os endereços escritos são os dos buffers de DMA
    // deste módulo, já conferidos em `init`.
    unsafe { Port::<u32>::new(port).write(value) }
}

/// Acha e inicializa a placa. `physical_memory_offset` é o do `BootInfo`, com o
/// qual `memory::translate_addr` traduz endereços virtuais em físicos. Nunca
/// entra em pânico: qualquer problema vira `Absent` ou `Unavailable`.
pub fn init(physical_memory_offset: VirtAddr) -> CardState {
    let Some(device) = pci::find(VENDOR, DEVICE) else {
        return CardState::Absent;
    };
    pci::enable_io_and_bus_master(&device);
    let Some(io) = pci::bar0_io_port(&device) else {
        return CardState::Unavailable("BAR0 nao e de E/S");
    };

    let Some(dma_phys) = dma_physical_base(physical_memory_offset) else {
        return CardState::Unavailable("DMA nao contiguo");
    };

    // Liga a placa, faz o reset e espera o bit de reset sumir.
    out8(io + CONFIG1, 0);
    out8(io + CMD, CMD_RESET);
    let mut polls = 0;
    while in8(io + CMD) & CMD_RESET != 0 {
        polls += 1;
        if polls > RESET_MAX_POLLS {
            return CardState::Unavailable("reset nao terminou");
        }
    }

    let mut mac = [0u8; 6];
    for (i, byte) in mac.iter_mut().enumerate() {
        *byte = in8(io + IDR0 + i as u16);
    }

    // Anel de recepção, sem interrupções, aceitando só o que é para nós.
    out32(io + RBSTART, dma_phys as u32);
    out16(io + IMR, 0);
    out16(io + ISR, 0xFFFF);
    out32(io + RCR, RCR_VALUE);
    out8(io + CMD, CMD_RX_TX_ENABLE);

    CardState::Ready(Card {
        io,
        mac,
        rx_off: 0,
        tx_next: 0,
        tx_used: [false; TX_BUFFERS],
        dma_phys,
    })
}

/// O endereço físico do bloco de DMA, se todas as páginas dele são contíguas e
/// terminam abaixo de 4 GiB.
fn dma_physical_base(offset: VirtAddr) -> Option<u64> {
    let virt = addr_of_mut!(DMA) as u64;
    // SAFETY: `offset` é o `physical_memory_offset` real do `BootInfo`, o que
    // `translate_addr` exige; a tabela de páginas ativa é a do kernel, que
    // mapeia o `.bss` onde `DMA` mora.
    let base = unsafe { memory::translate_addr(VirtAddr::new(virt), offset) }?.as_u64();
    for page in 1..DMA_PAGES as u64 {
        // SAFETY: igual acima.
        let phys =
            unsafe { memory::translate_addr(VirtAddr::new(virt + page * 4096), offset) }?.as_u64();
        if phys != base + page * 4096 {
            return None;
        }
    }
    let end = base + (DMA_PAGES as u64) * 4096;
    (base % 4096 == 0 && end <= (1 << 32)).then_some(base)
}

impl Card {
    /// O endereço MAC lido da placa.
    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn rx_ptr(&self) -> *const u8 {
        // SAFETY: só calcula o endereço do anel (`static` deste módulo).
        unsafe { addr_of_mut!(DMA.rx) as *const u8 }
    }

    /// Envia um quadro Ethernet (sem CRC). Devolve `false` se o quadro é grande
    /// demais ou se nenhum buffer ficou livre no limite de leituras.
    pub fn transmit(&mut self, frame: &[u8]) -> bool {
        if frame.is_empty() || frame.len() > MAX_FRAME {
            return false;
        }
        let n = self.tx_next;
        // O buffer só está livre se nunca foi usado ou se a placa já o devolveu.
        let mut polls = 0;
        while self.tx_used[n] && in32(self.io + TSD0 + 4 * n as u16) & TSD_OWN == 0 {
            polls += 1;
            if polls > TX_MAX_POLLS {
                return false;
            }
        }
        // SAFETY: `n < TX_BUFFERS`, então o intervalo cabe em `DMA.tx`; a placa
        // não lê este buffer enquanto `TSD` não for escrito (OWN já voltou).
        let dst = unsafe {
            core::slice::from_raw_parts_mut(
                (addr_of_mut!(DMA.tx) as *mut u8).add(n * TX_SIZE),
                TX_SIZE,
            )
        };
        let len = frame.len().max(MIN_FRAME);
        dst[..frame.len()].copy_from_slice(frame);
        dst[frame.len()..len].fill(0);
        let phys = self.dma_phys + RX_BUFFER as u64 + (n * TX_SIZE) as u64;
        out32(self.io + TSAD0 + 4 * n as u16, phys as u32);
        out32(self.io + TSD0 + 4 * n as u16, len as u32);
        self.tx_used[n] = true;
        self.tx_next = (n + 1) % TX_BUFFERS;
        true
    }

    /// Copia para `buf` o próximo quadro recebido (sem CRC). `None` se o anel
    /// está vazio. Quadros com erro ou maiores que `MAX_FRAME` são descartados
    /// aqui mesmo (a pilha nunca os vê) e o anel avança. O segundo valor é
    /// quantos quadros foram descartados nesta chamada.
    pub fn receive(&mut self, buf: &mut [u8]) -> (Option<usize>, u32) {
        let mut dropped = 0;
        while in8(self.io + CMD) & CMD_BUFFER_EMPTY == 0 {
            let header = self.rx_ptr();
            // SAFETY: `rx_off < RX_RING` e o cabeçalho (4 bytes) mais um quadro
            // de até 1518 bytes cabem nos 9744 do anel com folga; a leitura é
            // volátil porque a placa escreve nesse buffer por DMA.
            let (status, length) = unsafe {
                let p = header.add(self.rx_off);
                (
                    u16::from_le_bytes([read_volatile(p), read_volatile(p.add(1))]),
                    u16::from_le_bytes([read_volatile(p.add(2)), read_volatile(p.add(3))]) as usize,
                )
            };
            // Tamanho fora do possível: o anel não é confiável. Recomeça de onde
            // a placa está escrevendo (`CBR`).
            if length < RX_HEADER + 4 || length > MAX_FRAME + 4 + 4 {
                self.rx_off = in16(self.io + CBR) as usize % RX_RING;
                out16(self.io + CAPR, (self.rx_off as u16).wrapping_sub(16));
                dropped += 1;
                continue;
            }
            let data = length - 4; // tira o CRC
            let good = status & RX_OK != 0 && data <= MAX_FRAME && data <= buf.len();
            if good {
                // SAFETY: `rx_off + 4 + data` fica dentro do buffer do anel (ver
                // acima); `data <= buf.len()` foi conferido.
                unsafe {
                    let p = header.add(self.rx_off + RX_HEADER);
                    for (i, byte) in buf[..data].iter_mut().enumerate() {
                        *byte = read_volatile(p.add(i));
                    }
                }
            } else {
                dropped += 1;
            }
            self.rx_off = (self.rx_off + length + RX_HEADER + 3) & !3;
            self.rx_off %= RX_RING;
            out16(self.io + CAPR, (self.rx_off as u16).wrapping_sub(16));
            if good {
                return (Some(data), dropped);
            }
        }
        (None, dropped)
    }
}
