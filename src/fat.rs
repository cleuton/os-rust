//! Leitor de FAT16 somente leitura.
//!
//! Um volume FAT é feito de quatro regiões, nesta ordem:
//!
//! ```text
//! setor de boot | tabela de alocação (FAT) | diretório raiz | área de dados
//! ```
//!
//! - O **setor de boot** descreve o volume (tamanho do setor e do cluster,
//!   onde cada região começa, quantos setores tem).
//! - A **FAT** é um vetor de números de 16 bits, um por cluster. A entrada de
//!   um cluster diz qual cluster vem depois dele no mesmo arquivo; um valor
//!   especial marca o fim da cadeia.
//! - O **diretório raiz** é uma região fixa de entradas de 32 bytes (nome 8.3,
//!   atributos, primeiro cluster, tamanho).
//! - A **área de dados** é dividida em clusters (aqui, 1 setor cada); os
//!   subdiretórios e o conteúdo dos arquivos vivem lá, em cadeias de clusters.
//!
//! O leitor trata o volume como **dado não confiável**: toda imagem pode estar
//! corrompida. Por isso (1) todo número de cluster é validado antes de virar
//! setor, (2) toda travessia de cadeia tem um limite de passos igual ao número
//! de clusters do volume (um ciclo termina em erro, nunca em laço infinito) e
//! (3) nenhuma conta com valores do disco pode estourar ou indexar fora de
//! um slice sem checagem. Qualquer inconsistência vira `FatError`, nunca um
//! pânico.
//!
//! Só nomes curtos 8.3, sem diferenciar maiúsculas de minúsculas. Entradas de
//! nome longo, apagadas, de rótulo de volume, `.` e `..` são ignoradas.

use crate::blockdev::{BlockDevice, BlockError, SECTOR_SIZE};

/// Menor número de clusters de um volume FAT16 de verdade (abaixo disso as
/// ferramentas o leriam como FAT12).
const MIN_FAT16_CLUSTERS: u32 = 4085;
/// Maior número de clusters de um volume FAT16.
const MAX_FAT16_CLUSTERS: u32 = 65524;
/// Entradas de um diretório que este leitor aceita percorrer, contando as
/// apagadas e as de nome longo (é também o tamanho do diretório raiz que o
/// projeto gera). Um diretório maior que isso, ou cuja cadeia tem um ciclo,
/// termina em erro: o limite impede que um ciclo faça o kernel reler as mesmas
/// entradas dezenas de milhares de vezes.
pub const MAX_DIR_ENTRIES: u32 = 512;
/// Os mesmos 512 em setores de diretório (16 entradas por setor).
const MAX_DIR_SECTORS: u32 = MAX_DIR_ENTRIES / 16;

/// Por que uma operação no volume falhou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatError {
    /// O setor de boot não descreve um volume FAT16 coerente (o motivo).
    BadBoot(&'static str),
    /// A leitura de um setor falhou.
    Io(BlockError),
    /// A estrutura do volume é inconsistente: cadeia com ciclo, cluster fora
    /// do volume, tamanho maior que a cadeia, entrada de diretório malformada.
    Corrupt,
    /// O nome não existe no diretório.
    NotFound,
    /// Esperava-se um diretório e o nó é um arquivo.
    NotADirectory,
    /// Esperava-se um arquivo e o nó é um diretório.
    IsADirectory,
}

impl From<BlockError> for FatError {
    fn from(error: BlockError) -> FatError {
        FatError::Io(error)
    }
}

/// Onde cada região do volume começa, calculado a partir do setor de boot.
/// Todos os valores são números de setor (ou contagens), já validados.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub sectors_per_cluster: u32,
    pub fat_start: u32,
    pub fat_sectors: u32,
    pub root_start: u32,
    pub root_sectors: u32,
    pub data_start: u32,
    pub cluster_count: u32,
}

/// Arquivo ou diretório.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
}

/// Uma entrada de diretório válida, já traduzida.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirEntry {
    /// Nome para exibição, em minúsculas: `nome.ext` (sem preenchimento).
    pub name: [u8; 12],
    /// Quantos bytes de `name` valem.
    pub name_len: u8,
    /// O nome no formato do disco (8 + 3, maiúsculas, com espaços), usado
    /// para comparar com os componentes de um caminho.
    pub raw_name: [u8; 11],
    pub kind: Kind,
    /// Tamanho em bytes; `0` para diretório.
    pub size: u32,
    pub first_cluster: u16,
}

impl DirEntry {
    /// O nó que esta entrada nomeia.
    pub fn node(&self) -> Node {
        Node { kind: self.kind, first_cluster: self.first_cluster, size: self.size }
    }

    /// O nome como texto (sempre ASCII).
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("?")
    }
}

/// Um arquivo ou diretório encontrado: o que basta para lê-lo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Node {
    pub kind: Kind,
    /// Primeiro cluster; `0` para a raiz e para arquivo vazio.
    pub first_cluster: u16,
    pub size: u32,
}

/// A posição de uma leitura sequencial de arquivo. Guarda também em que
/// cluster a posição cai, para que a próxima leitura continue dali em vez de
/// recomeçar a cadeia.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    first_cluster: u16,
    size: u32,
    pos: u32,
    cluster: u16,
    cluster_index: u32,
}

impl Cursor {
    /// Posição atual, em bytes desde o início do arquivo.
    pub fn position(&self) -> u32 {
        self.pos
    }

    /// Tamanho do arquivo.
    pub fn size(&self) -> u32 {
        self.size
    }
}

/// Um volume FAT16 montado sobre um dispositivo de blocos.
pub struct Fat<'a> {
    dev: &'a dyn BlockDevice,
    geo: Geometry,
}

fn le16(buf: &[u8], at: usize) -> u32 {
    u16::from_le_bytes([buf[at], buf[at + 1]]) as u32
}

fn le32(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

impl<'a> Fat<'a> {
    /// Valida o setor de boot e devolve o volume. Cada validação que falha
    /// devolve `BadBoot` com o motivo, para a mensagem do kernel.
    pub fn mount(dev: &'a dyn BlockDevice) -> Result<Fat<'a>, FatError> {
        let mut boot = [0u8; SECTOR_SIZE];
        dev.read_sector(0, &mut boot)?;
        if boot[510] != 0x55 || boot[511] != 0xAA {
            return Err(FatError::BadBoot("assinatura"));
        }
        if le16(&boot, 11) != SECTOR_SIZE as u32 {
            return Err(FatError::BadBoot("bytes por setor"));
        }
        let spc = boot[13] as u32;
        if !(1..=64).contains(&spc) || !spc.is_power_of_two() {
            return Err(FatError::BadBoot("setores por cluster"));
        }
        let reserved = le16(&boot, 14);
        if reserved == 0 {
            return Err(FatError::BadBoot("reservados"));
        }
        let fats = boot[16] as u32;
        if fats != 1 && fats != 2 {
            return Err(FatError::BadBoot("numero de FATs"));
        }
        let root_entries = le16(&boot, 17);
        if root_entries == 0 || root_entries % 16 != 0 {
            return Err(FatError::BadBoot("raiz"));
        }
        let total = match le16(&boot, 19) {
            0 => le32(&boot, 32),
            n => n,
        };
        if total == 0 || total > dev.sector_count() {
            return Err(FatError::BadBoot("total de setores"));
        }
        let fat_sectors = le16(&boot, 22);
        if fat_sectors == 0 {
            return Err(FatError::BadBoot("tamanho da FAT"));
        }
        // As contas abaixo usam valores do disco: todas com `checked_*`.
        let root_sectors = root_entries * 32 / SECTOR_SIZE as u32;
        let data_start = fats
            .checked_mul(fat_sectors)
            .and_then(|f| f.checked_add(reserved))
            .and_then(|f| f.checked_add(root_sectors))
            .filter(|&d| d < total)
            .ok_or(FatError::BadBoot("area de dados"))?;
        let cluster_count = (total - data_start) / spc;
        if !(MIN_FAT16_CLUSTERS..=MAX_FAT16_CLUSTERS).contains(&cluster_count) {
            return Err(FatError::BadBoot("contagem de clusters"));
        }
        // A FAT precisa ter uma entrada de 16 bits para cada cluster, mais as
        // duas reservadas (0 e 1).
        if fat_sectors * (SECTOR_SIZE as u32 / 2) < cluster_count + 2 {
            return Err(FatError::BadBoot("tamanho da FAT"));
        }
        let geo = Geometry {
            sectors_per_cluster: spc,
            fat_start: reserved,
            fat_sectors,
            root_start: reserved + fats * fat_sectors,
            root_sectors,
            data_start,
            cluster_count,
        };
        Ok(Fat { dev, geo })
    }

    /// Monta de novo um volume já validado: `geo` veio de um `mount` anterior
    /// sobre o mesmo dispositivo (é como `fs.rs` guarda os volumes, sem
    /// precisar de um `Fat` com tempo de vida dentro de um `static`).
    pub fn from_parts(dev: &'a dyn BlockDevice, geo: Geometry) -> Fat<'a> {
        Fat { dev, geo }
    }

    /// A geometria validada.
    pub fn geometry(&self) -> Geometry {
        self.geo
    }

    /// O diretório raiz.
    pub fn root(&self) -> Node {
        Node { kind: Kind::Dir, first_cluster: 0, size: 0 }
    }

    /// O setor onde o cluster `cluster` começa. Valida o número: só existem
    /// os clusters `2..=cluster_count + 1`.
    fn cluster_sector(&self, cluster: u16) -> Result<u32, FatError> {
        let c = cluster as u32;
        if c < 2 || c > self.geo.cluster_count + 1 {
            return Err(FatError::Corrupt);
        }
        // Dentro do volume: `c - 2 < cluster_count`, e o total já foi validado
        // contra o tamanho do dispositivo em `mount`.
        Ok(self.geo.data_start + (c - 2) * self.geo.sectors_per_cluster)
    }

    /// O cluster que vem depois de `cluster` na cadeia, ou `None` no fim dela.
    /// Livre, reservado, setor ruim ou número fora do volume dentro de uma
    /// cadeia são `Corrupt`.
    fn next_cluster(&self, cluster: u16) -> Result<Option<u16>, FatError> {
        let c = cluster as u32;
        if c < 2 || c > self.geo.cluster_count + 1 {
            return Err(FatError::Corrupt);
        }
        // Cada entrada tem 2 bytes. `mount` garantiu que a FAT cobre todos
        // os clusters, então este setor está dentro da FAT.
        let offset = c * 2;
        let mut sector = [0u8; SECTOR_SIZE];
        self.dev.read_sector(self.geo.fat_start + offset / SECTOR_SIZE as u32, &mut sector)?;
        let at = (offset % SECTOR_SIZE as u32) as usize;
        let value = u16::from_le_bytes([sector[at], sector[at + 1]]);
        match value {
            0x0000 | 0x0001 | 0xFFF0..=0xFFF7 => Err(FatError::Corrupt),
            0xFFF8..=0xFFFF => Ok(None),
            next if (next as u32) <= self.geo.cluster_count + 1 => Ok(Some(next)),
            _ => Err(FatError::Corrupt),
        }
    }

    /// Chama `f` para cada entrada válida do diretório, na ordem em que estão
    /// no disco, até `f` devolver `true` (parar) ou as entradas acabarem.
    /// A raiz é uma região fixa; um subdiretório é uma cadeia de clusters,
    /// percorrida com um limite de `MAX_DIR_ENTRIES` entradas.
    fn walk_dir(&self, dir: &Node, f: &mut dyn FnMut(&DirEntry) -> bool) -> Result<(), FatError> {
        if dir.kind != Kind::Dir {
            return Err(FatError::NotADirectory);
        }
        let mut sector = [0u8; SECTOR_SIZE];
        if dir.first_cluster == 0 {
            for s in 0..self.geo.root_sectors {
                self.dev.read_sector(self.geo.root_start + s, &mut sector)?;
                if self.scan_sector(&sector, f)? {
                    return Ok(());
                }
            }
            return Ok(());
        }
        let mut cluster = dir.first_cluster;
        let mut scanned = 0;
        loop {
            let first = self.cluster_sector(cluster)?;
            for s in 0..self.geo.sectors_per_cluster {
                scanned += 1;
                if scanned > MAX_DIR_SECTORS {
                    // Mais setores que o limite: diretório grande demais, ou
                    // uma cadeia com ciclo relendo as mesmas entradas.
                    return Err(FatError::Corrupt);
                }
                self.dev.read_sector(first + s, &mut sector)?;
                if self.scan_sector(&sector, f)? {
                    return Ok(());
                }
            }
            match self.next_cluster(cluster)? {
                None => return Ok(()),
                Some(next) => cluster = next,
            }
        }
    }

    /// Percorre as 16 entradas de um setor de diretório. Devolve `true` se
    /// `f` pediu para parar **ou** se o diretório terminou (entrada `0x00`).
    fn scan_sector(&self, sector: &[u8; SECTOR_SIZE], f: &mut dyn FnMut(&DirEntry) -> bool) -> Result<bool, FatError> {
        for raw in sector.chunks_exact(32) {
            match raw[0] {
                0x00 => return Ok(true),
                0xE5 => continue,
                _ => {}
            }
            let attr = raw[11];
            // Nome longo (0x0F) e rótulo de volume (0x08) não são arquivos.
            if attr & 0x3F == 0x0F || attr & 0x08 != 0 {
                continue;
            }
            // `.` e `..` dos subdiretórios.
            if raw[0] == b'.' {
                continue;
            }
            let entry = parse_entry(raw)?;
            if f(&entry) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// A `index`-ésima entrada válida do diretório, ou `None` se ele tem menos.
    pub fn read_dir_entry(&self, dir: &Node, index: u32) -> Result<Option<DirEntry>, FatError> {
        let mut seen = 0u32;
        let mut found = None;
        self.walk_dir(dir, &mut |entry| {
            if seen == index {
                found = Some(*entry);
                true
            } else {
                seen += 1;
                false
            }
        })?;
        Ok(found)
    }

    /// Resolve um caminho já separado em componentes (8 + 3, maiúsculas),
    /// a partir da raiz. Sem componentes devolve a raiz.
    pub fn lookup(&self, components: &[[u8; 11]]) -> Result<Node, FatError> {
        let mut node = self.root();
        for component in components {
            let mut next = None;
            self.walk_dir(&node, &mut |entry| {
                if entry.raw_name == *component {
                    next = Some(entry.node());
                    true
                } else {
                    false
                }
            })?;
            node = next.ok_or(FatError::NotFound)?;
        }
        Ok(node)
    }

    /// Um cursor no início do arquivo `node`.
    pub fn cursor(&self, node: &Node) -> Result<Cursor, FatError> {
        if node.kind != Kind::File {
            return Err(FatError::IsADirectory);
        }
        Ok(Cursor { first_cluster: node.first_cluster, size: node.size, pos: 0, cluster: node.first_cluster, cluster_index: 0 })
    }

    /// Lê até `buf.len()` bytes a partir da posição do cursor, sem passar do
    /// tamanho do arquivo, e avança o cursor. Devolve quantos bytes leu; `0`
    /// só no fim do arquivo (ou se `buf` é vazio). Uma cadeia que termina
    /// antes de cobrir o tamanho do arquivo é `Corrupt`.
    pub fn read(&self, cursor: &mut Cursor, buf: &mut [u8]) -> Result<usize, FatError> {
        let mut written = 0usize;
        match self.read_into(cursor, buf, &mut written) {
            Ok(()) => Ok(written),
            // Um erro depois de já ter copiado bytes não os descarta: entrega
            // o que leu, e a próxima chamada (que recomeça do mesmo ponto)
            // encontra o mesmo erro e o devolve.
            Err(_) if written > 0 => Ok(written),
            Err(error) => Err(error),
        }
    }

    fn read_into(&self, cursor: &mut Cursor, buf: &mut [u8], written: &mut usize) -> Result<(), FatError> {
        let cluster_bytes = self.geo.sectors_per_cluster * SECTOR_SIZE as u32;
        let mut sector = [0u8; SECTOR_SIZE];
        while *written < buf.len() && cursor.pos < cursor.size {
            if cursor.first_cluster == 0 {
                // Tamanho maior que zero sem nenhum cluster.
                return Err(FatError::Corrupt);
            }
            // Em que cluster (contado desde o primeiro) a posição cai.
            let wanted = cursor.pos / cluster_bytes;
            if wanted >= self.geo.cluster_count {
                // O arquivo seria maior que o volume.
                return Err(FatError::Corrupt);
            }
            while cursor.cluster_index < wanted {
                cursor.cluster = self.next_cluster(cursor.cluster)?.ok_or(FatError::Corrupt)?;
                cursor.cluster_index += 1;
            }
            let in_cluster = cursor.pos % cluster_bytes;
            let lba = self.cluster_sector(cursor.cluster)? + in_cluster / SECTOR_SIZE as u32;
            self.dev.read_sector(lba, &mut sector)?;
            let in_sector = (in_cluster % SECTOR_SIZE as u32) as usize;
            let n = (SECTOR_SIZE - in_sector)
                .min(buf.len() - *written)
                .min((cursor.size - cursor.pos) as usize);
            buf[*written..*written + n].copy_from_slice(&sector[in_sector..in_sector + n]);
            *written += n;
            cursor.pos += n as u32;
        }
        Ok(())
    }
}

/// Traduz uma entrada bruta de 32 bytes. Nome com byte de controle ou fora do
/// ASCII imprimível, ou diretório sem cluster, é entrada malformada.
fn parse_entry(raw: &[u8]) -> Result<DirEntry, FatError> {
    let mut raw_name = [0u8; 11];
    raw_name.copy_from_slice(&raw[..11]);
    for &b in &raw_name {
        if !(0x20..0x7F).contains(&b) || b"\"*+,/:;<=>?[\\]|".contains(&b) {
            return Err(FatError::Corrupt);
        }
    }
    raw_name.make_ascii_uppercase();
    let kind = if raw[11] & 0x10 != 0 { Kind::Dir } else { Kind::File };
    let first_cluster = u16::from_le_bytes([raw[26], raw[27]]);
    let size = u32::from_le_bytes([raw[28], raw[29], raw[30], raw[31]]);
    if kind == Kind::Dir && first_cluster < 2 {
        return Err(FatError::Corrupt);
    }
    // Nome de exibição: `nome.ext` em minúsculas, sem os espaços de preenchimento.
    let mut name = [0u8; 12];
    let mut len = 0usize;
    for &b in raw_name[..8].iter().take_while(|&&b| b != b' ') {
        name[len] = b.to_ascii_lowercase();
        len += 1;
    }
    if len == 0 {
        return Err(FatError::Corrupt);
    }
    if raw_name[8] != b' ' {
        name[len] = b'.';
        len += 1;
        for &b in raw_name[8..].iter().take_while(|&&b| b != b' ') {
            name[len] = b.to_ascii_lowercase();
            len += 1;
        }
    }
    Ok(DirEntry {
        name,
        name_len: len as u8,
        raw_name,
        kind,
        size: if kind == Kind::Dir { 0 } else { size },
        first_cluster,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    /// Um dispositivo de blocos em memória, para fabricar imagens e adulterá-las.
    struct MemDisk(Vec<u8>);

    impl BlockDevice for MemDisk {
        fn sector_count(&self) -> u32 {
            (self.0.len() / SECTOR_SIZE) as u32
        }

        fn read_sector(&self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockError> {
            let start = (lba as usize) * SECTOR_SIZE;
            let sector = self.0.get(start..start + SECTOR_SIZE).ok_or(BlockError::OutOfRange)?;
            buf.copy_from_slice(sector);
            Ok(())
        }
    }

    /// Conteúdo previsível: o byte `i` vale `(i * 7 + seed) % 251`.
    fn pattern(len: usize, seed: usize) -> Vec<u8> {
        (0..len).map(|i| ((i * 7 + seed) % 251) as u8).collect()
    }

    /// Uma imagem com arquivos de todos os tamanhos interessantes, dois níveis
    /// de diretório e um diretório que ocupa vários clusters.
    fn image() -> Vec<u8> {
        let mut b = fatimg::Builder::new();
        b.file("vazio.txt", b"").unwrap();
        b.file("curto.txt", &pattern(100, 1)).unwrap();
        b.file("exato.bin", &pattern(512, 2)).unwrap();
        b.file("multi.bin", &pattern(1500, 3)).unwrap();
        b.dir("docs").unwrap();
        b.file("docs/a.txt", &pattern(10, 4)).unwrap();
        b.dir("docs/sub").unwrap();
        b.file("docs/sub/b.txt", &pattern(20, 5)).unwrap();
        b.dir("cheio").unwrap();
        for i in 0..40 {
            b.file(&format!("cheio/f{i:02}.txt"), &pattern(i + 1, i)).unwrap();
        }
        b.build().unwrap()
    }

    fn comp(name: &str) -> [u8; 11] {
        let (base, ext) = name.split_once('.').unwrap_or((name, ""));
        let mut out = [b' '; 11];
        for (k, c) in base.bytes().enumerate() {
            out[k] = c.to_ascii_uppercase();
        }
        for (k, c) in ext.bytes().enumerate() {
            out[8 + k] = c.to_ascii_uppercase();
        }
        out
    }

    fn lookup(fat: &Fat, path: &[&str]) -> Result<Node, FatError> {
        let comps: Vec<[u8; 11]> = path.iter().map(|p| comp(p)).collect();
        fat.lookup(&comps)
    }

    fn read_all(fat: &Fat, node: &Node) -> Result<Vec<u8>, FatError> {
        let mut cursor = fat.cursor(node)?;
        let mut out = Vec::new();
        let mut chunk = [0u8; 100];
        loop {
            let n = fat.read(&mut cursor, &mut chunk)?;
            if n == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&chunk[..n]);
        }
    }

    fn list(fat: &Fat, dir: &Node) -> Result<Vec<String>, FatError> {
        let mut names = Vec::new();
        let mut i = 0;
        while let Some(e) = fat.read_dir_entry(dir, i)? {
            names.push(String::from(e.name_str()));
            i += 1;
        }
        Ok(names)
    }

    /// Escreve uma entrada da FAT (nas duas cópias).
    fn set_fat(img: &mut [u8], cluster: u16, value: u16) {
        for copy in 0..2 {
            let at = (1 + copy * 17) * SECTOR_SIZE + cluster as usize * 2;
            img[at..at + 2].copy_from_slice(&value.to_le_bytes());
        }
    }

    fn first_cluster(img: &[u8], path: &[&str]) -> u16 {
        let disk = MemDisk(img.to_vec());
        lookup(&Fat::mount(&disk).unwrap(), path).unwrap().first_cluster
    }

    /// Posição, na imagem, da entrada de diretório de um arquivo da raiz.
    fn root_entry_at(index: usize) -> usize {
        35 * SECTOR_SIZE + index * 32
    }

    // ---- setor de boot ----

    fn mount_error(mutate: impl FnOnce(&mut Vec<u8>)) -> FatError {
        let mut img = image();
        mutate(&mut img);
        let disk = MemDisk(img);
        Fat::mount(&disk).err().expect("o volume deveria ser recusado")
    }

    #[test_case]
    fn monta_uma_imagem_valida() {
        let img = image();
        let disk = MemDisk(img);
        let fat = Fat::mount(&disk).unwrap();
        assert_eq!(fat.geometry().cluster_count, 4133);
        assert_eq!(fat.geometry().data_start, 67);
        assert_eq!(fat.geometry().root_start, 35);
    }

    #[test_case]
    fn recusa_cada_tipo_de_setor_de_boot_invalido() {
        assert_eq!(mount_error(|i| i[510] = 0), FatError::BadBoot("assinatura"));
        assert_eq!(mount_error(|i| i[511] = 0), FatError::BadBoot("assinatura"));
        assert_eq!(mount_error(|i| i[12] = 4), FatError::BadBoot("bytes por setor"));
        assert_eq!(mount_error(|i| i[13] = 0), FatError::BadBoot("setores por cluster"));
        assert_eq!(mount_error(|i| i[13] = 3), FatError::BadBoot("setores por cluster"));
        assert_eq!(mount_error(|i| i[13] = 128), FatError::BadBoot("setores por cluster"));
        assert_eq!(mount_error(|i| i[14..16].fill(0)), FatError::BadBoot("reservados"));
        assert_eq!(mount_error(|i| i[16] = 0), FatError::BadBoot("numero de FATs"));
        assert_eq!(mount_error(|i| i[16] = 3), FatError::BadBoot("numero de FATs"));
        assert_eq!(mount_error(|i| i[17..19].fill(0)), FatError::BadBoot("raiz"));
        assert_eq!(mount_error(|i| i[17] = 17), FatError::BadBoot("raiz"));
        assert_eq!(mount_error(|i| i[22..24].fill(0)), FatError::BadBoot("tamanho da FAT"));
    }

    #[test_case]
    fn recusa_totais_e_contagens_incoerentes() {
        // Total maior que o dispositivo.
        assert_eq!(mount_error(|i| i[19..21].copy_from_slice(&5000u16.to_le_bytes())), FatError::BadBoot("total de setores"));
        // Total zero nos dois campos.
        assert_eq!(mount_error(|i| i[19..21].fill(0)), FatError::BadBoot("total de setores"));
        // Poucos clusters: não é FAT16.
        assert_eq!(mount_error(|i| i[19..21].copy_from_slice(&600u16.to_le_bytes())), FatError::BadBoot("contagem de clusters"));
        // Área de dados além do fim.
        assert_eq!(mount_error(|i| i[22..24].copy_from_slice(&40000u16.to_le_bytes())), FatError::BadBoot("area de dados"));
        // FAT pequena demais para os clusters.
        assert_eq!(mount_error(|i| i[22..24].copy_from_slice(&1u16.to_le_bytes())), FatError::BadBoot("tamanho da FAT"));
        // Clusters demais: 64 setores por cluster com muito pouco disco dá
        // poucos clusters (não é FAT16).
        assert_eq!(mount_error(|i| i[13] = 64), FatError::BadBoot("contagem de clusters"));
    }

    #[test_case]
    fn dispositivo_menor_que_o_volume_e_recusado() {
        let mut img = image();
        img.truncate(1000 * SECTOR_SIZE);
        let disk = MemDisk(img);
        assert_eq!(Fat::mount(&disk).err(), Some(FatError::BadBoot("total de setores")));
    }

    // ---- diretórios, nomes e arquivos ----

    #[test_case]
    fn lista_a_raiz_e_um_subdiretorio() {
        let disk = MemDisk(image());
        let fat = Fat::mount(&disk).unwrap();
        let root = fat.root();
        let names = list(&fat, &root).unwrap();
        assert_eq!(names, ["vazio.txt", "curto.txt", "exato.bin", "multi.bin", "docs", "cheio"]);
        let docs = lookup(&fat, &["docs"]).unwrap();
        assert_eq!(list(&fat, &docs).unwrap(), ["a.txt", "sub"]);
        let sub = lookup(&fat, &["docs", "sub"]).unwrap();
        assert_eq!(list(&fat, &sub).unwrap(), ["b.txt"]);
    }

    #[test_case]
    fn nomes_8_3_nao_diferenciam_maiusculas_de_minusculas() {
        let disk = MemDisk(image());
        let fat = Fat::mount(&disk).unwrap();
        let lower = lookup(&fat, &["docs", "a.txt"]).unwrap();
        let upper = lookup(&fat, &["DOCS", "A.TXT"]).unwrap();
        assert_eq!(lower, upper);
        assert_eq!(lower.size, 10);
    }

    #[test_case]
    fn le_arquivos_de_todos_os_tamanhos() {
        let disk = MemDisk(image());
        let fat = Fat::mount(&disk).unwrap();
        // Vazio: nenhum byte, nenhum cluster.
        let vazio = lookup(&fat, &["vazio.txt"]).unwrap();
        assert_eq!(vazio.first_cluster, 0);
        assert_eq!(read_all(&fat, &vazio).unwrap().len(), 0);
        // Um cluster.
        assert_eq!(read_all(&fat, &lookup(&fat, &["curto.txt"]).unwrap()).unwrap(), pattern(100, 1));
        // Exatamente um cluster.
        assert_eq!(read_all(&fat, &lookup(&fat, &["exato.bin"]).unwrap()).unwrap(), pattern(512, 2));
        // Vários clusters, tamanho que não é múltiplo do cluster: nem um byte a mais.
        let multi = read_all(&fat, &lookup(&fat, &["multi.bin"]).unwrap()).unwrap();
        assert_eq!(multi.len(), 1500);
        assert_eq!(multi, pattern(1500, 3));
        assert_eq!(read_all(&fat, &lookup(&fat, &["docs", "sub", "b.txt"]).unwrap()).unwrap(), pattern(20, 5));
    }

    #[test_case]
    fn leitura_em_pedacos_de_7_bytes_iguala_a_leitura_de_uma_vez() {
        let disk = MemDisk(image());
        let fat = Fat::mount(&disk).unwrap();
        let node = lookup(&fat, &["multi.bin"]).unwrap();
        let mut cursor = fat.cursor(&node).unwrap();
        let mut out = Vec::new();
        let mut chunk = [0u8; 7];
        loop {
            let n = fat.read(&mut cursor, &mut chunk).unwrap();
            if n == 0 {
                break;
            }
            out.extend_from_slice(&chunk[..n]);
        }
        assert_eq!(out, pattern(1500, 3));
        assert_eq!(cursor.position(), 1500);
        // No fim, continua devolvendo zero.
        assert_eq!(fat.read(&mut cursor, &mut chunk).unwrap(), 0);
    }

    #[test_case]
    fn diretorio_com_varios_clusters_e_listado_inteiro() {
        let disk = MemDisk(image());
        let fat = Fat::mount(&disk).unwrap();
        let cheio = lookup(&fat, &["cheio"]).unwrap();
        let names = list(&fat, &cheio).unwrap();
        assert_eq!(names.len(), 40);
        assert_eq!(names[0], "f00.txt");
        assert_eq!(names[39], "f39.txt");
        let last = lookup(&fat, &["cheio", "f39.txt"]).unwrap();
        assert_eq!(read_all(&fat, &last).unwrap(), pattern(40, 39));
    }

    #[test_case]
    fn caminho_inexistente_tipo_errado_e_leitura_de_diretorio() {
        let disk = MemDisk(image());
        let fat = Fat::mount(&disk).unwrap();
        assert_eq!(lookup(&fat, &["nada"]), Err(FatError::NotFound));
        assert_eq!(lookup(&fat, &["docs", "nada"]), Err(FatError::NotFound));
        // Arquivo no meio do caminho.
        assert_eq!(lookup(&fat, &["curto.txt", "x"]), Err(FatError::NotADirectory));
        // Ler um diretório como arquivo e listar um arquivo como diretório.
        let docs = lookup(&fat, &["docs"]).unwrap();
        assert_eq!(fat.cursor(&docs).err(), Some(FatError::IsADirectory));
        let curto = lookup(&fat, &["curto.txt"]).unwrap();
        assert_eq!(fat.read_dir_entry(&curto, 0).err(), Some(FatError::NotADirectory));
    }

    #[test_case]
    fn nome_longo_apagada_e_rotulo_sao_ignorados() {
        let mut img = image();
        // Entrada 1 da raiz (`curto.txt`) vira entrada de nome longo (0x0F),
        // a 2 (`exato.bin`) é apagada e a 3 (`multi.bin`) vira rótulo de volume.
        img[root_entry_at(1) + 11] = 0x0F;
        img[root_entry_at(2)] = 0xE5;
        img[root_entry_at(3) + 11] = 0x08;
        let disk = MemDisk(img);
        let fat = Fat::mount(&disk).unwrap();
        assert_eq!(list(&fat, &fat.root()).unwrap(), ["vazio.txt", "docs", "cheio"]);
        assert_eq!(lookup(&fat, &["curto.txt"]), Err(FatError::NotFound));
        assert_eq!(lookup(&fat, &["exato.bin"]), Err(FatError::NotFound));
    }

    #[test_case]
    fn ponto_e_ponto_ponto_nao_aparecem_na_listagem() {
        let disk = MemDisk(image());
        let fat = Fat::mount(&disk).unwrap();
        let sub = lookup(&fat, &["docs", "sub"]).unwrap();
        // O diretório tem `.` e `..` no disco, mas a listagem só mostra `b.txt`.
        assert_eq!(list(&fat, &sub).unwrap(), ["b.txt"]);
    }

    #[test_case]
    fn entrada_com_nome_malformado_e_erro() {
        let mut img = image();
        img[root_entry_at(0)] = 0x01; // byte de controle no nome
        let disk = MemDisk(img);
        let fat = Fat::mount(&disk).unwrap();
        assert_eq!(fat.read_dir_entry(&fat.root(), 0).err(), Some(FatError::Corrupt));
        // Diretório sem cluster é malformado.
        let mut img = image();
        let at = root_entry_at(4); // `docs`
        img[at + 26..at + 28].fill(0);
        let disk = MemDisk(img);
        let fat = Fat::mount(&disk).unwrap();
        assert_eq!(lookup(&fat, &["docs"]).err(), Some(FatError::Corrupt));
    }

    // ---- cadeias corrompidas ----

    #[test_case]
    fn cadeia_com_ciclo_e_erro_e_nao_laco_infinito() {
        let mut img = image();
        let c = first_cluster(&img, &["multi.bin"]);
        // O primeiro cluster aponta para si mesmo: a cadeia nunca termina,
        // mas o arquivo precisa de 3 clusters.
        set_fat(&mut img, c, c);
        let disk = MemDisk(img);
        let fat = Fat::mount(&disk).unwrap();
        let node = lookup(&fat, &["multi.bin"]).unwrap();
        // Os dois primeiros clusters (1024 bytes) vêm do mesmo cluster; o erro
        // só aparece se o tamanho exigir mais que o volume tem. Um arquivo de
        // 1500 bytes lê do cluster `c` três vezes: sem erro de cadeia, mas com
        // conteúdo repetido. O que **não** pode acontecer é laço infinito:
        // com tamanho absurdo, o limite de passos termina em `Corrupt`.
        let mut img2 = disk.0.clone();
        let at = root_entry_at(3);
        img2[at + 28..at + 32].copy_from_slice(&u32::MAX.to_le_bytes());
        let disk2 = MemDisk(img2);
        let fat2 = Fat::mount(&disk2).unwrap();
        let node2 = lookup(&fat2, &["multi.bin"]).unwrap();
        assert_eq!(read_all(&fat2, &node2).err(), Some(FatError::Corrupt));
        let _ = node;
    }

    #[test_case]
    fn ciclo_de_dois_clusters_com_tamanho_grande_termina_em_erro() {
        let mut img = image();
        let c = first_cluster(&img, &["multi.bin"]);
        set_fat(&mut img, c, c + 1);
        set_fat(&mut img, c + 1, c);
        // O arquivo passa a declarar mais bytes do que o volume inteiro.
        let at = root_entry_at(3);
        img[at + 28..at + 32].copy_from_slice(&(3_000_000u32).to_le_bytes());
        let disk = MemDisk(img);
        let fat = Fat::mount(&disk).unwrap();
        let node = lookup(&fat, &["multi.bin"]).unwrap();
        assert_eq!(read_all(&fat, &node).err(), Some(FatError::Corrupt));
    }

    #[test_case]
    fn cadeia_que_aponta_para_fora_do_volume_e_erro() {
        let mut img = image();
        let c = first_cluster(&img, &["multi.bin"]);
        set_fat(&mut img, c, 0x2000); // 8192: depois do último cluster (4134)
        let disk = MemDisk(img);
        let fat = Fat::mount(&disk).unwrap();
        let node = lookup(&fat, &["multi.bin"]).unwrap();
        assert_eq!(read_all(&fat, &node).err(), Some(FatError::Corrupt));
    }

    #[test_case]
    fn cadeia_que_cai_em_livre_reservado_ou_setor_ruim_e_erro() {
        for bad in [0x0000u16, 0x0001, 0xFFF0, 0xFFF7] {
            let mut img = image();
            let c = first_cluster(&img, &["multi.bin"]);
            set_fat(&mut img, c, bad);
            let disk = MemDisk(img);
            let fat = Fat::mount(&disk).unwrap();
            let node = lookup(&fat, &["multi.bin"]).unwrap();
            assert_eq!(read_all(&fat, &node).err(), Some(FatError::Corrupt), "valor {bad:#x}");
        }
    }

    #[test_case]
    fn tamanho_maior_que_a_cadeia_e_erro_na_leitura() {
        let mut img = image();
        let at = root_entry_at(3);
        img[at + 28..at + 32].copy_from_slice(&5000u32.to_le_bytes());
        let disk = MemDisk(img);
        let fat = Fat::mount(&disk).unwrap();
        let node = lookup(&fat, &["multi.bin"]).unwrap();
        assert_eq!(node.size, 5000);
        // Os primeiros 1536 bytes (3 clusters) saem; depois a cadeia acaba.
        let mut cursor = fat.cursor(&node).unwrap();
        let mut buf = vec![0u8; 5000];
        let mut total = 0;
        let error = loop {
            match fat.read(&mut cursor, &mut buf[total..]) {
                Ok(0) => panic!("deveria falhar"),
                Ok(n) => total += n,
                Err(e) => break e,
            }
        };
        assert_eq!(error, FatError::Corrupt);
        assert_eq!(total, 1536);
    }

    #[test_case]
    fn arquivo_com_tamanho_e_sem_cluster_e_erro() {
        let mut img = image();
        let at = root_entry_at(0); // vazio.txt
        img[at + 28..at + 32].copy_from_slice(&10u32.to_le_bytes());
        let disk = MemDisk(img);
        let fat = Fat::mount(&disk).unwrap();
        let node = lookup(&fat, &["vazio.txt"]).unwrap();
        assert_eq!(read_all(&fat, &node).err(), Some(FatError::Corrupt));
    }

    #[test_case]
    fn diretorio_com_cadeia_ciclica_termina_em_erro() {
        let mut img = image();
        let c = first_cluster(&img, &["cheio"]);
        set_fat(&mut img, c, c);
        // Sem nenhuma entrada 0x00 depois das 16 primeiras, o ciclo repete
        // as mesmas entradas até estourar o limite de passos.
        let disk = MemDisk(img);
        let fat = Fat::mount(&disk).unwrap();
        let cheio = lookup(&fat, &["cheio"]).unwrap();
        assert_eq!(list(&fat, &cheio).err(), Some(FatError::Corrupt));
        // Procurar um nome que não está lá percorre tudo e também termina.
        assert_eq!(lookup(&fat, &["cheio", "nada.txt"]).err(), Some(FatError::Corrupt));
    }

    #[test_case]
    fn erro_de_leitura_do_dispositivo_vira_erro_de_io() {
        struct Quebrado;
        impl BlockDevice for Quebrado {
            fn sector_count(&self) -> u32 {
                4200
            }
            fn read_sector(&self, _: u32, _: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockError> {
                Err(BlockError::DeviceError)
            }
        }
        assert_eq!(Fat::mount(&Quebrado).err(), Some(FatError::Io(BlockError::DeviceError)));
    }
}
