//! Dispositivo de blocos: a menor ideia de "disco" que o resto do kernel precisa.
//!
//! Um dispositivo de blocos é uma sequência de setores de 512 bytes numerados
//! a partir de zero (o número é o endereço de bloco lógico, LBA). A única
//! operação é ler um setor. O leitor de FAT (`fat.rs`) só conhece este trait;
//! por isso o mesmo código lê o ramdisk, que mora dentro do kernel, e o disco
//! ATA, que mora no hardware.
//!
//! Não há cache nem leitura de vários setores. Escrever um setor é opcional:
//! por padrão um dispositivo recusa (`BlockError::ReadOnly`), e só o ramdisk
//! gravável (`WritableRam`) aceita. Assim o ATA e as imagens embutidas
//! continuam somente leitura sem mudar uma linha.

use alloc::vec::Vec;
use spin::Mutex;

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
    /// O dispositivo não aceita escrita.
    ReadOnly,
}

/// Algo de onde se leem setores de 512 bytes.
pub trait BlockDevice {
    /// Quantos setores o dispositivo tem.
    fn sector_count(&self) -> u32;

    /// Copia o setor `lba` para `buf`. Nunca lê fora de `0..sector_count()`:
    /// um `lba` fora do dispositivo devolve `BlockError::OutOfRange`.
    fn read_sector(&self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockError>;

    /// Grava `buf` no setor `lba`. O padrão recusa: só quem aceita escrita
    /// (o ramdisk gravável) a implementa.
    fn write_sector(&self, _lba: u32, _buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockError> {
        Err(BlockError::ReadOnly)
    }

    /// Se o dispositivo aceita `write_sector`. Existe separado da escrita para
    /// o sistema de arquivos recusar uma operação **antes** de tentar gravar.
    fn writable(&self) -> bool {
        false
    }
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

/// O ramdisk gravável: uma cópia, no heap, da imagem embutida na imagem de
/// boot. A imagem original nunca é escrita; reiniciar o kernel recopia tudo.
/// O `Mutex` existe porque `write_sector` recebe `&self` (o dispositivo mora
/// num `static`); o lock só dura a cópia de um setor.
pub struct WritableRam {
    data: Mutex<Vec<u8>>,
}

impl WritableRam {
    /// Copia `image` para a memória gravável.
    pub fn new(image: &[u8]) -> WritableRam {
        WritableRam { data: Mutex::new(image.to_vec()) }
    }

    /// Volta ao conteúdo de `image` (que deve ter o mesmo tamanho da imagem
    /// de origem). Não realoca.
    pub fn reset(&self, image: &[u8]) {
        let mut data = self.data.lock();
        if data.len() == image.len() {
            data.copy_from_slice(image);
        }
    }

    fn range(lba: u32) -> Result<core::ops::Range<usize>, BlockError> {
        let start = (lba as usize).checked_mul(SECTOR_SIZE).ok_or(BlockError::OutOfRange)?;
        let end = start.checked_add(SECTOR_SIZE).ok_or(BlockError::OutOfRange)?;
        Ok(start..end)
    }
}

impl BlockDevice for WritableRam {
    fn sector_count(&self) -> u32 {
        (self.data.lock().len() / SECTOR_SIZE) as u32
    }

    fn read_sector(&self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockError> {
        let data = self.data.lock();
        let sector = data.get(Self::range(lba)?).ok_or(BlockError::OutOfRange)?;
        buf.copy_from_slice(sector);
        Ok(())
    }

    fn write_sector(&self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockError> {
        let mut data = self.data.lock();
        let sector = data.get_mut(Self::range(lba)?).ok_or(BlockError::OutOfRange)?;
        sector.copy_from_slice(buf);
        Ok(())
    }

    fn writable(&self) -> bool {
        true
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

    #[test_case]
    fn ramdisk_gravavel_escreve_e_le_de_volta() {
        let ram = WritableRam::new(&BYTES);
        assert!(ram.writable());
        assert_eq!(ram.sector_count(), 3);
        let novo = [0xABu8; SECTOR_SIZE];
        ram.write_sector(1, &novo).unwrap();
        let mut buf = [0u8; SECTOR_SIZE];
        ram.read_sector(1, &mut buf).unwrap();
        assert_eq!(buf, novo);
        // Os vizinhos não mudam.
        ram.read_sector(0, &mut buf).unwrap();
        assert!(buf.iter().all(|&b| b == 1));
    }

    #[test_case]
    fn escrita_fora_do_fim_e_erro() {
        let ram = WritableRam::new(&BYTES);
        let novo = [0u8; SECTOR_SIZE];
        assert_eq!(ram.write_sector(3, &novo), Err(BlockError::OutOfRange));
        assert_eq!(ram.write_sector(u32::MAX, &novo), Err(BlockError::OutOfRange));
    }

    #[test_case]
    fn dispositivo_somente_leitura_recusa_escrita() {
        let disk = RamDisk(&BYTES);
        assert!(!disk.writable());
        assert_eq!(disk.write_sector(0, &[0u8; SECTOR_SIZE]), Err(BlockError::ReadOnly));
    }

    #[test_case]
    fn reset_restaura_o_conteudo_original() {
        let ram = WritableRam::new(&BYTES);
        ram.write_sector(0, &[9u8; SECTOR_SIZE]).unwrap();
        ram.reset(&BYTES);
        let mut buf = [0u8; SECTOR_SIZE];
        ram.read_sector(0, &mut buf).unwrap();
        assert!(buf.iter().all(|&b| b == 1));
    }
}
