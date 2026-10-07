//! Gerador de imagens FAT16 para o os-rust.
//!
//! Escreve um volume pequeno (4200 setores de 512 bytes, cerca de 2 MiB) com
//! um layout fixo e simples, o mesmo que o leitor do kernel (`src/fat.rs`)
//! espera:
//!
//! ```text
//! setor 0        boot sector (BPB), termina em 0x55 0xAA
//! setores 1..35  duas cópias da FAT (17 setores cada)
//! setores 35..67 diretório raiz (512 entradas de 32 bytes)
//! setor 67 em    área de dados; o primeiro cluster é o de número 2
//! ```
//!
//! Com **1 setor por cluster** o volume tem 4133 clusters, acima dos 4085 que
//! fazem de um volume FAT16 de verdade (abaixo disso as ferramentas reais o
//! leriam como FAT12). Nomes só no formato 8.3, sempre em maiúsculas.
//!
//! Uso: `Builder::new()`, `dir("docs")`, `file("docs/a.txt", bytes)`,
//! `build()`. Os diretórios pais precisam existir antes dos filhos.

#![no_std]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

/// Bytes por setor.
pub const SECTOR_SIZE: usize = 512;
/// Total de setores do volume gerado.
pub const TOTAL_SECTORS: usize = 4200;
/// Setores de cada cópia da FAT: 17 × 256 entradas = 4352, mais que os 4135
/// (clusters + 2 reservados) que o volume precisa.
const FAT_SECTORS: usize = 17;
/// Cópias da FAT (a segunda é só para que o volume seja válido para
/// ferramentas reais; o leitor do kernel lê a primeira).
const FAT_COUNT: usize = 2;
/// Entradas do diretório raiz.
const ROOT_ENTRIES: usize = 512;
/// Setores ocupados pelo diretório raiz: 512 × 32 / 512.
const ROOT_SECTORS: usize = ROOT_ENTRIES * 32 / SECTOR_SIZE;
/// Primeiro setor da FAT (logo depois do setor reservado de boot).
const FAT_START: usize = 1;
/// Primeiro setor do diretório raiz.
const ROOT_START: usize = FAT_START + FAT_COUNT * FAT_SECTORS;
/// Primeiro setor da área de dados (cluster 2).
const DATA_START: usize = ROOT_START + ROOT_SECTORS;
/// Quantidade de clusters de dados do volume.
pub const CLUSTER_COUNT: usize = TOTAL_SECTORS - DATA_START;

/// Por que uma imagem não pôde ser montada.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Nome fora do formato 8.3 (vazio, longo demais ou com caractere inválido).
    BadName,
    /// O diretório pai do caminho não existe (ou é um arquivo).
    NoParent,
    /// Já existe uma entrada com esse nome no mesmo diretório.
    Duplicate,
    /// O diretório raiz já tem 512 entradas.
    RootFull,
    /// Os dados não cabem nos clusters do volume.
    VolumeFull,
}

/// Uma entrada da árvore: o raiz é o nó 0.
struct Node {
    /// Índice do diretório pai.
    parent: usize,
    /// Nome 8.3 no formato do disco: 8 de nome e 3 de extensão, maiúsculas,
    /// preenchidos com espaços.
    name: [u8; 11],
    is_dir: bool,
    /// Conteúdo (só arquivos).
    data: Vec<u8>,
}

/// Monta, em memória, o conteúdo de um volume FAT16.
pub struct Builder {
    nodes: Vec<Node>,
}

impl Default for Builder {
    fn default() -> Self {
        Builder::new()
    }
}

impl Builder {
    /// Um volume vazio, só com o diretório raiz.
    pub fn new() -> Builder {
        Builder {
            nodes: vec![Node { parent: 0, name: [b' '; 11], is_dir: true, data: Vec::new() }],
        }
    }

    /// Cria um diretório (`"docs"` ou `"docs/sub"`; o pai precisa existir).
    pub fn dir(&mut self, path: &str) -> Result<(), Error> {
        self.add(path, true, &[])
    }

    /// Cria um arquivo com o conteúdo dado (`"docs/a.txt"`; o pai precisa existir).
    pub fn file(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        self.add(path, false, bytes)
    }

    fn add(&mut self, path: &str, is_dir: bool, data: &[u8]) -> Result<(), Error> {
        let mut parent = 0usize;
        let mut parts = path.split('/').peekable();
        loop {
            let part = parts.next().ok_or(Error::BadName)?;
            let name = short_name(part)?;
            if parts.peek().is_some() {
                // Componente intermediário: precisa ser um diretório existente.
                parent = self.find(parent, &name).filter(|&i| self.nodes[i].is_dir).ok_or(Error::NoParent)?;
            } else {
                if self.find(parent, &name).is_some() {
                    return Err(Error::Duplicate);
                }
                if parent == 0 && self.children(0).count() >= ROOT_ENTRIES {
                    return Err(Error::RootFull);
                }
                self.nodes.push(Node { parent, name, is_dir, data: data.to_vec() });
                return Ok(());
            }
        }
    }

    /// Índice do filho de `parent` com esse nome, se existir.
    fn find(&self, parent: usize, name: &[u8; 11]) -> Option<usize> {
        // O raiz (índice 0) é o pai de si mesmo; pula-o para não se achar.
        (1..self.nodes.len()).find(|&i| self.nodes[i].parent == parent && &self.nodes[i].name == name)
    }

    /// Índices dos filhos de um diretório, na ordem de criação.
    fn children(&self, dir: usize) -> impl Iterator<Item = usize> + '_ {
        (1..self.nodes.len()).filter(move |&i| self.nodes[i].parent == dir)
    }

    /// Escreve o volume inteiro.
    pub fn build(self) -> Result<Vec<u8>, Error> {
        // 1. Quantos clusters cada nó ocupa e onde começa. Um diretório guarda
        //    `.`, `..` e os filhos, 16 entradas por setor; um arquivo vazio
        //    não ocupa cluster nenhum (cluster inicial 0).
        let count = self.nodes.len();
        let mut first = vec![0u16; count];
        let mut length = vec![0usize; count];
        let mut next_free = 2usize;
        for i in 1..count {
            let n = &self.nodes[i];
            let clusters = if n.is_dir {
                (2 + self.children(i).count()).div_ceil(SECTOR_SIZE / 32)
            } else {
                n.data.len().div_ceil(SECTOR_SIZE)
            };
            length[i] = clusters;
            if clusters > 0 {
                first[i] = next_free as u16;
                next_free += clusters;
            }
        }
        if next_free - 2 > CLUSTER_COUNT {
            return Err(Error::VolumeFull);
        }

        let mut image = vec![0u8; TOTAL_SECTORS * SECTOR_SIZE];
        write_boot_sector(&mut image[..SECTOR_SIZE]);

        // 2. A FAT: as duas primeiras entradas são reservadas; cada cadeia é
        //    de clusters consecutivos e termina em 0xFFFF.
        let mut fat = vec![0u16; FAT_SECTORS * SECTOR_SIZE / 2];
        fat[0] = 0xFFF8; // media descriptor
        fat[1] = 0xFFFF;
        for i in 1..count {
            for k in 0..length[i] {
                let c = first[i] as usize + k;
                fat[c] = if k + 1 == length[i] { 0xFFFF } else { (c + 1) as u16 };
            }
        }
        for copy in 0..FAT_COUNT {
            let base = (FAT_START + copy * FAT_SECTORS) * SECTOR_SIZE;
            for (k, entry) in fat.iter().enumerate() {
                image[base + 2 * k..base + 2 * k + 2].copy_from_slice(&entry.to_le_bytes());
            }
        }

        // 3. Diretórios e conteúdo dos arquivos.
        for dir in 0..count {
            if !self.nodes[dir].is_dir {
                continue;
            }
            let mut entries: Vec<[u8; 32]> = Vec::new();
            if dir != 0 {
                // `..` aponta para o pai; para a raiz o FAT usa cluster 0.
                let parent = self.nodes[dir].parent;
                entries.push(dir_entry(*b".          ", 0x10, first[dir], 0));
                entries.push(dir_entry(*b"..         ", 0x10, if parent == 0 { 0 } else { first[parent] }, 0));
            }
            for child in self.children(dir) {
                let n = &self.nodes[child];
                let (attr, size) = if n.is_dir { (0x10, 0) } else { (0x20, n.data.len() as u32) };
                entries.push(dir_entry(n.name, attr, first[child], size));
            }
            let base = if dir == 0 { ROOT_START * SECTOR_SIZE } else { (DATA_START + first[dir] as usize - 2) * SECTOR_SIZE };
            for (k, entry) in entries.iter().enumerate() {
                image[base + 32 * k..base + 32 * k + 32].copy_from_slice(entry);
            }
        }
        for i in 1..count {
            let n = &self.nodes[i];
            if !n.is_dir && !n.data.is_empty() {
                let base = (DATA_START + first[i] as usize - 2) * SECTOR_SIZE;
                image[base..base + n.data.len()].copy_from_slice(&n.data);
            }
        }
        Ok(image)
    }
}

/// Converte `"ola.txt"` para o nome do disco: `"OLA     TXT"`.
fn short_name(part: &str) -> Result<[u8; 11], Error> {
    let (base, ext) = match part.split_once('.') {
        Some((b, e)) => (b, e),
        None => (part, ""),
    };
    let valid = |s: &str| s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'~' | b'$'));
    if base.is_empty() || base.len() > 8 || ext.len() > 3 || !valid(base) || !valid(ext) {
        return Err(Error::BadName);
    }
    let mut name = [b' '; 11];
    for (k, c) in base.bytes().enumerate() {
        name[k] = c.to_ascii_uppercase();
    }
    for (k, c) in ext.bytes().enumerate() {
        name[8 + k] = c.to_ascii_uppercase();
    }
    Ok(name)
}

/// Uma entrada de diretório de 32 bytes.
fn dir_entry(name: [u8; 11], attr: u8, first_cluster: u16, size: u32) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(&name);
    e[11] = attr;
    e[26..28].copy_from_slice(&first_cluster.to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    e
}

/// O boot sector: salto, BPB e BPB estendido (FAT16), assinatura `0x55AA`.
fn write_boot_sector(sector: &mut [u8]) {
    sector[0..3].copy_from_slice(&[0xEB, 0x3C, 0x90]); // salto para o código de boot (inexistente)
    sector[3..11].copy_from_slice(b"OSRUST  "); // OEM
    sector[11..13].copy_from_slice(&(SECTOR_SIZE as u16).to_le_bytes()); // bytes por setor
    sector[13] = 1; // setores por cluster
    sector[14..16].copy_from_slice(&1u16.to_le_bytes()); // setores reservados
    sector[16] = FAT_COUNT as u8;
    sector[17..19].copy_from_slice(&(ROOT_ENTRIES as u16).to_le_bytes());
    sector[19..21].copy_from_slice(&(TOTAL_SECTORS as u16).to_le_bytes()); // total de setores (16 bits)
    sector[21] = 0xF8; // media: disco fixo
    sector[22..24].copy_from_slice(&(FAT_SECTORS as u16).to_le_bytes());
    sector[24..26].copy_from_slice(&32u16.to_le_bytes()); // setores por trilha (irrelevante)
    sector[26..28].copy_from_slice(&2u16.to_le_bytes()); // cabeças (irrelevante)
    // 28..32 setores escondidos e 32..36 total de 32 bits ficam zerados.
    sector[36] = 0x80; // número do drive
    sector[38] = 0x29; // BPB estendido presente
    sector[39..43].copy_from_slice(&0x4F53_5254u32.to_le_bytes()); // número de série
    sector[43..54].copy_from_slice(b"NO NAME    "); // rótulo (sem rótulo: o diretório raiz não tem entrada de rótulo)
    sector[54..62].copy_from_slice(b"FAT16   "); // tipo
    sector[510] = 0x55;
    sector[511] = 0xAA;
}

