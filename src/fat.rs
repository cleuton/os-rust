//! FAT16: leitura e escrita simples (criar, acrescentar, apagar).
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
//! **Escrita.** Um arquivo só nasce vazio e só cresce pelo fim; apagar devolve
//! os clusters; nada é guardado em cache (cada operação termina com dados, FAT
//! e tamanho já no volume). A ordem das escritas é escolhida para que uma
//! interrupção entre dois setores deixe o volume legível: o tamanho de um
//! arquivo é gravado **por último**, só depois de a cadeia que o cobre existir,
//! e a entrada de diretório é apagada **primeiro**, antes de os clusters serem
//! liberados. No pior caso sobram clusters marcados como usados sem dono.
//! Toda mudança na FAT vai para todas as cópias, a primeira antes das outras.
//! O escritor segue a regra do leitor: o volume é dado não confiável.
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
    /// Criar: o nome já existe (arquivo, diretório ou a raiz).
    Exists,
    /// Sem cluster livre, ou diretório sem entrada livre.
    NoSpace,
    /// O volume não aceita escrita (dispositivo somente leitura, ou mais de
    /// 1 setor por cluster).
    ReadOnly,
}

impl From<BlockError> for FatError {
    fn from(error: BlockError) -> FatError {
        match error {
            BlockError::ReadOnly => FatError::ReadOnly,
            other => FatError::Io(other),
        }
    }
}

/// Onde mora a entrada de diretório de um arquivo: o setor e o deslocamento
/// (múltiplo de 32) dentro dele. Como os diretórios não crescem nem se movem e
/// a entrada de um arquivo aberto não pode ser apagada, a posição é estável, e
/// serve de identidade do arquivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryLoc {
    pub lba: u32,
    pub offset: u16,
}

/// O que `plan_create` descobriu, sem ter escrito nada: o nome (formato do
/// disco) e a primeira posição livre do diretório, se houver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreatePlan {
    name: [u8; 11],
    slot: Option<EntryLoc>,
}

/// Tamanho de uma entrada de diretório.
const ENTRY_SIZE: usize = 32;
/// Valor de fim de cadeia que o escritor grava.
const END_OF_CHAIN: u16 = 0xFFFF;

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

    // ------------------------------------------------------------------
    // Escrita
    // ------------------------------------------------------------------

    /// Recusa a escrita se o dispositivo não a aceita ou se o volume não tem
    /// 1 setor por cluster (o único layout que o escritor conhece).
    pub fn check_writable(&self) -> Result<(), FatError> {
        if !self.dev.writable() || self.geo.sectors_per_cluster != 1 {
            return Err(FatError::ReadOnly);
        }
        Ok(())
    }

    /// Quantas cópias da FAT o volume tem (1 ou 2): a distância entre o início
    /// da FAT e o da raiz, dividida pelo tamanho de uma cópia. `mount` garantiu
    /// `fat_sectors > 0` e `root_start = fat_start + cópias * fat_sectors`.
    fn fat_copies(&self) -> u32 {
        (self.geo.root_start - self.geo.fat_start) / self.geo.fat_sectors
    }

    /// Confere que `cluster` existe no volume (`2..=cluster_count + 1`).
    fn check_cluster(&self, cluster: u16) -> Result<(), FatError> {
        let c = cluster as u32;
        if c < 2 || c > self.geo.cluster_count + 1 {
            return Err(FatError::Corrupt);
        }
        Ok(())
    }

    /// Grava `value` na entrada `cluster` da FAT, em todas as cópias, a
    /// primeira antes das outras.
    fn set_fat_entry(&self, cluster: u16, value: u16) -> Result<(), FatError> {
        self.check_cluster(cluster)?;
        let offset = cluster as u32 * 2;
        let at = (offset % SECTOR_SIZE as u32) as usize;
        for copy in 0..self.fat_copies() {
            let lba = self.geo.fat_start + copy * self.geo.fat_sectors + offset / SECTOR_SIZE as u32;
            let mut sector = [0u8; SECTOR_SIZE];
            self.dev.read_sector(lba, &mut sector)?;
            sector[at..at + 2].copy_from_slice(&value.to_le_bytes());
            self.dev.write_sector(lba, &sector)?;
        }
        Ok(())
    }

    /// Percorre as entradas da FAT dos clusters `2..=cluster_count + 1`, setor
    /// a setor, até `f` devolver `true`.
    fn scan_fat(&self, f: &mut dyn FnMut(u16, u16) -> bool) -> Result<(), FatError> {
        let last = self.geo.cluster_count + 1;
        let per_sector = (SECTOR_SIZE / 2) as u32;
        let mut sector = [0u8; SECTOR_SIZE];
        for s in 0..self.geo.fat_sectors {
            self.dev.read_sector(self.geo.fat_start + s, &mut sector)?;
            for i in 0..per_sector {
                let cluster = s * per_sector + i;
                if cluster < 2 {
                    continue;
                }
                if cluster > last {
                    return Ok(());
                }
                let value = u16::from_le_bytes([sector[i as usize * 2], sector[i as usize * 2 + 1]]);
                if f(cluster as u16, value) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Quantos clusters estão livres (entrada `0x0000`).
    pub fn free_clusters(&self) -> Result<u32, FatError> {
        let mut free = 0u32;
        self.scan_fat(&mut |_, value| {
            if value == 0 {
                free += 1;
            }
            false
        })?;
        Ok(free)
    }

    /// O menor cluster livre, ou `NoSpace`.
    fn find_free_cluster(&self) -> Result<u16, FatError> {
        let mut found = None;
        self.scan_fat(&mut |cluster, value| {
            if value == 0 {
                found = Some(cluster);
                true
            } else {
                false
            }
        })?;
        found.ok_or(FatError::NoSpace)
    }

    /// Chama `f(lba, setor)` para cada setor do diretório, na ordem, até `f`
    /// devolver `true`. Mesmas regras de `walk_dir`: a raiz é uma região fixa e
    /// um subdiretório é uma cadeia percorrida com o limite `MAX_DIR_SECTORS`.
    fn walk_dir_sectors(
        &self,
        dir: &Node,
        f: &mut dyn FnMut(u32, &[u8; SECTOR_SIZE]) -> Result<bool, FatError>,
    ) -> Result<(), FatError> {
        if dir.kind != Kind::Dir {
            return Err(FatError::NotADirectory);
        }
        let mut sector = [0u8; SECTOR_SIZE];
        if dir.first_cluster == 0 {
            for s in 0..self.geo.root_sectors {
                let lba = self.geo.root_start + s;
                self.dev.read_sector(lba, &mut sector)?;
                if f(lba, &sector)? {
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
                    return Err(FatError::Corrupt);
                }
                self.dev.read_sector(first + s, &mut sector)?;
                if f(first + s, &sector)? {
                    return Ok(());
                }
            }
            match self.next_cluster(cluster)? {
                None => return Ok(()),
                Some(next) => cluster = next,
            }
        }
    }

    /// Resolve o diretório pai de um caminho (todos os componentes menos o
    /// último).
    fn parent_dir(&self, components: &[[u8; 11]]) -> Result<Node, FatError> {
        let parent = self.lookup(&components[..components.len() - 1])?;
        if parent.kind != Kind::Dir {
            return Err(FatError::NotADirectory);
        }
        Ok(parent)
    }

    /// Procura `name` em `dir`. Devolve a posição da entrada (se existe) e a
    /// primeira posição livre (entrada apagada `0xE5` ou fim de diretório
    /// `0x00`, na ordem do diretório).
    #[allow(clippy::type_complexity)]
    fn scan_dir_for(&self, dir: &Node, name: &[u8; 11]) -> Result<(Option<(EntryLoc, DirEntry)>, Option<EntryLoc>), FatError> {
        let mut found = None;
        let mut free = None;
        let mut error = None;
        self.walk_dir_sectors(dir, &mut |lba, sector| {
            for (index, raw) in sector.chunks_exact(ENTRY_SIZE).enumerate() {
                let loc = EntryLoc { lba, offset: (index * ENTRY_SIZE) as u16 };
                match raw[0] {
                    0x00 => {
                        if free.is_none() {
                            free = Some(loc);
                        }
                        return Ok(true);
                    }
                    0xE5 => {
                        if free.is_none() {
                            free = Some(loc);
                        }
                        continue;
                    }
                    _ => {}
                }
                // Nome longo, rótulo de volume, `.` e `..`: não são arquivos.
                if raw[11] & 0x3F == 0x0F || raw[11] & 0x08 != 0 || raw[0] == b'.' {
                    continue;
                }
                let mut upper = [0u8; 11];
                upper.copy_from_slice(&raw[..11]);
                upper.make_ascii_uppercase();
                if upper == *name {
                    match parse_entry(raw) {
                        Ok(entry) => found = Some((loc, entry)),
                        Err(e) => error = Some(e),
                    }
                    return Ok(true);
                }
            }
            Ok(false)
        })?;
        match error {
            Some(e) => Err(e),
            None => Ok((found, free)),
        }
    }

    /// A entrada de um arquivo: onde ela mora e o nó. A raiz e os diretórios
    /// são `IsADirectory`; um nome que não existe é `NotFound`.
    pub fn locate(&self, components: &[[u8; 11]]) -> Result<(EntryLoc, Node), FatError> {
        if components.is_empty() {
            return Err(FatError::IsADirectory);
        }
        let parent = self.parent_dir(components)?;
        let name = components[components.len() - 1];
        match self.scan_dir_for(&parent, &name)?.0 {
            None => Err(FatError::NotFound),
            Some((_, entry)) if entry.kind == Kind::Dir => Err(FatError::IsADirectory),
            Some((loc, entry)) => Ok((loc, entry.node())),
        }
    }

    /// Primeira metade de criar um arquivo, **sem escrever nada**: confere que
    /// o volume aceita escrita, que o pai existe e é diretório, que o nome não
    /// existe (`Exists`, inclusive a raiz) e acha a primeira posição livre.
    /// Separar planejar de gravar deixa quem chama conferir outras condições
    /// (descritores livres) antes de criar qualquer coisa.
    pub fn plan_create(&self, components: &[[u8; 11]]) -> Result<CreatePlan, FatError> {
        self.check_writable()?;
        if components.is_empty() {
            return Err(FatError::Exists);
        }
        let parent = self.parent_dir(components)?;
        let name = components[components.len() - 1];
        let (found, slot) = self.scan_dir_for(&parent, &name)?;
        if found.is_some() {
            return Err(FatError::Exists);
        }
        Ok(CreatePlan { name, slot })
    }

    /// Segunda metade de criar: grava a entrada nova (atributo arquivo, sem
    /// cluster, tamanho 0, data e hora zeradas) com **uma** escrita de setor.
    /// Sem posição livre no diretório (que não cresce), `NoSpace`.
    pub fn commit_create(&self, plan: CreatePlan) -> Result<EntryLoc, FatError> {
        self.check_writable()?;
        let loc = plan.slot.ok_or(FatError::NoSpace)?;
        let mut sector = [0u8; SECTOR_SIZE];
        self.dev.read_sector(loc.lba, &mut sector)?;
        let at = loc.offset as usize;
        let mut entry = [0u8; ENTRY_SIZE];
        entry[..11].copy_from_slice(&plan.name);
        entry[11] = 0x20;
        sector[at..at + ENTRY_SIZE].copy_from_slice(&entry);
        self.dev.write_sector(loc.lba, &sector)?;
        Ok(loc)
    }

    /// Lê a entrada de um arquivo vivo: o setor do diretório, o tamanho e o
    /// primeiro cluster. Qualquer coisa que não seja uma entrada de arquivo
    /// vivo é `Corrupt`.
    fn read_file_entry(&self, loc: EntryLoc) -> Result<([u8; SECTOR_SIZE], u32, u16), FatError> {
        let at = loc.offset as usize;
        if at % ENTRY_SIZE != 0 || at + ENTRY_SIZE > SECTOR_SIZE {
            return Err(FatError::Corrupt);
        }
        let mut sector = [0u8; SECTOR_SIZE];
        self.dev.read_sector(loc.lba, &mut sector)?;
        let raw = &sector[at..at + ENTRY_SIZE];
        if raw[0] == 0x00 || raw[0] == 0xE5 || raw[11] & 0x10 != 0 || raw[11] & 0x08 != 0 || raw[11] & 0x3F == 0x0F {
            return Err(FatError::Corrupt);
        }
        let first = u16::from_le_bytes([raw[26], raw[27]]);
        let size = u32::from_le_bytes([raw[28], raw[29], raw[30], raw[31]]);
        Ok((sector, size, first))
    }

    /// Grava o tamanho (e, se `first` é `Some`, o primeiro cluster) na entrada.
    /// Relê o setor: nada fica em cache.
    fn update_entry(&self, loc: EntryLoc, size: u32, first: Option<u16>) -> Result<(), FatError> {
        let (mut sector, _, _) = self.read_file_entry(loc)?;
        let at = loc.offset as usize;
        if let Some(first) = first {
            sector[at + 26..at + 28].copy_from_slice(&first.to_le_bytes());
        }
        sector[at + 28..at + 32].copy_from_slice(&size.to_le_bytes());
        self.dev.write_sector(loc.lba, &sector)?;
        Ok(())
    }

    /// Confere a cadeia do arquivo por inteiro (limite de `cluster_count`
    /// passos) e que ela cobre `size`. Devolve o último cluster (`None` para
    /// arquivo sem cluster).
    fn check_chain(&self, size: u32, first: u16) -> Result<Option<u16>, FatError> {
        if first == 0 {
            return if size == 0 { Ok(None) } else { Err(FatError::Corrupt) };
        }
        self.check_cluster(first)?;
        let mut cluster = first;
        let mut count = 1u32;
        while let Some(next) = self.next_cluster(cluster)? {
            count += 1;
            if count > self.geo.cluster_count {
                // Mais clusters que o volume tem: ciclo.
                return Err(FatError::Corrupt);
            }
            cluster = next;
        }
        if count < size.div_ceil(SECTOR_SIZE as u32) {
            return Err(FatError::Corrupt);
        }
        Ok(Some(cluster))
    }

    /// Acrescenta `data` ao fim do arquivo cuja entrada está em `loc`. Devolve
    /// quantos bytes gravou; `Err` só se não gravou nenhum. Um erro (volume
    /// cheio, falha do disco) depois de já ter gravado algo devolve o que
    /// gravou, e a próxima chamada, que recomeça do mesmo ponto, encontra o
    /// mesmo erro e o devolve.
    ///
    /// Antes de qualquer escrita a cadeia inteira é conferida. Depois, para
    /// cada cluster novo, a ordem é: (1) marcá-lo como fim de cadeia; (2) ligar
    /// o anterior a ele; (3) gravar o setor inteiro (dados mais zeros, para não
    /// sobrar nada de um arquivo apagado); (4) só então gravar o tamanho na
    /// entrada de diretório. Assim o tamanho nunca passa do que a cadeia cobre.
    pub fn append(&self, loc: EntryLoc, data: &[u8]) -> Result<usize, FatError> {
        self.check_writable()?;
        let mut written = 0usize;
        match self.append_into(loc, data, &mut written) {
            Ok(()) => Ok(written),
            Err(_) if written > 0 => Ok(written),
            Err(error) => Err(error),
        }
    }

    fn append_into(&self, loc: EntryLoc, data: &[u8], written: &mut usize) -> Result<(), FatError> {
        let (_, size, first) = self.read_file_entry(loc)?;
        let mut tail = self.check_chain(size, first)?;
        // O tamanho do FAT cabe em 32 bits: o que passar disso não cabe.
        let room = (u32::MAX - size) as usize;
        let data = &data[..data.len().min(room)];
        if data.is_empty() {
            return if room == 0 { Err(FatError::NoSpace) } else { Ok(()) };
        }
        let mut pos = size;
        while *written < data.len() {
            let remaining = data.len() - *written;
            let in_cluster = (pos % SECTOR_SIZE as u32) as usize;
            if in_cluster != 0 {
                // Ainda há espaço no último cluster: ler-modificar-gravar.
                let cluster = tail.ok_or(FatError::Corrupt)?;
                let n = (SECTOR_SIZE - in_cluster).min(remaining);
                let lba = self.cluster_sector(cluster)?;
                let mut sector = [0u8; SECTOR_SIZE];
                self.dev.read_sector(lba, &mut sector)?;
                sector[in_cluster..in_cluster + n].copy_from_slice(&data[*written..*written + n]);
                self.dev.write_sector(lba, &sector)?;
                pos += n as u32;
                self.update_entry(loc, pos, None)?;
                *written += n;
                continue;
            }
            // Fronteira de cluster: um cluster que sobrou ligado depois do fim
            // (escrita interrompida) é reaproveitado; senão, o menor livre.
            let candidate = match tail {
                Some(t) => self.next_cluster(t)?,
                None if first != 0 => Some(first),
                None => None,
            };
            let (cluster, linked) = match candidate {
                Some(c) => {
                    self.check_cluster(c)?;
                    // Confere a entrada do reaproveitado antes de escrever.
                    self.next_cluster(c)?;
                    (c, true)
                }
                None => (self.find_free_cluster()?, false),
            };
            if !linked {
                self.set_fat_entry(cluster, END_OF_CHAIN)?;
                if let Some(t) = tail {
                    self.set_fat_entry(t, cluster)?;
                }
            }
            let n = SECTOR_SIZE.min(remaining);
            let mut sector = [0u8; SECTOR_SIZE];
            sector[..n].copy_from_slice(&data[*written..*written + n]);
            self.dev.write_sector(self.cluster_sector(cluster)?, &sector)?;
            pos += n as u32;
            self.update_entry(loc, pos, if tail.is_none() { Some(cluster) } else { None })?;
            *written += n;
            tail = Some(cluster);
        }
        Ok(())
    }

    /// Apaga o arquivo cuja entrada está em `loc`. Antes de alterar qualquer
    /// coisa confere a cadeia inteira (ciclo, cluster fora do volume, entrada
    /// livre ou reservada no meio, cadeia mais curta que o tamanho: `Corrupt`,
    /// nada alterado). Depois: (1) marca a entrada como apagada (`0xE5`);
    /// (2) libera cada cluster. A entrada vai primeiro: uma interrupção entre
    /// as duas deixa clusters sem dono, nunca um arquivo com a cadeia livre.
    pub fn remove(&self, loc: EntryLoc) -> Result<(), FatError> {
        self.check_writable()?;
        let (mut sector, size, first) = self.read_file_entry(loc)?;
        self.check_chain(size, first)?;
        sector[loc.offset as usize] = 0xE5;
        self.dev.write_sector(loc.lba, &sector)?;
        if first == 0 {
            return Ok(());
        }
        let mut cluster = first;
        loop {
            let next = self.next_cluster(cluster)?;
            self.set_fat_entry(cluster, 0)?;
            match next {
                Some(n) => cluster = n,
                None => return Ok(()),
            }
        }
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

    // ==== escrita ====

    use alloc::collections::BTreeMap;
    use core::cell::{Cell, RefCell};

    /// Um dispositivo gravável em memória. Conta as escritas, guarda quais
    /// setores foram escritos e, com `fail_after = Some(n)`, deixa passar `n`
    /// escritas e falha da seguinte em diante (uma interrupção simulada).
    struct RwDisk {
        data: RefCell<Vec<u8>>,
        writes: Cell<u32>,
        fail_after: Cell<Option<u32>>,
        written: RefCell<Vec<u32>>,
    }

    impl RwDisk {
        fn new(img: Vec<u8>) -> RwDisk {
            RwDisk { data: RefCell::new(img), writes: Cell::new(0), fail_after: Cell::new(None), written: RefCell::new(Vec::new()) }
        }

        fn flaky(img: Vec<u8>, ok_writes: u32) -> RwDisk {
            let disk = RwDisk::new(img);
            disk.fail_after.set(Some(ok_writes));
            disk
        }

        fn bytes(&self) -> Vec<u8> {
            self.data.borrow().clone()
        }

        fn writes(&self) -> u32 {
            self.writes.get()
        }
    }

    impl BlockDevice for RwDisk {
        fn sector_count(&self) -> u32 {
            (self.data.borrow().len() / SECTOR_SIZE) as u32
        }

        fn read_sector(&self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockError> {
            let start = (lba as usize) * SECTOR_SIZE;
            let data = self.data.borrow();
            buf.copy_from_slice(data.get(start..start + SECTOR_SIZE).ok_or(BlockError::OutOfRange)?);
            Ok(())
        }

        fn write_sector(&self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockError> {
            if let Some(limit) = self.fail_after.get() {
                if self.writes.get() >= limit {
                    return Err(BlockError::DeviceError);
                }
            }
            let start = (lba as usize) * SECTOR_SIZE;
            let mut data = self.data.borrow_mut();
            data.get_mut(start..start + SECTOR_SIZE).ok_or(BlockError::OutOfRange)?.copy_from_slice(buf);
            self.writes.set(self.writes.get() + 1);
            self.written.borrow_mut().push(lba);
            Ok(())
        }

        fn writable(&self) -> bool {
            true
        }
    }

    fn comps(path: &[&str]) -> Vec<[u8; 11]> {
        path.iter().map(|p| comp(p)).collect()
    }

    fn criar(fat: &Fat, path: &[&str]) -> EntryLoc {
        let plan = fat.plan_create(&comps(path)).unwrap();
        fat.commit_create(plan).unwrap()
    }

    /// Acrescenta tudo, repetindo `append` até acabar.
    fn acrescentar_tudo(fat: &Fat, loc: EntryLoc, mut data: &[u8]) {
        while !data.is_empty() {
            let n = fat.append(loc, data).unwrap();
            assert!(n > 0);
            data = &data[n..];
        }
    }

    fn conteudo(fat: &Fat, path: &[&str]) -> Vec<u8> {
        read_all(fat, &lookup(fat, path).unwrap()).unwrap()
    }

    /// As duas cópias da FAT (setores 1..18 e 18..35 da imagem) são iguais.
    fn copias_iguais(disk: &RwDisk) -> bool {
        let data = disk.data.borrow();
        data[SECTOR_SIZE..18 * SECTOR_SIZE] == data[18 * SECTOR_SIZE..35 * SECTOR_SIZE]
    }

    fn walk_dir_into(fat: &Fat, dir: &Node, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) -> Result<(), FatError> {
        let mut i = 0;
        while let Some(e) = fat.read_dir_entry(dir, i)? {
            i += 1;
            let path = format!("{}/{}", prefix, e.name_str());
            match e.kind {
                Kind::Dir => walk_dir_into(fat, &e.node(), &path, out)?,
                Kind::File => {
                    out.insert(path, read_all(fat, &e.node())?);
                }
            }
        }
        Ok(())
    }

    /// O leitor percorre o volume inteiro: todos os diretórios e todos os
    /// arquivos lidos até o fim. Qualquer inconsistência é erro.
    fn walk_all(fat: &Fat) -> Result<BTreeMap<String, Vec<u8>>, FatError> {
        let mut out = BTreeMap::new();
        walk_dir_into(fat, &fat.root(), "", &mut out)?;
        Ok(out)
    }

    #[test_case]
    fn criar_escreve_uma_entrada_de_32_bytes_em_um_unico_setor() {
        let disk = RwDisk::new(image());
        let fat = Fat::mount(&disk).unwrap();
        let loc = criar(&fat, &["nova.txt"]);
        assert_eq!(disk.writes(), 1);
        let bytes = disk.bytes();
        let at = loc.lba as usize * SECTOR_SIZE + loc.offset as usize;
        let mut esperado = [0u8; 32];
        esperado[..11].copy_from_slice(b"NOVA    TXT");
        esperado[11] = 0x20;
        assert_eq!(&bytes[at..at + 32], &esperado);
        let node = lookup(&fat, &["nova.txt"]).unwrap();
        assert_eq!((node.size, node.first_cluster), (0, 0));
        assert!(list(&fat, &fat.root()).unwrap().contains(&String::from("nova.txt")));
    }

    #[test_case]
    fn criar_recusa_nome_que_existe_sem_escrever() {
        let disk = RwDisk::new(image());
        let fat = Fat::mount(&disk).unwrap();
        for path in [&["curto.txt"][..], &["CURTO.TXT"], &["Curto.Txt"], &["docs"], &["docs", "a.txt"]] {
            assert_eq!(fat.plan_create(&comps(path)).err(), Some(FatError::Exists));
        }
        // A raiz do volume também "já existe".
        assert_eq!(fat.plan_create(&[]).err(), Some(FatError::Exists));
        assert_eq!(disk.writes(), 0);
    }

    #[test_case]
    fn criar_pai_inexistente_e_arquivo_no_meio_do_caminho() {
        let disk = RwDisk::new(image());
        let fat = Fat::mount(&disk).unwrap();
        assert_eq!(fat.plan_create(&comps(&["nada", "x.txt"])).err(), Some(FatError::NotFound));
        assert_eq!(fat.plan_create(&comps(&["curto.txt", "x.txt"])).err(), Some(FatError::NotADirectory));
        assert_eq!(disk.writes(), 0);
    }

    #[test_case]
    fn criar_usa_a_primeira_posicao_livre() {
        let mut img = image();
        // Apaga a segunda entrada da raiz (`curto.txt`): a posição fica livre,
        // antes do fim do diretório.
        img[root_entry_at(1)] = 0xE5;
        let disk = RwDisk::new(img);
        let fat = Fat::mount(&disk).unwrap();
        let loc = criar(&fat, &["novo.txt"]);
        assert_eq!((loc.lba, loc.offset), (35, 32));
    }

    #[test_case]
    fn diretorio_cheio_na_raiz_e_em_subdiretorio_e_sem_espaco() {
        // Raiz com as 512 entradas ocupadas.
        let mut b = fatimg::Builder::new();
        for i in 0..512 {
            b.file(&format!("f{i:03}.txt"), b"").unwrap();
        }
        let disk = RwDisk::new(b.build().unwrap());
        let fat = Fat::mount(&disk).unwrap();
        let plan = fat.plan_create(&comps(&["extra.txt"])).unwrap();
        assert_eq!(fat.commit_create(plan).err(), Some(FatError::NoSpace));
        assert_eq!(disk.writes(), 0);
        // Subdiretório: tem as posições dos setores que já possui.
        let disk = RwDisk::new(image());
        let fat = Fat::mount(&disk).unwrap();
        let mut criados = 0;
        loop {
            let nome = format!("n{criados:02}.txt");
            let plan = fat.plan_create(&comps(&["cheio", &nome])).unwrap();
            match fat.commit_create(plan) {
                Ok(_) => criados += 1,
                Err(e) => {
                    assert_eq!(e, FatError::NoSpace);
                    break;
                }
            }
        }
        // 40 arquivos mais `.` e `..` são 42 de 48 posições.
        assert_eq!(criados, 6);
        walk_all(&fat).unwrap();
    }

    fn confere_acrescentar(partes: &[usize]) {
        let disk = RwDisk::new(image());
        let fat = Fat::mount(&disk).unwrap();
        let antes = fat.free_clusters().unwrap();
        let loc = criar(&fat, &["gr.bin"]);
        let mut esperado = Vec::new();
        for (i, &n) in partes.iter().enumerate() {
            let dados = pattern(n, 10 + i);
            acrescentar_tudo(&fat, loc, &dados);
            esperado.extend_from_slice(&dados);
            assert_eq!(conteudo(&fat, &["gr.bin"]), esperado);
            assert!(copias_iguais(&disk));
        }
        let usados = esperado.len().div_ceil(SECTOR_SIZE) as u32;
        assert_eq!(fat.free_clusters().unwrap(), antes - usados);
        assert_eq!(lookup(&fat, &["gr.bin"]).unwrap().size as usize, esperado.len());
        walk_all(&fat).unwrap();
    }

    #[test_case]
    fn acrescentar_a_arquivo_vazio_dentro_de_um_cluster_e_cruzando_a_fronteira() {
        confere_acrescentar(&[10]);
        confere_acrescentar(&[10, 20]);
        confere_acrescentar(&[500, 20]);
        confere_acrescentar(&[511, 1, 1]);
    }

    #[test_case]
    fn acrescentar_com_tamanho_multiplo_de_512_e_com_4096_de_uma_vez() {
        confere_acrescentar(&[512, 1]);
        confere_acrescentar(&[512, 512]);
        confere_acrescentar(&[4096]);
        confere_acrescentar(&[1, 4096, 3]);
        confere_acrescentar(&[0]);
    }

    #[test_case]
    fn o_primeiro_cluster_e_o_menor_livre() {
        let disk = RwDisk::new(image());
        let fat = Fat::mount(&disk).unwrap();
        let livre = fat.find_free_cluster().unwrap();
        let loc = criar(&fat, &["a.bin"]);
        assert_eq!(fat.append(loc, b"x").unwrap(), 1);
        assert_eq!(lookup(&fat, &["a.bin"]).unwrap().first_cluster, livre);
    }

    #[test_case]
    fn cluster_liberado_e_reaproveitado_pelo_menor_livre() {
        let disk = RwDisk::new(image());
        let fat = Fat::mount(&disk).unwrap();
        let a = criar(&fat, &["a.bin"]);
        acrescentar_tudo(&fat, a, &pattern(600, 1));
        let c1 = lookup(&fat, &["a.bin"]).unwrap().first_cluster;
        let b = criar(&fat, &["b.bin"]);
        acrescentar_tudo(&fat, b, &pattern(100, 2));
        let (loc_a, _) = fat.locate(&comps(&["a.bin"])).unwrap();
        fat.remove(loc_a).unwrap();
        let c = criar(&fat, &["c.bin"]);
        acrescentar_tudo(&fat, c, &pattern(100, 3));
        assert_eq!(lookup(&fat, &["c.bin"]).unwrap().first_cluster, c1);
        assert_eq!(conteudo(&fat, &["b.bin"]), pattern(100, 2));
        assert_eq!(conteudo(&fat, &["c.bin"]), pattern(100, 3));
    }

    #[test_case]
    fn volume_cheio_grava_parcial_depois_no_space_e_continua_legivel() {
        let disk = RwDisk::new(image());
        let fat = Fat::mount(&disk).unwrap();
        let base = walk_all(&fat).unwrap();
        let loc = criar(&fat, &["enche.bin"]);
        // Deixa 3 clusters livres.
        let mut livres = fat.free_clusters().unwrap();
        while livres > 3 {
            let quer = ((livres - 3) as usize * SECTOR_SIZE).min(4096);
            let n = fat.append(loc, &pattern(quer, 0)).unwrap();
            assert_eq!(n, quer);
            livres = fat.free_clusters().unwrap();
        }
        // Pede 4096 bytes (8 clusters): só cabem 3 (1536 bytes).
        assert_eq!(fat.append(loc, &pattern(4096, 1)).unwrap(), 3 * SECTOR_SIZE);
        assert_eq!(fat.free_clusters().unwrap(), 0);
        assert_eq!(fat.append(loc, b"x").err(), Some(FatError::NoSpace));
        let cheio = walk_all(&fat).unwrap();
        for (nome, dados) in &base {
            assert_eq!(&cheio[nome], dados);
        }
        // Apagar devolve espaço, e uma escrita nova funciona.
        fat.remove(loc).unwrap();
        assert!(fat.free_clusters().unwrap() > 3000);
        let outro = criar(&fat, &["ok.txt"]);
        assert_eq!(fat.append(outro, b"ok").unwrap(), 2);
    }

    #[test_case]
    fn remover_apaga_a_entrada_e_libera_a_cadeia_nas_duas_copias() {
        let disk = RwDisk::new(image());
        let fat = Fat::mount(&disk).unwrap();
        let antes = fat.free_clusters().unwrap();
        let loc = criar(&fat, &["gr.bin"]);
        acrescentar_tudo(&fat, loc, &pattern(1300, 4));
        let first = lookup(&fat, &["gr.bin"]).unwrap().first_cluster;
        assert!(fat.free_clusters().unwrap() < antes);
        fat.remove(loc).unwrap();
        assert_eq!(fat.free_clusters().unwrap(), antes);
        let bytes = disk.bytes();
        assert_eq!(bytes[loc.lba as usize * SECTOR_SIZE + loc.offset as usize], 0xE5);
        // O primeiro cluster está livre nas duas cópias.
        for copia in 0..2 {
            let at = (1 + copia * 17) * SECTOR_SIZE + first as usize * 2;
            assert_eq!(&bytes[at..at + 2], &[0, 0]);
        }
        assert!(copias_iguais(&disk));
        assert!(!list(&fat, &fat.root()).unwrap().contains(&String::from("gr.bin")));
        assert_eq!(lookup(&fat, &["gr.bin"]).err(), Some(FatError::NotFound));
    }

    #[test_case]
    fn remover_arquivo_vazio_so_apaga_a_entrada() {
        let disk = RwDisk::new(image());
        let fat = Fat::mount(&disk).unwrap();
        let antes = fat.free_clusters().unwrap();
        let (loc, _) = fat.locate(&comps(&["vazio.txt"])).unwrap();
        fat.remove(loc).unwrap();
        assert_eq!(fat.free_clusters().unwrap(), antes);
        assert_eq!(lookup(&fat, &["vazio.txt"]).err(), Some(FatError::NotFound));
    }

    #[test_case]
    fn cadeia_mais_longa_que_o_tamanho_e_liberada_inteira() {
        let disk = RwDisk::new(image());
        let fat = Fat::mount(&disk).unwrap();
        let (loc, node) = fat.locate(&comps(&["curto.txt"])).unwrap();
        let antes = fat.free_clusters().unwrap();
        // Liga a `curto.txt` (1 cluster) mais um cluster livre além do tamanho.
        let extra = fat.find_free_cluster().unwrap();
        fat.set_fat_entry(extra, END_OF_CHAIN).unwrap();
        fat.set_fat_entry(node.first_cluster, extra).unwrap();
        assert_eq!(fat.free_clusters().unwrap(), antes - 1);
        fat.remove(loc).unwrap();
        assert_eq!(fat.free_clusters().unwrap(), antes + 1);
    }

    /// Adultera a imagem e confere que `append` e `remove` recusam com
    /// `Corrupt`, sem escrever nada.
    fn confere_recusa(mutate: impl FnOnce(&mut Vec<u8>, u16)) {
        let mut img = image();
        let c = first_cluster(&img, &["multi.bin"]);
        mutate(&mut img, c);
        let disk = RwDisk::new(img);
        let fat = Fat::mount(&disk).unwrap();
        let (loc, _) = fat.locate(&comps(&["multi.bin"])).unwrap();
        let antes = disk.bytes();
        assert_eq!(fat.append(loc, b"abc").err(), Some(FatError::Corrupt));
        assert_eq!(fat.remove(loc).err(), Some(FatError::Corrupt));
        assert_eq!(disk.writes(), 0);
        assert_eq!(disk.bytes(), antes);
    }

    #[test_case]
    fn cadeia_adulterada_e_recusada_sem_escrever() {
        // Ciclo: o primeiro cluster aponta para si mesmo.
        confere_recusa(|img, c| set_fat(img, c, c));
        // Ciclo de dois clusters.
        confere_recusa(|img, c| {
            set_fat(img, c, c + 1);
            set_fat(img, c + 1, c);
        });
        // Aponta para fora do volume.
        confere_recusa(|img, c| set_fat(img, c, 0x7FFF));
        // Entrada livre no meio da cadeia.
        confere_recusa(|img, c| set_fat(img, c + 1, 0x0000));
        // Entrada reservada no meio da cadeia.
        confere_recusa(|img, c| set_fat(img, c + 1, 0xFFF7));
        // Cadeia mais curta que o tamanho.
        confere_recusa(|img, c| {
            let at = root_entry_at(3);
            img[at + 28..at + 32].copy_from_slice(&4000u32.to_le_bytes());
            let _ = c;
        });
        // Tamanho sem nenhum cluster.
        confere_recusa(|img, _| {
            let at = root_entry_at(3);
            img[at + 26..at + 28].copy_from_slice(&0u16.to_le_bytes());
        });
    }

    #[test_case]
    fn cluster_novo_nao_guarda_sobra_de_arquivo_apagado() {
        let disk = RwDisk::new(image());
        let fat = Fat::mount(&disk).unwrap();
        let a = criar(&fat, &["a.bin"]);
        acrescentar_tudo(&fat, a, &[0xAA; 512]);
        let c = lookup(&fat, &["a.bin"]).unwrap().first_cluster;
        fat.remove(a).unwrap();
        let b = criar(&fat, &["b.bin"]);
        acrescentar_tudo(&fat, b, b"abc");
        assert_eq!(lookup(&fat, &["b.bin"]).unwrap().first_cluster, c);
        let sector = fat.cluster_sector(c).unwrap() as usize * SECTOR_SIZE;
        let bytes = disk.bytes();
        assert_eq!(&bytes[sector..sector + 3], b"abc");
        assert!(bytes[sector + 3..sector + SECTOR_SIZE].iter().all(|&x| x == 0));
    }

    #[test_case]
    fn volume_de_2_setores_por_cluster_e_dispositivo_sem_escrita_recusam() {
        // 2 setores por cluster, com 4100 clusters.
        let total = 2 * 4100 + 67;
        let mut img = image();
        img.resize(total * SECTOR_SIZE, 0);
        img[13] = 2;
        img[19..21].copy_from_slice(&0u16.to_le_bytes());
        img[32..36].copy_from_slice(&(total as u32).to_le_bytes());
        let disk = RwDisk::new(img);
        let fat = Fat::mount(&disk).unwrap();
        assert_eq!(fat.geometry().sectors_per_cluster, 2);
        assert_eq!(fat.plan_create(&comps(&["x.txt"])).err(), Some(FatError::ReadOnly));
        let (loc, _) = fat.locate(&comps(&["curto.txt"])).unwrap();
        assert_eq!(fat.append(loc, b"x").err(), Some(FatError::ReadOnly));
        assert_eq!(fat.remove(loc).err(), Some(FatError::ReadOnly));
        assert_eq!(disk.writes(), 0);
        // Um dispositivo que não sabe escrever também recusa.
        let ro = MemDisk(image());
        let fat = Fat::mount(&ro).unwrap();
        assert_eq!(fat.plan_create(&comps(&["x.txt"])).err(), Some(FatError::ReadOnly));
    }

    #[test_case]
    fn fat_entry_e_set_fat_entry_validam_o_cluster() {
        let disk = RwDisk::new(image());
        let fat = Fat::mount(&disk).unwrap();
        assert_eq!(fat.set_fat_entry(0, 5).err(), Some(FatError::Corrupt));
        assert_eq!(fat.set_fat_entry(1, 5).err(), Some(FatError::Corrupt));
        let alem = (fat.geometry().cluster_count + 2) as u16;
        assert_eq!(fat.set_fat_entry(alem, 5).err(), Some(FatError::Corrupt));
        let livre = fat.find_free_cluster().unwrap();
        fat.set_fat_entry(livre, 0x1234).unwrap();
        assert!(copias_iguais(&disk));
        assert_ne!(fat.find_free_cluster().unwrap(), livre);
    }

    // ---- interrupção simulada ----

    /// Roda `op` num disco que falha depois de `n` escritas, para cada `n`, até
    /// a operação terminar sem falha. Depois de cada corte, remonta o volume
    /// sobre os mesmos bytes e confere que ele continua montável, que o leitor
    /// o percorre inteiro e que `confere` aceita o resultado.
    fn interrompe(
        base: &[u8],
        op: impl Fn(&Fat) -> bool,
        confere: impl Fn(&BTreeMap<String, Vec<u8>>, &BTreeMap<String, Vec<u8>>),
    ) {
        let antes = {
            let disk = RwDisk::new(base.to_vec());
            walk_all(&Fat::mount(&disk).unwrap()).unwrap()
        };
        let mut n = 0;
        loop {
            let disk = RwDisk::flaky(base.to_vec(), n);
            let completa = {
                let fat = Fat::mount(&disk).unwrap();
                op(&fat)
            };
            // Remonta sobre os bytes que ficaram, num disco que não falha.
            let depois_disk = RwDisk::new(disk.bytes());
            let fat = Fat::mount(&depois_disk).expect("o volume continua montavel");
            let depois = walk_all(&fat).expect("o leitor percorre o volume inteiro");
            confere(&antes, &depois);
            if completa {
                break;
            }
            // Depois do corte, acrescentar de novo a um arquivo qualquer funciona.
            let loc = criar(&fat, &["pos.txt"]);
            assert_eq!(fat.append(loc, b"ok").unwrap(), 2);
            n += 1;
            assert!(n < 200, "a operacao nunca terminou");
        }
    }

    /// Todo arquivo de `antes` está igual em `depois`, menos `exceto`.
    fn anteriores_intactos(antes: &BTreeMap<String, Vec<u8>>, depois: &BTreeMap<String, Vec<u8>>, exceto: &str) {
        for (nome, dados) in antes {
            if nome != exceto {
                assert_eq!(depois.get(nome), Some(dados), "{} mudou", nome);
            }
        }
    }

    #[test_case]
    fn interrupcao_ao_criar_deixa_o_volume_consistente() {
        interrompe(
            &image(),
            |fat| fat.plan_create(&comps(&["novo.txt"])).and_then(|p| fat.commit_create(p)).is_ok(),
            |antes, depois| {
                anteriores_intactos(antes, depois, "/novo.txt");
                // O arquivo novo ou não existe, ou existe vazio.
                if let Some(dados) = depois.get("/novo.txt") {
                    assert!(dados.is_empty());
                }
            },
        );
    }

    /// Uma imagem com `x.bin` de `tamanho` bytes.
    fn com_x(tamanho: usize) -> Vec<u8> {
        let disk = RwDisk::new(image());
        let fat = Fat::mount(&disk).unwrap();
        let loc = criar(&fat, &["x.bin"]);
        acrescentar_tudo(&fat, loc, &pattern(tamanho, 7));
        disk.bytes()
    }

    fn interrompe_acrescentar(tamanho: usize, novo: usize) {
        let velho = pattern(tamanho, 7);
        let novo_dados = pattern(novo, 9);
        let mut tudo = velho.clone();
        tudo.extend_from_slice(&novo_dados);
        interrompe(
            &com_x(tamanho),
            |fat| {
                let (loc, _) = fat.locate(&comps(&["x.bin"])).unwrap();
                matches!(fat.append(loc, &novo_dados), Ok(n) if n == novo_dados.len())
            },
            |antes, depois| {
                anteriores_intactos(antes, depois, "/x.bin");
                // O arquivo está como antes, ou com um prefixo do que se
                // queria gravar: nunca com lixo nem com tamanho além da cadeia.
                let x = &depois["/x.bin"];
                assert!(x.len() >= velho.len() && x.len() <= tudo.len());
                assert_eq!(x[..], tudo[..x.len()]);
            },
        );
    }

    #[test_case]
    fn interrupcao_ao_acrescentar_atravessando_um_cluster_deixa_o_volume_consistente() {
        // 509 bytes + 10: 3 no cluster que existe, 7 num cluster novo.
        interrompe_acrescentar(509, 10);
        // Tamanho exatamente múltiplo de 512 e vários clusters novos.
        interrompe_acrescentar(512, 1100);
        // Arquivo vazio.
        interrompe_acrescentar(0, 600);
    }

    #[test_case]
    fn interrupcao_ao_remover_deixa_o_volume_consistente() {
        interrompe(
            &image(),
            |fat| {
                let (loc, _) = fat.locate(&comps(&["multi.bin"])).unwrap();
                fat.remove(loc).is_ok()
            },
            |antes, depois| {
                anteriores_intactos(antes, depois, "/multi.bin");
                // O arquivo ou continua inteiro, ou sumiu.
                if let Some(dados) = depois.get("/multi.bin") {
                    assert_eq!(dados, &antes["/multi.bin"]);
                }
            },
        );
    }
}
