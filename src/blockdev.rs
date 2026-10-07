//! Dispositivo de blocos: a menor ideia de "disco" que o resto do kernel precisa.
//!
//! Um dispositivo de blocos é uma sequência de setores de 512 bytes numerados
//! a partir de zero (o número é o endereço de bloco lógico, LBA). A única
//! operação é ler um setor. O leitor de FAT (`fat.rs`) só conhece este trait;
//! por isso o mesmo código lê o ramdisk, que mora dentro do kernel, e o disco
//! ATA, que mora no hardware.
//!
//! Não há cache, leitura de vários setores nem escrita: o marco é somente
//! leitura e o que se lê é pouco.

/// Tamanho de um setor, em bytes.
pub const SECTOR_SIZE: usize = 512;

/// Por que a leitura de um setor falhou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// O número do setor está além do fim do dispositivo.
    OutOfRange,
    /// O dispositivo não existe (barramento sem resposta).
    NoDevice,
    /// O dispositivo não respondeu dentro do limite de espera.
    Timeout,
    /// O dispositivo reportou erro.
    DeviceError,
}

/// Algo de onde se leem setores de 512 bytes.
pub trait BlockDevice {
    /// Quantos setores o dispositivo tem.
    fn sector_count(&self) -> u32;

    /// Copia o setor `lba` para `buf`. Nunca lê fora de `0..sector_count()`:
    /// um `lba` fora do dispositivo devolve `BlockError::OutOfRange`.
    fn read_sector(&self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockError>;
}

/// Um dispositivo de blocos que é uma fatia de bytes em memória: o ramdisk
/// embutido na imagem de boot (`include_bytes!`) e, nos testes, imagens
/// fabricadas. Bytes além do último setor completo são ignorados.
pub struct RamDisk(pub &'static [u8]);

impl BlockDevice for RamDisk {
    fn sector_count(&self) -> u32 {
        (self.0.len() / SECTOR_SIZE) as u32
    }

    fn read_sector(&self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockError> {
        let start = (lba as usize).checked_mul(SECTOR_SIZE).ok_or(BlockError::OutOfRange)?;
        let end = start.checked_add(SECTOR_SIZE).ok_or(BlockError::OutOfRange)?;
        let sector = self.0.get(start..end).ok_or(BlockError::OutOfRange)?;
        buf.copy_from_slice(sector);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static BYTES: [u8; 3 * SECTOR_SIZE + 10] = {
        let mut b = [0u8; 3 * SECTOR_SIZE + 10];
        let mut i = 0;
        while i < b.len() {
            b[i] = (i / SECTOR_SIZE) as u8 + 1;
            i += 1;
        }
        b
    };

    #[test_case]
    fn le_o_primeiro_e_o_ultimo_setor() {
        let disk = RamDisk(&BYTES);
        let mut buf = [0u8; SECTOR_SIZE];
        disk.read_sector(0, &mut buf).unwrap();
        assert!(buf.iter().all(|&b| b == 1));
        disk.read_sector(2, &mut buf).unwrap();
        assert!(buf.iter().all(|&b| b == 3));
    }

    #[test_case]
    fn setor_alem_do_fim_e_erro() {
        let disk = RamDisk(&BYTES);
        let mut buf = [0u8; SECTOR_SIZE];
        assert_eq!(disk.read_sector(3, &mut buf), Err(BlockError::OutOfRange));
        assert_eq!(disk.read_sector(u32::MAX, &mut buf), Err(BlockError::OutOfRange));
    }

    #[test_case]
    fn bytes_que_nao_completam_um_setor_nao_contam() {
        // 3 setores completos mais 10 bytes soltos: são 3 setores.
        assert_eq!(RamDisk(&BYTES).sector_count(), 3);
    }
}
