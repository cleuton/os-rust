//! Driver de disco ATA por PIO.
//!
//! O disco IDE é controlado por alguns registradores em portas de E/S. Ler um
//! setor é uma conversa curta: escolher o disco, dizer qual setor (endereço
//! LBA) e mandar o comando `READ SECTORS`; esperar o disco ficar pronto; e
//! ler 256 palavras de 16 bits da porta de dados. "PIO" quer dizer que é o
//! processador quem copia os bytes, uma palavra por vez, sem DMA.
//!
//! Duas escolhas mantêm o driver pequeno:
//!
//! - **Polling, sem interrupção.** O bit `nIEN` do registrador de controle
//!   desliga a IRQ do disco, e o PIC não muda: continua só com o timer e o
//!   teclado. Em vez de dormir até o disco avisar, o kernel lê o registrador
//!   de status em laço até o disco responder.
//! - **Toda espera tem limite.** Um laço de polling sem limite travaria o
//!   kernel para sempre se o disco não respondesse. Cada espera lê o status no
//!   máximo [`MAX_POLLS`] vezes e depois desiste com `Timeout`. Não há relógio
//!   no meio: o limite é uma contagem de leituras.
//!
//! **Atomicidade.** A leitura de um setor é uma sequência de acessos às
//! portas que não pode ser intercalada com outra: se outro código mexesse no
//! controlador no meio, os registradores ficariam inconsistentes. O kernel não
//! é preemptivo (o timer só troca de programa quando interrompe ring 3), mas a
//! regra fica explícita no código: a sequência inteira roda com interrupções
//! desligadas e sob uma trava do controlador.

use spin::Mutex;
use x86_64::instructions::interrupts;
use x86_64::instructions::port::Port;

use crate::blockdev::{BlockDevice, BlockError, SECTOR_SIZE};

/// Quantas leituras do registrador de status cada espera faz, no máximo, antes
/// de desistir.
pub const MAX_POLLS: u32 = 100_000;

/// Status: o disco está ocupado.
const STATUS_BSY: u8 = 0x80;
/// Status: o disco tem dados prontos para ler (ou espera dados).
const STATUS_DRQ: u8 = 0x08;
/// Status: o último comando falhou.
const STATUS_ERR: u8 = 0x01;
/// Status: falha do dispositivo.
const STATUS_DF: u8 = 0x20;

/// Comando `READ SECTORS` (LBA28).
const CMD_READ_SECTORS: u8 = 0x20;
/// Comando `IDENTIFY DEVICE`.
const CMD_IDENTIFY: u8 = 0xEC;
/// Registrador de controle: bit 1 (`nIEN`) desliga a IRQ do disco.
const CONTROL_NIEN: u8 = 0x02;

/// Os dois canais IDE do PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    Primary,
    Secondary,
}

impl Bus {
    /// Primeira porta do bloco de registradores de comando (8 portas).
    fn io_base(self) -> u16 {
        match self {
            Bus::Primary => 0x1F0,
            Bus::Secondary => 0x170,
        }
    }

    /// Porta do registrador de controle (escrita) e de status alternativo
    /// (leitura, sem efeito colateral).
    fn control(self) -> u16 {
        match self {
            Bus::Primary => 0x3F6,
            Bus::Secondary => 0x376,
        }
    }
}

/// Cada canal tem dois discos: mestre e escravo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drive {
    Master,
    Slave,
}

/// Por que o disco não pôde ser usado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtaError {
    /// Nada respondeu nesse canal e posição.
    NoDevice,
    /// Há um dispositivo, mas não é um disco ATA (por exemplo, ATAPI).
    NotAta,
    /// O disco não respondeu dentro de `MAX_POLLS` leituras.
    Timeout,
    /// O disco reportou erro.
    DeviceError,
}

impl From<AtaError> for BlockError {
    fn from(error: AtaError) -> BlockError {
        match error {
            AtaError::NoDevice | AtaError::NotAta => BlockError::NoDevice,
            AtaError::Timeout => BlockError::Timeout,
            AtaError::DeviceError => BlockError::DeviceError,
        }
    }
}

/// Lê o status até o disco sair de `BSY` e `done` ficar verdadeiro, com no
/// máximo `max` leituras. Um bit de erro (`ERR` ou `DF`) com o disco livre é
/// `DeviceError`; esgotar as leituras é `Timeout`. É a única espera do driver
/// e não toca em hardware por conta própria: quem chama diz como ler o status,
/// o que permite testá-la sem disco.
pub fn wait(mut read_status: impl FnMut() -> u8, done: impl Fn(u8) -> bool, max: u32) -> Result<u8, AtaError> {
    for _ in 0..max {
        let status = read_status();
        // Com `BSY` ligado, os outros bits não valem nada.
        if status & STATUS_BSY != 0 {
            continue;
        }
        if status & (STATUS_ERR | STATUS_DF) != 0 {
            return Err(AtaError::DeviceError);
        }
        if done(status) {
            return Ok(status);
        }
    }
    Err(AtaError::Timeout)
}

fn inb(port: u16) -> u8 {
    // SAFETY: lê uma porta de E/S de um registrador do controlador IDE
    // (`0x1F0..=0x1F7`, `0x170..=0x177`, `0x3F6`, `0x376`); ler esses
    // registradores não mexe em memória e só é chamado pela sequência de
    // acesso ao controlador abaixo, sob a trava do controlador.
    unsafe { Port::<u8>::new(port).read() }
}

fn outb(port: u16, value: u8) {
    // SAFETY: escreve em uma porta de E/S de um registrador do controlador
    // IDE (as mesmas de `inb`); o efeito é só comandar o disco, nunca
    // memória do kernel.
    unsafe { Port::<u8>::new(port).write(value) }
}

fn inw(port: u16) -> u16 {
    // SAFETY: lê a porta de dados do controlador IDE (`io_base`); só é
    // chamada depois de o disco sinalizar `DRQ` (dados prontos).
    unsafe { Port::<u16>::new(port).read() }
}

/// Trava do controlador: uma conversa com o disco por vez.
static CONTROLLER: Mutex<()> = Mutex::new(());

/// Um disco ATA já identificado.
#[derive(Debug, Clone, Copy)]
pub struct AtaDisk {
    bus: Bus,
    drive: Drive,
    sectors: u32,
}

impl AtaDisk {
    /// Procura um disco ATA no canal e na posição dados. Desliga a IRQ do
    /// disco, seleciona-o e manda `IDENTIFY`. Status `0x00` ou `0xFF` depois de
    /// selecionar é ausência (barramento sem resposta ou flutuando);
    /// assinatura diferente de ATA nos registradores LBA é outro tipo de
    /// dispositivo.
    pub fn detect(bus: Bus, drive: Drive) -> Result<AtaDisk, AtaError> {
        let _controller = CONTROLLER.lock();
        interrupts::without_interrupts(|| {
            let io = bus.io_base();
            // Sem IRQ: o PIC não precisa saber que este disco existe.
            outb(bus.control(), CONTROL_NIEN);
            select(bus, drive, 0);
            let status = inb(bus.control());
            if status == 0x00 || status == 0xFF {
                return Err(AtaError::NoDevice);
            }
            outb(io + 2, 0);
            outb(io + 3, 0);
            outb(io + 4, 0);
            outb(io + 5, 0);
            outb(io + 7, CMD_IDENTIFY);
            if inb(bus.control()) == 0 {
                return Err(AtaError::NoDevice);
            }
            // Espera o disco largar `BSY`. Um erro aqui (`ERR`) é, em geral, um
            // dispositivo ATAPI que recusou `IDENTIFY`: não é disco ATA.
            match wait(|| inb(bus.control()), |_| true, MAX_POLLS) {
                Ok(_) => {}
                Err(AtaError::DeviceError) => return Err(AtaError::NotAta),
                Err(error) => return Err(error),
            }
            // Discos ATA deixam zero nestes registradores; ATAPI e SATA, não.
            if inb(io + 4) != 0 || inb(io + 5) != 0 {
                return Err(AtaError::NotAta);
            }
            wait(|| inb(bus.control()), |s| s & STATUS_DRQ != 0, MAX_POLLS)?;
            let mut words = [0u16; 256];
            for word in words.iter_mut() {
                *word = inw(io);
            }
            // Palavras 60 e 61: total de setores endereçáveis por LBA28.
            let sectors = words[60] as u32 | (words[61] as u32) << 16;
            if sectors == 0 {
                return Err(AtaError::NotAta);
            }
            Ok(AtaDisk { bus, drive, sectors })
        })
    }
}

/// Seleciona o disco (e os 4 bits altos do LBA28) e espera os 400 ns que o
/// padrão exige antes de confiar no status: quatro leituras do status
/// alternativo.
fn select(bus: Bus, drive: Drive, lba_high_bits: u8) {
    let slave = match drive {
        Drive::Master => 0,
        Drive::Slave => 1,
    };
    // Bits 7 e 5 sempre 1, bit 6 = modo LBA, bit 4 = qual disco.
    outb(bus.io_base() + 6, 0xE0 | slave << 4 | (lba_high_bits & 0x0F));
    for _ in 0..4 {
        inb(bus.control());
    }
}

impl BlockDevice for AtaDisk {
    fn sector_count(&self) -> u32 {
        self.sectors
    }

    fn read_sector(&self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockError> {
        if lba >= self.sectors {
            return Err(BlockError::OutOfRange);
        }
        // Atomicidade (ver o cabeçalho do módulo): trava do controlador e
        // interrupções desligadas durante toda a sequência de portas.
        let _controller = CONTROLLER.lock();
        interrupts::without_interrupts(|| -> Result<(), AtaError> {
            let io = self.bus.io_base();
            select(self.bus, self.drive, (lba >> 24) as u8);
            // Disco livre antes de mandar o comando.
            wait(|| inb(self.bus.control()), |_| true, MAX_POLLS)?;
            outb(io + 2, 1); // um setor
            outb(io + 3, lba as u8);
            outb(io + 4, (lba >> 8) as u8);
            outb(io + 5, (lba >> 16) as u8);
            outb(io + 7, CMD_READ_SECTORS);
            for _ in 0..4 {
                inb(self.bus.control());
            }
            wait(|| inb(self.bus.control()), |s| s & STATUS_DRQ != 0, MAX_POLLS)?;
            for pair in buf.chunks_exact_mut(2) {
                pair.copy_from_slice(&inw(io).to_le_bytes());
            }
            Ok(())
        })
        .map_err(BlockError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;

    #[test_case]
    fn espera_que_nunca_termina_desiste_depois_do_limite_exato() {
        let reads = Cell::new(0u32);
        let result = wait(
            || {
                reads.set(reads.get() + 1);
                STATUS_BSY // o disco nunca sai de ocupado
            },
            |_| true,
            1000,
        );
        assert_eq!(result, Err(AtaError::Timeout));
        assert_eq!(reads.get(), 1000);
    }

    #[test_case]
    fn espera_que_o_criterio_nunca_cumpre_tambem_tem_limite() {
        let reads = Cell::new(0u32);
        let result = wait(
            || {
                reads.set(reads.get() + 1);
                0x50 // pronto, mas sem DRQ
            },
            |s| s & STATUS_DRQ != 0,
            50,
        );
        assert_eq!(result, Err(AtaError::Timeout));
        assert_eq!(reads.get(), 50);
    }

    #[test_case]
    fn espera_termina_assim_que_o_criterio_vale() {
        let reads = Cell::new(0u32);
        let result = wait(
            || {
                reads.set(reads.get() + 1);
                STATUS_DRQ
            },
            |s| s & STATUS_DRQ != 0,
            1000,
        );
        assert_eq!(result, Ok(STATUS_DRQ));
        assert_eq!(reads.get(), 1);
    }

    #[test_case]
    fn espera_ignora_ocupado_e_entrega_o_primeiro_status_pronto() {
        let reads = Cell::new(0u32);
        let result = wait(
            || {
                reads.set(reads.get() + 1);
                if reads.get() < 4 { STATUS_BSY } else { STATUS_DRQ }
            },
            |s| s & STATUS_DRQ != 0,
            1000,
        );
        assert_eq!(result, Ok(STATUS_DRQ));
        assert_eq!(reads.get(), 4);
    }

    #[test_case]
    fn bit_de_erro_com_o_disco_livre_e_erro_do_dispositivo() {
        assert_eq!(wait(|| STATUS_ERR, |_| true, 10), Err(AtaError::DeviceError));
        assert_eq!(wait(|| STATUS_DF, |_| true, 10), Err(AtaError::DeviceError));
        // Com `BSY` ligado, o bit de erro não vale (e a espera estoura).
        assert_eq!(wait(|| STATUS_BSY | STATUS_ERR, |_| true, 10), Err(AtaError::Timeout));
    }

    #[test_case]
    fn espera_com_limite_zero_nao_le_nada() {
        let reads = Cell::new(0u32);
        let result = wait(
            || {
                reads.set(reads.get() + 1);
                0
            },
            |_| true,
            0,
        );
        assert_eq!(result, Err(AtaError::Timeout));
        assert_eq!(reads.get(), 0);
    }

    #[test_case]
    fn erros_do_ata_viram_erros_de_bloco() {
        assert_eq!(BlockError::from(AtaError::NoDevice), BlockError::NoDevice);
        assert_eq!(BlockError::from(AtaError::NotAta), BlockError::NoDevice);
        assert_eq!(BlockError::from(AtaError::Timeout), BlockError::Timeout);
        assert_eq!(BlockError::from(AtaError::DeviceError), BlockError::DeviceError);
    }
}
