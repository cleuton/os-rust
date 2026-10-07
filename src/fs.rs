//! Volumes, caminhos e arquivos abertos.
//!
//! O kernel tem **dois volumes fixos**, ambos FAT16 somente leitura:
//!
//! | Prefixo  | O que é                                   |
//! |----------|-------------------------------------------|
//! | `/ram`   | o ramdisk, embutido na imagem de boot     |
//! | `/disco` | o disco ATA (canal primário, escravo)     |
//!
//! Um caminho é `/<volume>/<componente>/...`, sem diferenciar maiúsculas de
//! minúsculas, com componentes no formato 8.3. `.` e `..` são **inválidos**:
//! nenhum caminho consegue sair do volume. Este módulo conhece prefixos e
//! estados de volume; o layout FAT fica em `fat.rs`, e a origem dos bytes
//! (memória ou disco) em `blockdev.rs` e `ata.rs`.
//!
//! Um volume que não pôde ser montado (sem disco, imagem inválida, erro de
//! leitura) fica **indisponível** com o motivo, e qualquer operação sobre ele
//! devolve `FsError::Unavailable`: o kernel nunca entra em pânico por causa de
//! um volume.

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

use crate::ata::{AtaDisk, AtaError, Bus, Drive};
use crate::blockdev::{BlockDevice, RamDisk};
use crate::fat::{Cursor, DirEntry, Fat, FatError, Geometry, Kind, Node};

pub use abi::{MAX_EXEC_SIZE, MAX_OPEN_FILES, MAX_PATH_LEN};

/// Profundidade máxima de um caminho, em componentes (sem contar o volume).
pub const MAX_DEPTH: usize = 8;

/// A imagem do ramdisk, gerada pelo `build.rs` a partir de `discos/ram/`.
static RAMDISK: RamDisk = RamDisk(include_bytes!(concat!(env!("OUT_DIR"), "/ramdisk.img")));

/// Um dispositivo de blocos que pode morar num `static`.
pub type Device = &'static (dyn BlockDevice + Sync);

/// Os dois volumes do kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeId {
    Ram,
    Disco,
}

impl VolumeId {
    /// Todos os volumes, na ordem em que `ls` sem argumento os lista.
    pub const ALL: [VolumeId; 2] = [VolumeId::Ram, VolumeId::Disco];

    /// O prefixo do volume em um caminho, sem a barra: `ram` ou `disco`.
    pub fn name(self) -> &'static str {
        match self {
            VolumeId::Ram => "ram",
            VolumeId::Disco => "disco",
        }
    }

    fn index(self) -> usize {
        match self {
            VolumeId::Ram => 0,
            VolumeId::Disco => 1,
        }
    }
}

/// Por que um volume está indisponível.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Nenhum disco no barramento.
    NoDisk,
    /// Há um dispositivo, mas não é um disco ATA.
    NotAta,
    /// O volume não é um FAT16 coerente (o motivo, do setor de boot).
    BadVolume(&'static str),
    /// Erro ao ler o disco.
    IoError,
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reason::NoDisk => write!(f, "sem disco"),
            Reason::NotAta => write!(f, "dispositivo nao e um disco ATA"),
            Reason::BadVolume(detail) => write!(f, "volume FAT invalido: {}", detail),
            Reason::IoError => write!(f, "erro de leitura"),
        }
    }
}

/// O que o kernel sabe de um volume.
#[derive(Clone, Copy)]
pub enum VolumeState {
    /// Montado: o dispositivo e a geometria validada.
    Ready { dev: Device, geo: Geometry },
    /// Não montado, com o motivo.
    Unavailable(Reason),
}

/// Os erros que o shell e as syscalls veem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    /// O prefixo não é `/ram` nem `/disco`.
    UnknownVolume,
    /// O volume existe, mas não está disponível.
    Unavailable(Reason),
    /// O caminho (ou um componente) não existe.
    NotFound,
    /// Um componente intermediário, ou o alvo de uma listagem, é um arquivo.
    NotADirectory,
    /// O alvo de uma leitura é um diretório.
    IsADirectory,
    /// Mais de `MAX_PATH_LEN` bytes ou mais de `MAX_DEPTH` componentes.
    PathTooLong,
    /// Caminho malformado: sem `/` inicial, componente vazio, caractere
    /// inválido, nome fora de 8.3, `.` ou `..`.
    InvalidPath,
    /// Arquivo maior que o limite pedido (só `read_whole`).
    TooBig { size: usize, max: usize },
    /// Descritor inválido.
    BadDescriptor,
    /// Tabela de arquivos cheia.
    TooManyOpen,
    /// Erro de leitura ou volume corrompido.
    Io,
}

impl From<FatError> for FsError {
    fn from(error: FatError) -> FsError {
        match error {
            FatError::NotFound => FsError::NotFound,
            FatError::NotADirectory => FsError::NotADirectory,
            FatError::IsADirectory => FsError::IsADirectory,
            // Setor de boot ruim, erro de bloco e estrutura corrompida: do
            // ponto de vista de quem lê, tudo é erro de leitura.
            FatError::BadBoot(_) | FatError::Io(_) | FatError::Corrupt => FsError::Io,
        }
    }
}

// ---------------------------------------------------------------------------
// Caminhos
// ---------------------------------------------------------------------------

/// Um caminho já validado: o volume e os componentes, cada um no formato do
/// disco (8 + 3, maiúsculas, com espaços). Sem heap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedPath {
    pub volume: VolumeId,
    components: [[u8; 11]; MAX_DEPTH],
    count: usize,
}

impl ParsedPath {
    /// Os componentes, sem o volume. Vazio para a raiz do volume.
    pub fn components(&self) -> &[[u8; 11]] {
        &self.components[..self.count]
    }
}

/// Traduz `"ola.txt"` para o formato do disco, `"OLA     TXT"`. `.` e `..`
/// não passam: o nome-base seria vazio.
fn short_name(part: &[u8]) -> Result<[u8; 11], FsError> {
    let (base, ext) = match part.iter().position(|&b| b == b'.') {
        Some(dot) => (&part[..dot], &part[dot + 1..]),
        None => (part, &part[..0]),
    };
    let valid = |s: &[u8]| s.iter().all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'~' | b'$'));
    if base.is_empty() || base.len() > 8 || ext.len() > 3 || !valid(base) || !valid(ext) {
        return Err(FsError::InvalidPath);
    }
    let mut name = [b' '; 11];
    for (k, &c) in base.iter().enumerate() {
        name[k] = c.to_ascii_uppercase();
    }
    for (k, &c) in ext.iter().enumerate() {
        name[8 + k] = c.to_ascii_uppercase();
    }
    Ok(name)
}

/// Valida e separa um caminho (ver o cabeçalho do módulo).
pub fn parse(path: &[u8]) -> Result<ParsedPath, FsError> {
    if path.len() > MAX_PATH_LEN {
        return Err(FsError::PathTooLong);
    }
    let rest = match path.split_first() {
        Some((b'/', rest)) => rest,
        _ => return Err(FsError::InvalidPath),
    };
    let mut parts = rest.split(|&b| b == b'/');
    let volume_name = parts.next().unwrap_or(&[]);
    let volume = VolumeId::ALL
        .into_iter()
        .find(|v| v.name().as_bytes().eq_ignore_ascii_case(volume_name))
        .ok_or(FsError::UnknownVolume)?;
    let mut parsed = ParsedPath { volume, components: [[b' '; 11]; MAX_DEPTH], count: 0 };
    let mut parts = parts.peekable();
    while let Some(part) = parts.next() {
        if part.is_empty() {
            // Só o último pedaço pode ser vazio (barra no fim: `/ram/`).
            if parts.peek().is_some() {
                return Err(FsError::InvalidPath);
            }
            break;
        }
        if parsed.count == MAX_DEPTH {
            return Err(FsError::PathTooLong);
        }
        parsed.components[parsed.count] = short_name(part)?;
        parsed.count += 1;
    }
    Ok(parsed)
}

// ---------------------------------------------------------------------------
// Volumes
// ---------------------------------------------------------------------------

static VOLUMES: Mutex<[VolumeState; 2]> =
    Mutex::new([VolumeState::Unavailable(Reason::NoDisk), VolumeState::Unavailable(Reason::NoDisk)]);

/// Tenta montar um volume FAT16 sobre `dev`.
pub fn mount_state(dev: Device) -> VolumeState {
    match Fat::mount(dev) {
        Ok(fat) => VolumeState::Ready { dev, geo: fat.geometry() },
        Err(FatError::BadBoot(detail)) => VolumeState::Unavailable(Reason::BadVolume(detail)),
        Err(_) => VolumeState::Unavailable(Reason::IoError),
    }
}

fn state(id: VolumeId) -> VolumeState {
    VOLUMES.lock()[id.index()]
}

/// Troca o estado de um volume e devolve o anterior. Existe para os testes
/// simularem disco ausente, volume corrompido ou cadeia com ciclo (e depois
/// restaurarem o estado original); o kernel de produção não a usa.
#[doc(hidden)]
pub fn replace_volume(id: VolumeId, new: VolumeState) -> VolumeState {
    core::mem::replace(&mut VOLUMES.lock()[id.index()], new)
}

/// `Ok` se o volume está montado; senão o motivo.
pub fn volume_status(id: VolumeId) -> Result<(), Reason> {
    match state(id) {
        VolumeState::Ready { .. } => Ok(()),
        VolumeState::Unavailable(reason) => Err(reason),
    }
}

/// O disco de dados, depois de identificado: guardado num `static` para que
/// o volume `/disco` possa apontar para ele (`Device` é `'static`).
static DISCO_DISK: spin::Once<AtaDisk> = spin::Once::new();

/// Por que um volume ficou indisponível quando o disco ATA não pôde ser usado.
pub fn reason_for(error: AtaError) -> Reason {
    match error {
        AtaError::NoDevice => Reason::NoDisk,
        AtaError::NotAta => Reason::NotAta,
        AtaError::Timeout | AtaError::DeviceError => Reason::IoError,
    }
}

/// Procura o disco de dados (canal primário, escravo: o mestre é o disco de
/// boot) e tenta montá-lo.
fn probe_disco() -> VolumeState {
    match AtaDisk::detect(Bus::Primary, Drive::Slave) {
        Ok(disk) => mount_state(DISCO_DISK.call_once(|| disk)),
        Err(error) => VolumeState::Unavailable(reason_for(error)),
    }
}

/// Monta os volumes e registra o resultado na serial. Nunca entra em pânico:
/// qualquer falha (sem disco, disco que não é ATA, volume inválido, erro de
/// leitura) deixa o volume indisponível, com o motivo, e o resto do kernel
/// segue.
pub fn init() {
    *VOLUMES.lock() = [mount_state(&RAMDISK), probe_disco()];
    report(VolumeId::Ram);
    report(VolumeId::Disco);
}

/// Uma linha de serial com o estado do volume.
fn report(id: VolumeId) {
    match state(id) {
        VolumeState::Ready { geo, .. } => {
            crate::serial_println!("[fs] /{}: FAT16, {} clusters", id.name(), geo.cluster_count)
        }
        VolumeState::Unavailable(reason) => crate::serial_println!("[fs] /{}: indisponivel ({})", id.name(), reason),
    }
}

/// Roda `f` com o volume montado. O estado é copiado antes, então nenhuma
/// trava fica segura durante a leitura do disco.
fn with_fat<R>(id: VolumeId, f: impl FnOnce(&Fat<'_>) -> Result<R, FatError>) -> Result<R, FsError> {
    match state(id) {
        VolumeState::Ready { dev, geo } => Ok(f(&Fat::from_parts(dev, geo))?),
        VolumeState::Unavailable(reason) => Err(FsError::Unavailable(reason)),
    }
}

/// Resolve um caminho.
pub fn lookup(path: &[u8]) -> Result<(VolumeId, Node), FsError> {
    let parsed = parse(path)?;
    // Um volume indisponível é reportado antes de qualquer outro erro de
    // caminho inexistente.
    let node = with_fat(parsed.volume, |fat| fat.lookup(parsed.components()))?;
    Ok((parsed.volume, node))
}

/// A `index`-ésima entrada de um diretório de um volume.
pub fn dir_entry(volume: VolumeId, dir: &Node, index: u32) -> Result<Option<DirEntry>, FsError> {
    with_fat(volume, |fat| fat.read_dir_entry(dir, index))
}

// ---------------------------------------------------------------------------
// Arquivos abertos
// ---------------------------------------------------------------------------

/// Um arquivo ou diretório aberto, somente para leitura: onde ele mora e onde
/// a leitura está. Dados simples (`Copy`), sem heap.
#[derive(Debug, Clone, Copy)]
pub struct OpenFile {
    volume: VolumeId,
    node: Node,
    /// Posição da leitura sequencial (só arquivos).
    cursor: Option<Cursor>,
    /// Quantas entradas de diretório já foram entregues (só diretórios).
    entries_read: u32,
}

impl OpenFile {
    /// Abre o arquivo ou diretório `path`.
    pub fn open(path: &[u8]) -> Result<OpenFile, FsError> {
        let (volume, node) = lookup(path)?;
        let cursor = match node.kind {
            Kind::File => Some(with_fat(volume, |fat| fat.cursor(&node))?),
            Kind::Dir => None,
        };
        Ok(OpenFile { volume, node, cursor, entries_read: 0 })
    }

    /// Arquivo ou diretório.
    pub fn kind(&self) -> Kind {
        self.node.kind
    }

    /// Tamanho em bytes (`0` para diretório).
    pub fn size(&self) -> u32 {
        self.node.size
    }

    /// Lê até `buf.len()` bytes e avança a posição; `0` no fim do arquivo.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, FsError> {
        let cursor = self.cursor.as_mut().ok_or(FsError::IsADirectory)?;
        with_fat(self.volume, |fat| fat.read(cursor, buf))
    }

    /// A próxima entrada do diretório, ou `None` no fim.
    pub fn next_entry(&mut self) -> Result<Option<DirEntry>, FsError> {
        if self.node.kind != Kind::Dir {
            return Err(FsError::NotADirectory);
        }
        let entry = dir_entry(self.volume, &self.node, self.entries_read)?;
        if entry.is_some() {
            self.entries_read += 1;
        }
        Ok(entry)
    }
}

// ---------------------------------------------------------------------------
// Tabela de arquivos abertos de uma tarefa
// ---------------------------------------------------------------------------

/// Quantos arquivos estão abertos agora, somando todas as tarefas. Existe para
/// os testes provarem que nenhum descritor sobrevive ao fim do programa.
static OPEN_FILES: AtomicUsize = AtomicUsize::new(0);

/// Quantos arquivos (ou diretórios) estão abertos neste instante, em todas as
/// tarefas.
pub fn open_files_in_use() -> usize {
    OPEN_FILES.load(Ordering::Relaxed)
}

/// Os arquivos abertos de **uma** tarefa: `MAX_OPEN_FILES` posições fixas, sem
/// heap. O descritor que o programa vê é o índice da posição. Como a tabela é
/// um campo da `Task`, um programa nunca enxerga os descritores nem as posições
/// de leitura de outro: o isolamento é estrutural. Quando a tarefa termina (por
/// `exit` ou por erro) ela é destruída e o `Drop` abaixo fecha o que sobrou.
pub struct FileTable {
    slots: [Option<OpenFile>; MAX_OPEN_FILES],
}

impl FileTable {
    /// Uma tabela sem nenhum arquivo aberto.
    pub const fn new() -> FileTable {
        FileTable { slots: [None; MAX_OPEN_FILES] }
    }

    /// Abre `path` na menor posição livre e devolve o descritor. O caminho é
    /// resolvido antes de procurar a posição, então um caminho ruim é
    /// reportado como tal mesmo com a tabela cheia.
    pub fn open(&mut self, path: &[u8]) -> Result<usize, FsError> {
        let file = OpenFile::open(path)?;
        let fd = self.slots.iter().position(|slot| slot.is_none()).ok_or(FsError::TooManyOpen)?;
        self.slots[fd] = Some(file);
        OPEN_FILES.fetch_add(1, Ordering::Relaxed);
        Ok(fd)
    }

    /// O arquivo aberto no descritor `fd`, ou `BadDescriptor` se `fd` está
    /// fora da tabela ou fechado.
    pub fn get_mut(&mut self, fd: usize) -> Result<&mut OpenFile, FsError> {
        self.slots.get_mut(fd).and_then(|slot| slot.as_mut()).ok_or(FsError::BadDescriptor)
    }

    /// Fecha o descritor `fd`. Fechar de novo é `BadDescriptor`.
    pub fn close(&mut self, fd: usize) -> Result<(), FsError> {
        let slot = self.slots.get_mut(fd).ok_or(FsError::BadDescriptor)?;
        if slot.take().is_none() {
            return Err(FsError::BadDescriptor);
        }
        OPEN_FILES.fetch_sub(1, Ordering::Relaxed);
        Ok(())
    }
}

impl Default for FileTable {
    fn default() -> Self {
        FileTable::new()
    }
}

impl Drop for FileTable {
    fn drop(&mut self) {
        for slot in self.slots.iter_mut() {
            if slot.take().is_some() {
                OPEN_FILES.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
}

/// Lê o arquivo inteiro para o heap. Recusa, **antes** de ler qualquer dado,
/// um arquivo maior que `max`.
pub fn read_whole(path: &[u8], max: usize) -> Result<Vec<u8>, FsError> {
    let mut file = OpenFile::open(path)?;
    if file.kind() == Kind::Dir {
        return Err(FsError::IsADirectory);
    }
    let size = file.size() as usize;
    if size > max {
        return Err(FsError::TooBig { size, max });
    }
    let mut data = vec![0u8; size];
    let mut filled = 0;
    while filled < size {
        let n = file.read(&mut data[filled..])?;
        if n == 0 {
            // A cadeia acabou antes do tamanho: o `read` já devolveria erro;
            // isto é só a rede de segurança contra um laço sem progresso.
            return Err(FsError::Io);
        }
        filled += n;
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comp(name: &str) -> [u8; 11] {
        short_name(name.as_bytes()).unwrap()
    }

    #[test_case]
    fn parse_aceita_caminhos_validos_e_normaliza() {
        let p = parse(b"/RAM/OLA.TXT").unwrap();
        assert_eq!(p.volume, VolumeId::Ram);
        assert_eq!(p.components(), &[comp("ola.txt")]);
        let p = parse(b"/disco/docs/longo.txt").unwrap();
        assert_eq!(p.volume, VolumeId::Disco);
        assert_eq!(p.components(), &[comp("docs"), comp("longo.txt")]);
        assert_eq!(&comp("ola.txt"), b"OLA     TXT");
    }

    #[test_case]
    fn parse_raiz_do_volume_com_e_sem_barra_final() {
        assert_eq!(parse(b"/ram").unwrap().components().len(), 0);
        assert_eq!(parse(b"/ram/").unwrap().components().len(), 0);
        assert_eq!(parse(b"/ram/docs/").unwrap().components().len(), 1);
    }

    #[test_case]
    fn parse_recusa_caminhos_malformados() {
        assert_eq!(parse(b"ram/x"), Err(FsError::InvalidPath));
        assert_eq!(parse(b""), Err(FsError::InvalidPath));
        assert_eq!(parse(b"/ram//x"), Err(FsError::InvalidPath));
        assert_eq!(parse(b"/ram/.."), Err(FsError::InvalidPath));
        assert_eq!(parse(b"/ram/."), Err(FsError::InvalidPath));
        assert_eq!(parse(b"/ram/docs/../ola.txt"), Err(FsError::InvalidPath));
        assert_eq!(parse(b"/ram/a b"), Err(FsError::InvalidPath));
        assert_eq!(parse(b"/ram/NOMELONGO1.TXT"), Err(FsError::InvalidPath));
        assert_eq!(parse(b"/ram/a.txtx"), Err(FsError::InvalidPath));
        assert_eq!(parse(b"/ram/a.b.c"), Err(FsError::InvalidPath));
        assert_eq!(parse(b"/ram/\xff"), Err(FsError::InvalidPath));
    }

    #[test_case]
    fn parse_volume_desconhecido() {
        assert_eq!(parse(b"/xyz/a"), Err(FsError::UnknownVolume));
        assert_eq!(parse(b"/"), Err(FsError::UnknownVolume));
        assert_eq!(parse(b"/ramx/a"), Err(FsError::UnknownVolume));
    }

    #[test_case]
    fn parse_aplica_os_limites_de_tamanho() {
        // Exatamente 64 bytes: aceito (volume, e componentes de 1 a 3 letras).
        let mut ok = [b'a'; MAX_PATH_LEN];
        ok[..5].copy_from_slice(b"/ram/");
        for i in (6..MAX_PATH_LEN).step_by(2) {
            ok[i] = b'/';
        }
        // 5 + componentes separados por '/': pode haver mais de 8 componentes.
        let r = parse(&ok);
        assert!(r == Err(FsError::PathTooLong) || r.is_ok());
        let mut long = [b'a'; MAX_PATH_LEN + 1];
        long[..5].copy_from_slice(b"/ram/");
        assert_eq!(parse(&long), Err(FsError::PathTooLong));
        // 9 componentes.
        assert_eq!(parse(b"/ram/a/b/c/d/e/f/g/h/i"), Err(FsError::PathTooLong));
        // 8 componentes: aceito.
        assert_eq!(parse(b"/ram/a/b/c/d/e/f/g/h").unwrap().components().len(), 8);
        // Exatamente 64 bytes, com poucos componentes.
        let mut path = [b'x'; MAX_PATH_LEN];
        path[..5].copy_from_slice(b"/ram/");
        path[5..13].copy_from_slice(b"aaaaaaaa");
        path[13] = b'/';
        path[14..22].copy_from_slice(b"bbbbbbbb");
        path[22] = b'/';
        path[23..31].copy_from_slice(b"cccccccc");
        path[31] = b'/';
        path[32..40].copy_from_slice(b"dddddddd");
        path[40] = b'/';
        path[41..49].copy_from_slice(b"eeeeeeee");
        path[49] = b'/';
        path[50..58].copy_from_slice(b"ffffffff");
        path[58] = b'/';
        path[59..64].copy_from_slice(b"ggggg");
        assert_eq!(path.len(), MAX_PATH_LEN);
        assert_eq!(parse(&path).unwrap().components().len(), 7);
    }

    #[test_case]
    fn le_o_arquivo_do_ramdisk_inteiro() {
        let data = read_whole(b"/ram/ola.txt", MAX_EXEC_SIZE).unwrap();
        assert_eq!(&data[..], &include_bytes!("../discos/ram/ola.txt")[..]);
        let data = read_whole(b"/RAM/DOCS/SOBRE.TXT", MAX_EXEC_SIZE).unwrap();
        assert_eq!(&data[..], &include_bytes!("../discos/ram/docs/sobre.txt")[..]);
    }

    #[test_case]
    fn arquivo_vazio_le_zero_bytes() {
        assert_eq!(read_whole(b"/ram/vazio.txt", MAX_EXEC_SIZE).unwrap().len(), 0);
    }

    #[test_case]
    fn read_whole_recusa_diretorio_e_arquivo_grande_demais() {
        assert_eq!(read_whole(b"/ram/docs", MAX_EXEC_SIZE), Err(FsError::IsADirectory));
        let size = include_bytes!("../discos/ram/ola.txt").len();
        assert_eq!(read_whole(b"/ram/ola.txt", 3), Err(FsError::TooBig { size, max: 3 }));
        assert_eq!(read_whole(b"/ram/nada", MAX_EXEC_SIZE), Err(FsError::NotFound));
        assert_eq!(read_whole(b"/ram/ola.txt/x", MAX_EXEC_SIZE), Err(FsError::NotADirectory));
    }

    #[test_case]
    fn leitura_em_pedacos_iguala_a_leitura_inteira() {
        let mut file = OpenFile::open(b"/ram/docs/sobre.txt").unwrap();
        let mut all = Vec::new();
        let mut chunk = [0u8; 3];
        loop {
            let n = file.read(&mut chunk).unwrap();
            if n == 0 {
                break;
            }
            all.extend_from_slice(&chunk[..n]);
        }
        assert_eq!(&all[..], &include_bytes!("../discos/ram/docs/sobre.txt")[..]);
    }

    #[test_case]
    fn lista_a_raiz_do_ramdisk() {
        let mut dir = OpenFile::open(b"/ram").unwrap();
        let mut names = Vec::new();
        while let Some(entry) = dir.next_entry().unwrap() {
            names.push(alloc::string::String::from(entry.name_str()));
        }
        assert!(names.iter().any(|n| n == "ola.txt"));
        assert!(names.iter().any(|n| n == "docs"));
        assert!(names.iter().any(|n| n == "vazio.txt"));
        assert!(names.iter().any(|n| n == "bin"));
        // Ler arquivo como diretório e diretório como arquivo é tipo errado.
        assert_eq!(OpenFile::open(b"/ram/ola.txt").unwrap().next_entry().err(), Some(FsError::NotADirectory));
        assert_eq!(OpenFile::open(b"/ram").unwrap().read(&mut [0u8; 4]).err(), Some(FsError::IsADirectory));
    }

    #[test_case]
    fn a_tabela_tem_quatro_posicoes_e_reaproveita_a_menor_livre() {
        let antes = open_files_in_use();
        let mut table = FileTable::new();
        for expected in 0..MAX_OPEN_FILES {
            assert_eq!(table.open(b"/ram/ola.txt"), Ok(expected));
        }
        assert_eq!(open_files_in_use(), antes + MAX_OPEN_FILES);
        // A quinta não cabe.
        assert_eq!(table.open(b"/ram/ola.txt"), Err(FsError::TooManyOpen));
        // Um caminho ruim é reportado como tal, mesmo com a tabela cheia.
        assert_eq!(table.open(b"/ram/nada"), Err(FsError::NotFound));
        // Fechar libera a posição, e a menor livre é a que volta.
        assert_eq!(table.close(2), Ok(()));
        assert_eq!(table.close(0), Ok(()));
        assert_eq!(table.open(b"/ram/docs"), Ok(0));
        assert_eq!(table.open(b"/ram/docs"), Ok(2));
        drop(table);
        assert_eq!(open_files_in_use(), antes, "o Drop fecha o que sobrou");
    }

    #[test_case]
    fn descritor_invalido_ou_fechado_e_bad_descriptor() {
        let mut table = FileTable::new();
        assert!(table.get_mut(0).is_err());
        assert_eq!(table.get_mut(MAX_OPEN_FILES).err(), Some(FsError::BadDescriptor));
        assert_eq!(table.get_mut(usize::MAX).err(), Some(FsError::BadDescriptor));
        let fd = table.open(b"/ram/ola.txt").unwrap();
        assert!(table.get_mut(fd).is_ok());
        assert_eq!(table.close(fd), Ok(()));
        assert_eq!(table.close(fd), Err(FsError::BadDescriptor));
        assert_eq!(table.get_mut(fd).err(), Some(FsError::BadDescriptor));
        assert_eq!(table.close(MAX_OPEN_FILES), Err(FsError::BadDescriptor));
    }

    #[test_case]
    fn cada_tabela_tem_as_suas_proprias_posicoes_de_leitura() {
        let mut a = FileTable::new();
        let mut b = FileTable::new();
        let fa = a.open(b"/ram/ola.txt").unwrap();
        let fb = b.open(b"/ram/ola.txt").unwrap();
        let mut buf = [0u8; 4];
        assert_eq!(a.get_mut(fa).unwrap().read(&mut buf), Ok(4));
        // A leitura de `a` não moveu a de `b`: `b` ainda começa do início.
        let mut first = [0u8; 4];
        assert_eq!(b.get_mut(fb).unwrap().read(&mut first), Ok(4));
        assert_eq!(&first, &buf);
    }

    #[test_case]
    fn arquivo_aberto_como_diretorio_e_vice_versa_e_tipo_errado() {
        let mut table = FileTable::new();
        let file = table.open(b"/ram/ola.txt").unwrap();
        let dir = table.open(b"/ram").unwrap();
        assert_eq!(table.get_mut(file).unwrap().next_entry().err(), Some(FsError::NotADirectory));
        assert_eq!(table.get_mut(dir).unwrap().read(&mut [0u8; 4]).err(), Some(FsError::IsADirectory));
    }

    #[test_case]
    fn erros_do_ata_viram_motivos_de_volume_indisponivel() {
        assert_eq!(reason_for(AtaError::NoDevice), Reason::NoDisk);
        assert_eq!(reason_for(AtaError::NotAta), Reason::NotAta);
        assert_eq!(reason_for(AtaError::Timeout), Reason::IoError);
        assert_eq!(reason_for(AtaError::DeviceError), Reason::IoError);
    }

    #[test_case]
    fn volume_indisponivel_e_reportado_antes_do_caminho() {
        assert_eq!(volume_status(VolumeId::Ram), Ok(()));
        let old = replace_volume(VolumeId::Disco, VolumeState::Unavailable(Reason::NoDisk));
        assert_eq!(volume_status(VolumeId::Disco), Err(Reason::NoDisk));
        assert_eq!(lookup(b"/disco/qualquer").err(), Some(FsError::Unavailable(Reason::NoDisk)));
        replace_volume(VolumeId::Disco, old);
    }
}
