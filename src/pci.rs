//! O barramento PCI, só o necessário para achar uma placa de rede.
//!
//! O PCI dá a cada dispositivo um espaço de configuração de 256 bytes,
//! endereçado por barramento (`bus`, 0 a 255), dispositivo (`dev`, 0 a 31) e
//! função (`func`, 0 a 7). O "mecanismo 1" tem duas portas de E/S: escreve-se o
//! endereço em `0xCF8` e lê-se (ou escreve-se) o dado em `0xCFC`. O primeiro
//! registrador de toda função guarda o fabricante (`vendor`) e o modelo
//! (`device`); `0xFFFF` de fabricante quer dizer "não há nada aqui".
//!
//! A varredura é feita uma vez, no boot, sem alocar, e tem limite fixo de
//! leituras: um barramento que se comporte mal não prende o kernel.

use spin::Mutex;
use x86_64::instructions::port::Port;

/// Porta onde se escreve o endereço do registrador de configuração.
const CONFIG_ADDRESS: u16 = 0xCF8;
/// Porta de onde se lê (ou onde se escreve) o dado do registrador.
const CONFIG_DATA: u16 = 0xCFC;

/// Registrador 0: `vendor` (16 bits baixos) e `device` (16 bits altos).
const REG_ID: u8 = 0x00;
/// Registrador 1: comando (16 bits baixos) e status (16 bits altos).
const REG_COMMAND: u8 = 0x04;
/// Registrador 3: o byte 2 é o tipo de cabeçalho; o bit 7 diz que o
/// dispositivo tem várias funções.
const REG_HEADER: u8 = 0x0C;
/// Registrador do primeiro BAR (endereço base de E/S ou de memória).
const REG_BAR0: u8 = 0x10;

/// Bit do comando que liga o acesso às portas de E/S do dispositivo.
const COMMAND_IO_SPACE: u32 = 1 << 0;
/// Bit do comando que deixa o dispositivo fazer DMA (ser "mestre" do barramento).
const COMMAND_BUS_MASTER: u32 = 1 << 2;

/// Limite de leituras de configuração numa varredura. A varredura completa
/// lê a função 0 de 256 × 32 dispositivos (8 192 leituras) e, de cada
/// dispositivo com várias funções, mais 7: o limite cobre o pior caso com
/// folga e pára uma varredura que não termina.
pub const MAX_PCI_READS: usize = 8192 * 8;

/// A configuração do PCI usa duas portas que precisam ser usadas em par.
static CONFIG_LOCK: Mutex<()> = Mutex::new(());

/// Um dispositivo PCI achado pela varredura.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciDevice {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
}

fn address(bus: u8, dev: u8, func: u8, reg: u8) -> u32 {
    0x8000_0000
        | (bus as u32) << 16
        | (dev as u32) << 11
        | (func as u32) << 8
        | (reg as u32 & 0xFC)
}

/// Lê o registrador `reg` (múltiplo de 4) da função indicada.
fn config_read(bus: u8, dev: u8, func: u8, reg: u8) -> u32 {
    let _guard = CONFIG_LOCK.lock();
    // SAFETY: `0xCF8` e `0xCFC` são as portas do mecanismo 1 de configuração
    // do PCI; escrever um endereço e ler o dado não mexe em memória do
    // kernel, e o par de acessos fica sob `CONFIG_LOCK`.
    unsafe {
        Port::<u32>::new(CONFIG_ADDRESS).write(address(bus, dev, func, reg));
        Port::<u32>::new(CONFIG_DATA).read()
    }
}

fn config_write(bus: u8, dev: u8, func: u8, reg: u8, value: u32) {
    let _guard = CONFIG_LOCK.lock();
    // SAFETY: mesmas portas e mesma trava de `config_read`; o chamador só
    // escreve o registrador de comando do dispositivo que ele mesmo achou.
    unsafe {
        Port::<u32>::new(CONFIG_ADDRESS).write(address(bus, dev, func, reg));
        Port::<u32>::new(CONFIG_DATA).write(value);
    }
}

/// Procura o primeiro dispositivo com este `vendor` e `device`. Lê a função 0
/// de cada dispositivo e só varre as funções 1 a 7 quando o cabeçalho declara
/// várias funções. Devolve `None` se não achar ou se esgotar `MAX_PCI_READS`.
pub fn find(vendor: u16, device: u16) -> Option<PciDevice> {
    let mut reads = 0usize;
    for bus in 0..=255u8 {
        for dev in 0..32u8 {
            reads += 1;
            if reads > MAX_PCI_READS {
                return None;
            }
            let id = config_read(bus, dev, 0, REG_ID);
            if id & 0xFFFF == 0xFFFF {
                continue;
            }
            if id == ((device as u32) << 16 | vendor as u32) {
                return Some(PciDevice { bus, dev, func: 0 });
            }
            reads += 1;
            let multifunction = (config_read(bus, dev, 0, REG_HEADER) >> 16) & 0x80 != 0;
            if !multifunction {
                continue;
            }
            for func in 1..8u8 {
                reads += 1;
                if reads > MAX_PCI_READS {
                    return None;
                }
                let id = config_read(bus, dev, func, REG_ID);
                if id == ((device as u32) << 16 | vendor as u32) {
                    return Some(PciDevice { bus, dev, func });
                }
            }
        }
    }
    None
}

/// Liga o acesso às portas de E/S e o DMA (bus mastering) do dispositivo.
pub fn enable_io_and_bus_master(device: &PciDevice) {
    let PciDevice { bus, dev, func } = *device;
    let command = config_read(bus, dev, func, REG_COMMAND);
    // Escrever o registrador inteiro devolve também o status (16 bits altos);
    // os bits de status são "escreva 1 para limpar", então zera-se a metade alta.
    config_write(bus, dev, func, REG_COMMAND, (command & 0xFFFF) | COMMAND_IO_SPACE | COMMAND_BUS_MASTER);
}

/// A porta base de E/S do primeiro BAR, ou `None` se ele não é de E/S (o bit 0
/// de um BAR de E/S é 1) ou vale zero.
pub fn bar0_io_port(device: &PciDevice) -> Option<u16> {
    let bar = config_read(device.bus, device.dev, device.func, REG_BAR0);
    if bar & 1 == 0 {
        return None;
    }
    let port = (bar & !0x3) as u16;
    (port != 0).then_some(port)
}
