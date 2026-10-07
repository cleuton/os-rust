//! Arquivos: abrir, ler e listar, somente leitura.
//!
//! Um programa nunca fala com o disco: ele pede ao kernel, por syscalls
//! (`SYSCALLS.md`, seção "Sistema de arquivos"), e o kernel lê do volume. Esta
//! biblioteca esconde as syscalls atrás de tipos de Rust: [`File`] e [`Dir`]
//! fecham o descritor sozinhos quando saem de escopo, e os códigos de erro
//! viram [`FsError`].
//!
//! Caminhos: `/<volume>/<componente>/...`, com os volumes `/ram` e `/disco`,
//! nomes 8.3 (até 8 caracteres, ponto e até 3 de extensão), sem diferenciar
//! maiúsculas de minúsculas, no máximo [`abi::MAX_PATH_LEN`] bytes. Cada
//! programa pode ter [`abi::MAX_OPEN_FILES`] arquivos abertos ao mesmo tempo.

use abi::{
    DIR_ENTRY_SIZE, ERR_BADF, ERR_FAULT, ERR_INVAL, ERR_IO, ERR_MFILE, ERR_NAMETOOLONG,
    ERR_NODEV, ERR_NOENT, ERR_TYPE, KIND_DIR,
};

use crate::sys;

/// Por que uma operação de arquivo falhou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    /// O caminho (ou um componente dele) não existe.
    NotFound,
    /// O volume não existe ou não está disponível (por exemplo, sem disco).
    NoVolume,
    /// Tipo errado: ler um diretório como arquivo, listar um arquivo, ou um
    /// arquivo no meio do caminho.
    WrongType,
    /// Descritor inválido (não deveria acontecer com [`File`] e [`Dir`]).
    BadDescriptor,
    /// Já há [`abi::MAX_OPEN_FILES`] arquivos abertos.
    TooManyOpen,
    /// Caminho com mais de [`abi::MAX_PATH_LEN`] bytes, ou fundo demais.
    PathTooLong,
    /// Erro ao ler o volume (disco que não responde ou volume corrompido).
    Io,
    /// Caminho malformado: componente vazio, caractere inválido, nome fora de
    /// 8.3, `.` ou `..`, ou sem `/` inicial.
    Invalid,
    /// Ponteiro inválido passado ao kernel (não acontece com fatias vivas).
    Fault,
}

impl FsError {
    /// Traduz um código de erro (`< 0`) de syscall de arquivo.
    pub fn from_code(code: i64) -> FsError {
        match code {
            ERR_NOENT => FsError::NotFound,
            ERR_NODEV => FsError::NoVolume,
            ERR_TYPE => FsError::WrongType,
            ERR_BADF => FsError::BadDescriptor,
            ERR_MFILE => FsError::TooManyOpen,
            ERR_NAMETOOLONG => FsError::PathTooLong,
            ERR_IO => FsError::Io,
            ERR_INVAL => FsError::Invalid,
            ERR_FAULT => FsError::Fault,
            // Um código que este contrato não conhece (de uma versão futura).
            _ => FsError::Io,
        }
    }
}

impl core::fmt::Display for FsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let text = match self {
            FsError::NotFound => "nao encontrado",
            FsError::NoVolume => "volume indisponivel",
            FsError::WrongType => "tipo errado (arquivo ou diretorio)",
            FsError::BadDescriptor => "descritor invalido",
            FsError::TooManyOpen => "arquivos abertos demais",
            FsError::PathTooLong => "caminho longo demais",
            FsError::Io => "erro de leitura",
            FsError::Invalid => "caminho invalido",
            FsError::Fault => "ponteiro invalido",
        };
        f.write_str(text)
    }
}

/// `Ok(valor)` se o resultado de uma syscall é `≥ 0`; senão o erro.
fn check(result: i64) -> Result<i64, FsError> {
    if result < 0 {
        Err(FsError::from_code(result))
    } else {
        Ok(result)
    }
}

/// Um arquivo aberto para leitura. Fecha o descritor ao sair de escopo.
pub struct File {
    fd: i64,
}

impl File {
    /// Abre o arquivo `path`. Abrir um diretório com `File` funciona, mas a
    /// primeira leitura devolve [`FsError::WrongType`]: para listar, use [`Dir`].
    pub fn open(path: &str) -> Result<File, FsError> {
        Ok(File { fd: check(sys::open(path.as_bytes()))? })
    }

    /// Lê até `buf.len()` bytes, a partir da posição atual, e avança a
    /// posição. Devolve quantos bytes leu; `0` quer dizer que o arquivo
    /// acabou. `buf` deve ter no máximo [`abi::IO_MAX_LEN`] bytes.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, FsError> {
        Ok(check(sys::read(self.fd, buf))? as usize)
    }
}

impl Drop for File {
    fn drop(&mut self) {
        // Nada a fazer se o fechamento falhar: o kernel também libera tudo
        // quando o programa termina.
        sys::close(self.fd);
    }
}

/// Um diretório aberto para listagem. Fecha o descritor ao sair de escopo.
pub struct Dir {
    fd: i64,
}

impl Dir {
    /// Abre o diretório `path`.
    pub fn open(path: &str) -> Result<Dir, FsError> {
        Ok(Dir { fd: check(sys::open(path.as_bytes()))? })
    }

    /// A próxima entrada, ou `None` quando as entradas acabaram. As entradas
    /// vêm na ordem do diretório, sem `.` nem `..`.
    pub fn next(&mut self) -> Result<Option<DirEntry>, FsError> {
        let mut raw = [0u8; DIR_ENTRY_SIZE];
        if check(sys::read_dir(self.fd, &mut raw))? == 0 {
            return Ok(None);
        }
        let mut name = [0u8; 12];
        name.copy_from_slice(&raw[..12]);
        // O layout é o de `abi::DirEntryRaw` (20 bytes): 12 de nome, 1 de
        // tipo, 3 de zeros e 4 de tamanho (little-endian).
        let kind = raw[12];
        let size = u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]]);
        Ok(Some(DirEntry { name, kind, size }))
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        sys::close(self.fd);
    }
}

/// Uma entrada de diretório.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirEntry {
    name: [u8; 12],
    kind: u8,
    size: u32,
}

impl DirEntry {
    /// O nome (`nome.ext`, em minúsculas).
    pub fn name(&self) -> &str {
        let len = self.name.iter().position(|&b| b == 0).unwrap_or(self.name.len());
        core::str::from_utf8(&self.name[..len]).unwrap_or("?")
    }

    /// Verdadeiro se for um diretório.
    pub fn is_dir(&self) -> bool {
        self.kind == KIND_DIR
    }

    /// Tamanho em bytes (`0` para diretório).
    pub fn size(&self) -> u32 {
        self.size
    }
}
