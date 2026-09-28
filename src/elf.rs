//! Leitor de executáveis ELF64 estáticos.
//!
//! Só entende o que os programas de usuário deste projeto produzem: um ELF64
//! little-endian, `ET_EXEC`, para x86-64, sem relocação, com segmentos
//! `PT_LOAD` alinhados a 4 KiB dentro da região do usuário (`SYSCALLS.md`,
//! seção "Formato do executável"). Tudo mais é recusado com um `LoadError`
//! antes de qualquer página ser mapeada. É uma função pura: lê um `&[u8]`
//! e não mexe em memória nem em hardware, então se testa sem QEMU extra.
//!
//! Escrito à mão (sem a crate `xmas-elf`) porque para este ELF mínimo a
//! leitura direta é mais curta que a integração com uma crate, e é
//! justamente o assunto da aula: os campos abaixo são os campos reais do
//! formato.

use core::fmt;

use crate::user::{USER_HEAP_START, USER_REGION_START};

/// Máximo de segmentos `PT_LOAD` aceitos. O carregador guarda os intervalos
/// que mapeou num array de tamanho fixo (sem heap), para poder desfazer o
/// mapeamento em qualquer ponto de falha.
pub const MAX_SEGMENTS: usize = 8;

/// Tamanho de uma página, e também o alinhamento exigido dos segmentos.
const PAGE_SIZE: u64 = 4096;

/// Tamanho do cabeçalho ELF64 e de cada *program header*.
const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;

const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 62;
const PT_LOAD: u32 = 1;

/// Bits de `p_flags` de um segmento.
const PF_X: u32 = 1;
const PF_W: u32 = 2;

/// Fim (exclusivo) da área que os segmentos podem ocupar: o começo do heap do
/// programa (`USER_HEAP_START`). Código e dados ficam em
/// `[USER_REGION_START, USER_HEAP_START)`; um segmento que passasse daí cairia
/// em cima do heap, que só existe por `SYS_ALLOC`. A pilha, mais acima ainda,
/// fica fora do alcance por consequência.
const SEGMENTS_END: u64 = USER_HEAP_START;

/// Por que um executável foi recusado (ou por que a carga falhou). Devolvido
/// antes de qualquer página do usuário ser mapeada, ou depois de desfazer os
/// mapeamentos já feitos: nunca sobra mapeamento pela metade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    /// Não começa com a magia `7f 45 4c 46` (ou é menor que o cabeçalho).
    NotElf,
    /// É um ELF, mas não ELF64 little-endian `ET_EXEC` para x86-64.
    Unsupported,
    /// Estrutura interna inconsistente: tabela de *program headers* ou
    /// segmento fora do arquivo, `p_filesz > p_memsz`, `p_vaddr` desalinhado
    /// ou segmentos demais.
    Malformed,
    /// Algum segmento não cabe inteiro na faixa de código e dados do usuário
    /// (`[USER_REGION_START, USER_HEAP_START)`): fora da região, ou invadindo
    /// o heap ou a pilha.
    OutOfRegion,
    /// Dois segmentos usam a mesma página.
    Overlap,
    /// O ponto de entrada não está dentro de um segmento executável.
    BadEntry,
    /// No primeiro uso, a região do usuário já estava ocupada por outro dono.
    RegionBusy,
    /// O alocador de frames não tinha mais frames.
    OutOfFrames,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let message = match self {
            LoadError::NotElf => "nao e um executavel ELF",
            LoadError::Unsupported => "so ELF64 estatico (ET_EXEC) para x86-64 e aceito",
            LoadError::Malformed => "executavel com estrutura invalida",
            LoadError::OutOfRegion => "segmento fora da regiao de memoria do usuario",
            LoadError::Overlap => "dois segmentos usam a mesma pagina",
            LoadError::BadEntry => "ponto de entrada fora de um segmento executavel",
            LoadError::RegionBusy => "regiao de memoria do usuario ja esta ocupada",
            LoadError::OutOfFrames => "memoria fisica insuficiente",
        };
        f.write_str(message)
    }
}

/// Permissões de um segmento, lidas de `p_flags`. Viram os bits `WRITABLE`
/// e `NO_EXECUTE` das páginas (W^X): leitura é sempre permitida.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentFlags {
    pub write: bool,
    pub execute: bool,
}

/// Um segmento `PT_LOAD`: onde ele vai na memória do usuário e o que copiar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadSegment<'a> {
    /// Endereço virtual do início (alinhado a 4 KiB, dentro da região).
    pub vaddr: u64,
    /// Bytes a copiar (`p_filesz`), tirados do próprio arquivo.
    pub file_bytes: &'a [u8],
    /// Tamanho total na memória (`p_memsz`); o que passa de `file_bytes` é
    /// zerado (a `.bss`).
    pub mem_size: u64,
    pub flags: SegmentFlags,
}

impl<'a> LoadSegment<'a> {
    const EMPTY: LoadSegment<'static> = LoadSegment {
        vaddr: 0,
        file_bytes: &[],
        mem_size: 0,
        flags: SegmentFlags {
            write: false,
            execute: false,
        },
    };

    /// Primeiro endereço depois do fim do segmento, arredondado para cima
    /// até o fim da página.
    pub fn end_page_aligned(&self) -> u64 {
        (self.vaddr + self.mem_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
    }
}

/// Resultado de `parse`: o ponto de entrada e os segmentos a carregar.
/// Guarda os segmentos num array de tamanho fixo, sem alocar.
#[derive(Debug, Clone, Copy)]
pub struct ElfImage<'a> {
    /// `e_entry`: endereço da primeira instrução (dentro de um segmento
    /// executável).
    pub entry: u64,
    segments: [LoadSegment<'a>; MAX_SEGMENTS],
    count: usize,
}

impl<'a> ElfImage<'a> {
    /// Os segmentos `PT_LOAD` (não vazios), na ordem em que aparecem.
    pub fn segments(&self) -> &[LoadSegment<'a>] {
        &self.segments[..self.count]
    }
}

fn read_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn read_u64(bytes: &[u8], at: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(raw)
}

/// Lê e valida um executável ELF64 estático. Não mapeia nada: só devolve o
/// que o carregador (`user.rs`) precisa. As regras estão em `SYSCALLS.md`.
pub fn parse(bytes: &[u8]) -> Result<ElfImage<'_>, LoadError> {
    // Cabeçalho ELF64: magia (0..4), classe (4), ordem dos bytes (5), tipo
    // (16), máquina (18), entrada (24), tabela de program headers (32, 54,
    // 56). Os acessos abaixo só acontecem depois desta conferência de tamanho.
    if bytes.len() < ELF_HEADER_SIZE || bytes[0..4] != [0x7f, b'E', b'L', b'F'] {
        return Err(LoadError::NotElf);
    }
    let is_elf64 = bytes[4] == 2;
    let is_little_endian = bytes[5] == 1;
    if !is_elf64
        || !is_little_endian
        || read_u16(bytes, 16) != ET_EXEC
        || read_u16(bytes, 18) != EM_X86_64
    {
        return Err(LoadError::Unsupported);
    }

    let entry = read_u64(bytes, 24);
    let phoff = read_u64(bytes, 32);
    let phentsize = usize::from(read_u16(bytes, 54));
    let phnum = usize::from(read_u16(bytes, 56));
    if phentsize != PROGRAM_HEADER_SIZE {
        return Err(LoadError::Malformed);
    }
    let table_start = usize::try_from(phoff).map_err(|_| LoadError::Malformed)?;
    let table_end = phnum
        .checked_mul(PROGRAM_HEADER_SIZE)
        .and_then(|size| size.checked_add(table_start))
        .ok_or(LoadError::Malformed)?;
    if table_end > bytes.len() {
        return Err(LoadError::Malformed);
    }

    let mut image = ElfImage {
        entry,
        segments: [LoadSegment::EMPTY; MAX_SEGMENTS],
        count: 0,
    };

    for index in 0..phnum {
        // Program header: tipo (0), flags (4), offset no arquivo (8),
        // endereço virtual (16), tamanho no arquivo (32), tamanho na
        // memória (40).
        let header = table_start + index * PROGRAM_HEADER_SIZE;
        if read_u32(bytes, header) != PT_LOAD {
            continue;
        }
        let p_flags = read_u32(bytes, header + 4);
        let p_offset = read_u64(bytes, header + 8);
        let vaddr = read_u64(bytes, header + 16);
        let file_size = read_u64(bytes, header + 32);
        let mem_size = read_u64(bytes, header + 40);

        if mem_size == 0 {
            continue;
        }
        if image.count == MAX_SEGMENTS
            || file_size > mem_size
            || vaddr % PAGE_SIZE != 0
        {
            return Err(LoadError::Malformed);
        }
        let file_start = usize::try_from(p_offset).map_err(|_| LoadError::Malformed)?;
        let file_end = usize::try_from(file_size)
            .ok()
            .and_then(|size| size.checked_add(file_start))
            .ok_or(LoadError::Malformed)?;
        if file_end > bytes.len() {
            return Err(LoadError::Malformed);
        }
        let end = vaddr.checked_add(mem_size).ok_or(LoadError::OutOfRegion)?;
        let end_page_aligned = end
            .checked_add(PAGE_SIZE - 1)
            .ok_or(LoadError::OutOfRegion)?
            & !(PAGE_SIZE - 1);
        if vaddr < USER_REGION_START || end_page_aligned > SEGMENTS_END {
            return Err(LoadError::OutOfRegion);
        }

        let segment = LoadSegment {
            vaddr,
            file_bytes: &bytes[file_start..file_end],
            mem_size,
            flags: SegmentFlags {
                write: p_flags & PF_W != 0,
                execute: p_flags & PF_X != 0,
            },
        };
        // Duas regiões [a, b) e [c, d) se sobrepõem se a < d e c < b; como
        // ambas são múltiplos de página, sobrepor é dividir uma página.
        for other in image.segments() {
            if vaddr < other.end_page_aligned() && other.vaddr < end_page_aligned {
                return Err(LoadError::Overlap);
            }
        }
        image.segments[image.count] = segment;
        image.count += 1;
    }

    let entry_is_executable = image
        .segments()
        .iter()
        .any(|s| s.flags.execute && entry >= s.vaddr && entry < s.vaddr + s.mem_size);
    if !entry_is_executable {
        return Err(LoadError::BadEntry);
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::USER_STACK_BOTTOM;
    use alloc::vec::Vec;

    /// Um segmento para o construtor de ELF dos testes.
    struct Seg<'a> {
        vaddr: u64,
        flags: u32,
        data: &'a [u8],
        mem_size: u64,
    }

    /// Monta um ELF64 mínimo válido (cabeçalho, tabela de program headers,
    /// depois os dados de cada segmento em sequência).
    fn build(entry: u64, segs: &[Seg]) -> Vec<u8> {
        let table_end = ELF_HEADER_SIZE + segs.len() * PROGRAM_HEADER_SIZE;
        let mut file = alloc::vec![0u8; table_end];
        file[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        file[4] = 2; // ELF64
        file[5] = 1; // little-endian
        file[6] = 1; // versão
        file[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
        file[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
        file[24..32].copy_from_slice(&entry.to_le_bytes());
        file[32..40].copy_from_slice(&(ELF_HEADER_SIZE as u64).to_le_bytes());
        file[54..56].copy_from_slice(&(PROGRAM_HEADER_SIZE as u16).to_le_bytes());
        file[56..58].copy_from_slice(&(segs.len() as u16).to_le_bytes());
        for (i, seg) in segs.iter().enumerate() {
            let offset = file.len() as u64;
            file.extend_from_slice(seg.data);
            let h = ELF_HEADER_SIZE + i * PROGRAM_HEADER_SIZE;
            file[h..h + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
            file[h + 4..h + 8].copy_from_slice(&seg.flags.to_le_bytes());
            file[h + 8..h + 16].copy_from_slice(&offset.to_le_bytes());
            file[h + 16..h + 24].copy_from_slice(&seg.vaddr.to_le_bytes());
            file[h + 32..h + 40].copy_from_slice(&(seg.data.len() as u64).to_le_bytes());
            file[h + 40..h + 48].copy_from_slice(&seg.mem_size.to_le_bytes());
        }
        file
    }

    const RX: u32 = 4 | PF_X;
    const RW: u32 = 4 | PF_W;
    const START: u64 = USER_REGION_START;

    fn code() -> Seg<'static> {
        Seg { vaddr: START, flags: RX, data: &[0x0f, 0x0b], mem_size: 2 }
    }

    #[test_case]
    fn elf_valido_e_aceito_com_entrada_e_segmentos_esperados() {
        let dados = Seg { vaddr: START + 0x1000, flags: RW, data: &[1, 2, 3], mem_size: 0x2000 };
        let file = build(START, &[code(), dados]);
        let image = parse(&file).expect("ELF valido");
        assert_eq!(image.entry, START);
        assert_eq!(image.segments().len(), 2);
        assert_eq!(image.segments()[0].vaddr, START);
        assert_eq!(image.segments()[0].file_bytes, &[0x0f, 0x0b]);
        assert!(image.segments()[0].flags.execute && !image.segments()[0].flags.write);
        assert_eq!(image.segments()[1].mem_size, 0x2000);
        assert!(image.segments()[1].flags.write && !image.segments()[1].flags.execute);
    }

    #[test_case]
    fn magia_errada_e_recusada() {
        let mut file = build(START, &[code()]);
        file[0] = 0;
        assert_eq!(parse(&file).unwrap_err(), LoadError::NotElf);
        assert_eq!(parse(&[]).unwrap_err(), LoadError::NotElf);
    }

    #[test_case]
    fn classe_de_32_bits_e_recusada() {
        let mut file = build(START, &[code()]);
        file[4] = 1;
        assert_eq!(parse(&file).unwrap_err(), LoadError::Unsupported);
    }

    #[test_case]
    fn et_dyn_e_recusado() {
        let mut file = build(START, &[code()]);
        file[16..18].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(parse(&file).unwrap_err(), LoadError::Unsupported);
    }

    #[test_case]
    fn maquina_errada_e_recusada() {
        let mut file = build(START, &[code()]);
        file[18..20].copy_from_slice(&40u16.to_le_bytes()); // ARM
        assert_eq!(parse(&file).unwrap_err(), LoadError::Unsupported);
    }

    #[test_case]
    fn filesz_maior_que_memsz_e_recusado() {
        let seg = Seg { vaddr: START, flags: RX, data: &[0x0f, 0x0b, 0x90, 0x90], mem_size: 2 };
        assert_eq!(parse(&build(START, &[seg])).unwrap_err(), LoadError::Malformed);
    }

    #[test_case]
    fn segmento_alem_do_fim_do_arquivo_e_recusado() {
        let mut file = build(START, &[code()]);
        file.truncate(file.len() - 1);
        assert_eq!(parse(&file).unwrap_err(), LoadError::Malformed);
    }

    #[test_case]
    fn vaddr_desalinhado_e_recusado() {
        let seg = Seg { vaddr: START + 0x10, flags: RX, data: &[0x0f, 0x0b], mem_size: 2 };
        assert_eq!(parse(&build(START, &[seg])).unwrap_err(), LoadError::Malformed);
    }

    #[test_case]
    fn segmento_abaixo_da_regiao_do_usuario_e_recusado() {
        let seg = Seg { vaddr: 0x1000, flags: RX, data: &[0x0f, 0x0b], mem_size: 2 };
        assert_eq!(parse(&build(0x1000, &[seg])).unwrap_err(), LoadError::OutOfRegion);
    }

    #[test_case]
    fn segmento_sobre_a_pilha_e_recusado() {
        let seg = Seg { vaddr: USER_STACK_BOTTOM, flags: RX, data: &[0x0f, 0x0b], mem_size: 2 };
        assert_eq!(parse(&build(USER_STACK_BOTTOM, &[seg])).unwrap_err(), LoadError::OutOfRegion);
    }

    #[test_case]
    fn segmento_que_comeca_no_heap_e_recusado() {
        let seg = Seg { vaddr: USER_HEAP_START, flags: RX, data: &[0x0f, 0x0b], mem_size: 2 };
        assert_eq!(parse(&build(USER_HEAP_START, &[seg])).unwrap_err(), LoadError::OutOfRegion);
    }

    #[test_case]
    fn segmento_que_termina_no_comeco_do_heap_e_aceito() {
        // A última página antes do heap ainda é da faixa de código e dados.
        let vaddr = USER_HEAP_START - 4096;
        let seg = Seg { vaddr, flags: RX, data: &[0x0f, 0x0b], mem_size: 2 };
        assert!(parse(&build(vaddr, &[seg])).is_ok());
    }

    #[test_case]
    fn dois_segmentos_na_mesma_pagina_sao_recusados() {
        let outro = Seg { vaddr: START, flags: RW, data: &[1], mem_size: 1 };
        assert_eq!(parse(&build(START, &[code(), outro])).unwrap_err(), LoadError::Overlap);
    }

    #[test_case]
    fn entrada_fora_de_segmento_executavel_e_recusada() {
        let dados = Seg { vaddr: START + 0x1000, flags: RW, data: &[1], mem_size: 1 };
        // Entrada dentro do segmento gravável, que não é executável.
        assert_eq!(
            parse(&build(START + 0x1000, &[code(), dados])).unwrap_err(),
            LoadError::BadEntry
        );
        // Entrada fora de qualquer segmento.
        assert_eq!(parse(&build(START + 0x5000, &[code()])).unwrap_err(), LoadError::BadEntry);
    }

    #[test_case]
    fn mais_de_oito_segmentos_sao_recusados() {
        let mut segs: Vec<Seg> = Vec::new();
        for i in 0..(MAX_SEGMENTS as u64 + 1) {
            segs.push(Seg { vaddr: START + i * 0x1000, flags: RX, data: &[0x90], mem_size: 1 });
        }
        assert_eq!(parse(&build(START, &segs)).unwrap_err(), LoadError::Malformed);
    }

    #[test_case]
    fn programas_embutidos_sao_elf_validos() {
        for program in crate::programs::PROGRAMS {
            assert!(parse(program.image).is_ok(), "{} nao e um ELF valido", program.name);
        }
    }
}
