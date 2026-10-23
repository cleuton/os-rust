//! Testes de integração do driver ATA (Marco 8), dentro do QEMU.
//!
//! O QEMU de teste (ver `[package.metadata.bootimage]` no `Cargo.toml`) tem,
//! além do disco de boot (primário-mestre):
//!
//! - o disco de dados `disco.img` no primário-escravo;
//! - `corrompido.img` (o mesmo disco com o setor de boot zerado) no
//!   secundário-mestre;
//! - nada no secundário-escravo: a ausência de disco que o driver precisa
//!   tratar sem travar.
//!
//! Nenhum teste depende de relógio. O limite de espera do driver é conferido
//! pelos testes unitários de `src/ata.rs`, que não precisam de disco.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os_rust::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::vec::Vec;
use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use os_rust::ata::{AtaDisk, AtaError, Bus, Drive};
use os_rust::blockdev::{BlockDevice, BlockError, RamDisk, SECTOR_SIZE};
use os_rust::fat::{Fat, FatError, Kind, Node};
use os_rust::fs::{self, Reason, VolumeId};

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

fn disco() -> AtaDisk {
    AtaDisk::detect(Bus::Primary, Drive::Slave).expect("o disco de dados deveria existir")
}

/// Lê um arquivo inteiro pelo leitor FAT sobre `dev`.
fn ler(dev: &dyn BlockDevice, caminho: &[&str]) -> Vec<u8> {
    let fat = Fat::mount(dev).expect("volume valido");
    let comps: Vec<[u8; 11]> = caminho
        .iter()
        .map(|c| {
            let (base, ext) = c.split_once('.').unwrap_or((c, ""));
            let mut nome = [b' '; 11];
            for (i, b) in base.bytes().enumerate() {
                nome[i] = b.to_ascii_uppercase();
            }
            for (i, b) in ext.bytes().enumerate() {
                nome[8 + i] = b.to_ascii_uppercase();
            }
            nome
        })
        .collect();
    let node: Node = fat.lookup(&comps).expect("arquivo existe");
    assert_eq!(node.kind, Kind::File);
    let mut cursor = fat.cursor(&node).unwrap();
    let mut dados = Vec::new();
    let mut pedaco = [0u8; 100];
    loop {
        let n = fat.read(&mut cursor, &mut pedaco).unwrap();
        if n == 0 {
            return dados;
        }
        dados.extend_from_slice(&pedaco[..n]);
    }
}

#[test_case]
fn le_o_setor_de_boot_do_disco_de_dados() {
    let disk = disco();
    assert_eq!(disk.sector_count(), 4200);
    let mut setor = [0u8; SECTOR_SIZE];
    disk.read_sector(0, &mut setor).unwrap();
    // Assinatura de boot e identificação do gerador de imagens.
    assert_eq!(&setor[510..512], &[0x55, 0xAA]);
    assert_eq!(&setor[3..11], b"OSRUST  ");
    assert_eq!(&setor[54..62], b"FAT16   ");
}

#[test_case]
fn le_um_arquivo_conhecido_pelo_leitor_fat_sobre_o_ata() {
    let disk = disco();
    let esperado = include_bytes!("../discos/disco/docs/curto.txt");
    assert_eq!(ler(&disk, &["docs", "curto.txt"]), esperado);
    // O arquivo de dois clusters sai inteiro.
    let longo = include_bytes!("../discos/disco/docs/longo.txt");
    assert!(longo.len() > SECTOR_SIZE);
    assert_eq!(ler(&disk, &["docs", "longo.txt"]), longo);
}

#[test_case]
fn setor_alem_do_fim_do_disco_e_erro() {
    let disk = disco();
    let mut setor = [0u8; SECTOR_SIZE];
    assert_eq!(disk.read_sector(4200, &mut setor), Err(BlockError::OutOfRange));
    assert_eq!(disk.read_sector(u32::MAX, &mut setor), Err(BlockError::OutOfRange));
    // O último setor existente lê normalmente.
    disk.read_sector(4199, &mut setor).unwrap();
}

#[test_case]
fn drive_ausente_e_detectado_sem_travar() {
    // Secundário-escravo: o QEMU de teste não anexa nada aqui.
    assert_eq!(AtaDisk::detect(Bus::Secondary, Drive::Slave).err(), Some(AtaError::NoDevice));
    // E o motivo que o kernel mostra para um volume nessa situação.
    assert_eq!(fs::reason_for(AtaError::NoDevice), Reason::NoDisk);
    // O disco que existe continua funcionando depois da sondagem.
    let mut setor = [0u8; SECTOR_SIZE];
    disco().read_sector(0, &mut setor).unwrap();
}

#[test_case]
fn volume_corrompido_e_recusado_sem_panico() {
    // Secundário-mestre: `corrompido.img`, com o setor de boot zerado.
    let disk = AtaDisk::detect(Bus::Secondary, Drive::Master).expect("a imagem corrompida e um disco ATA valido");
    assert_eq!(disk.sector_count(), 4200);
    assert_eq!(Fat::mount(&disk).err(), Some(FatError::BadBoot("assinatura")));
}

#[test_case]
fn o_kernel_monta_o_disco_de_dados_como_volume() {
    assert_eq!(fs::volume_status(VolumeId::Disco), Ok(()));
    assert_eq!(fs::volume_status(VolumeId::Ram), Ok(()));
}

#[test_case]
fn o_mesmo_leitor_le_o_ramdisk_e_o_disco() {
    // FR-003: o mesmo código (`Fat`) sobre duas fontes de blocos diferentes.
    let ram = RamDisk(include_bytes!(concat!(env!("OUT_DIR"), "/ramdisk.img")));
    let disk = disco();
    for dev in [&ram as &dyn BlockDevice, &disk as &dyn BlockDevice] {
        let fat = Fat::mount(dev).unwrap();
        let raiz = fat.root();
        let mut entradas = 0;
        while fat.read_dir_entry(&raiz, entradas).unwrap().is_some() {
            entradas += 1;
        }
        assert!(entradas >= 2, "a raiz de cada volume tem arquivos e diretorios");
    }
}

// ---------------------------------------------------------------------------
// Escrita (Marco 11)
//
// O QEMU de teste usa `snapshot=on`: o que se grava vai para um arquivo
// temporário e a imagem original nunca muda. Mesmo assim, cada teste lê o setor
// antes e o regrava no fim, para o disco ficar como estava.
// ---------------------------------------------------------------------------

/// Um padrão de 512 bytes que muda com `semente`.
fn padrao(semente: u8) -> [u8; SECTOR_SIZE] {
    let mut setor = [0u8; SECTOR_SIZE];
    for (i, b) in setor.iter_mut().enumerate() {
        *b = (i as u8).wrapping_mul(7).wrapping_add(semente);
    }
    setor
}

fn le(disk: &AtaDisk, lba: u32) -> [u8; SECTOR_SIZE] {
    let mut setor = [0u8; SECTOR_SIZE];
    disk.read_sector(lba, &mut setor).unwrap();
    setor
}

#[test_case]
fn o_disco_diz_que_aceita_escrita() {
    assert!(disco().writable());
}

#[test_case]
fn grava_um_padrao_e_le_de_volta() {
    let disk = disco();
    let lba = 4000;
    let original = le(&disk, lba);
    let novo = padrao(3);
    assert_ne!(original, novo);
    disk.write_sector(lba, &novo).unwrap();
    assert_eq!(le(&disk, lba), novo);
    // Um segundo detect (outro AtaDisk) enxerga o mesmo: nada fica em cache no kernel.
    assert_eq!(le(&disco(), lba), novo);
    disk.write_sector(lba, &original).unwrap();
    assert_eq!(le(&disk, lba), original);
}

#[test_case]
fn grava_o_ultimo_setor_do_disco() {
    let disk = disco();
    let ultimo = disk.sector_count() - 1;
    let original = le(&disk, ultimo);
    let novo = padrao(9);
    disk.write_sector(ultimo, &novo).unwrap();
    assert_eq!(le(&disk, ultimo), novo);
    disk.write_sector(ultimo, &original).unwrap();
    assert_eq!(le(&disk, ultimo), original);
}

#[test_case]
fn gravar_alem_do_fim_e_out_of_range_e_o_disco_segue_funcionando() {
    let disk = disco();
    let dados = padrao(1);
    assert_eq!(disk.write_sector(disk.sector_count(), &dados), Err(BlockError::OutOfRange));
    assert_eq!(disk.write_sector(u32::MAX, &dados), Err(BlockError::OutOfRange));
    // Nenhuma porta foi tocada: o controlador continua respondendo.
    assert_eq!(&le(&disk, 0)[510..512], &[0x55, 0xAA]);
}

#[test_case]
fn gravar_nao_altera_os_vizinhos() {
    let disk = disco();
    let lba = 4100;
    let (antes, meio, depois) = (le(&disk, lba - 1), le(&disk, lba), le(&disk, lba + 1));
    disk.write_sector(lba, &padrao(5)).unwrap();
    assert_eq!(le(&disk, lba - 1), antes);
    assert_eq!(le(&disk, lba + 1), depois);
    disk.write_sector(lba, &meio).unwrap();
    assert_eq!(le(&disk, lba), meio);
}
