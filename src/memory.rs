//! Memória física: mapa de memória do bootloader, alocador de frames de
//! 4 KiB, e tradução/criação de mapeamentos na tabela de páginas ativa.
//!
//! Ver `research.md` (Marco 3) para a justificativa de cada decisão e
//! `contracts/kernel-memory-api.md` para a superfície pública exposta
//! aqui.

use bootloader::bootinfo::{MemoryMap, MemoryRegionType};
use spin::Mutex;
use x86_64::{
    structures::paging::{
        mapper::MapToError, FrameAllocator, Mapper, OffsetPageTable, Page, PageTable,
        PageTableFlags, PhysFrame, Size4KiB, Translate,
    },
    PhysAddr, VirtAddr,
};

/// Alocador de frames físicos de 4 KiB a partir do mapa de memória do
/// bootloader (FR-002, FR-003, FR-004; `research.md` seção 3).
///
/// Cada instância percorre o mapa desde o início: a garantia de "nunca
/// repetir um frame" vale por instância, não como reserva global entre
/// instâncias distintas (ver `data-model.md`, "BootInfoFrameAllocator").
pub struct BootInfoFrameAllocator {
    memory_map: &'static MemoryMap,
    next: usize,
}

impl BootInfoFrameAllocator {
    /// Cria um alocador que só entrega frames de regiões `Usable`.
    ///
    /// SAFETY: o chamador garante que `memory_map` é válido, isto é, veio
    /// de fato de um `BootInfo` entregue pelo bootloader — só assim as
    /// regiões que ele descreve correspondem à memória física real da
    /// máquina.
    pub unsafe fn init(memory_map: &'static MemoryMap) -> Self {
        BootInfoFrameAllocator {
            memory_map,
            next: 0,
        }
    }

    /// Itera todos os frames de 4 KiB de todas as regiões `Usable` do
    /// mapa, na ordem em que aparecem.
    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame<Size4KiB>> {
        self.memory_map
            .iter()
            .filter(|region| region.region_type == MemoryRegionType::Usable)
            .flat_map(|region| region.range.start_frame_number..region.range.end_frame_number)
            .map(|frame_number| {
                PhysFrame::containing_address(PhysAddr::new(frame_number * 4096))
            })
    }
}

// SAFETY: `usable_frames` só devolve frames de regiões `Usable` do mapa,
// e `allocate_frame` nunca devolve o mesmo índice `next` duas vezes numa
// mesma instância — as duas garantias que este trait exige do
// implementador (frames únicos, nunca usados por outra estrutura ao
// mesmo tempo, já que nenhum outro código do kernel também lê deste
// mesmo `memory_map` para entregar frames).
unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let frame = self.usable_frames().nth(self.next);
        self.next += 1;
        frame
    }
}

/// Devolve uma referência mutável para a tabela de páginas de nível 4
/// ativa (a que o registrador `CR3` aponta), usando o mapeamento
/// completo da memória física.
///
/// SAFETY: o chamador garante que `physical_memory_offset` é o valor
/// real informado pelo `BootInfo` do bootloader — só assim o endereço
/// virtual calculado (`físico da P4` + deslocamento) aponta de fato para
/// a P4 ativa. O chamador também garante que esta função não é chamada
/// de forma concorrente com outra chamada que também obtenha acesso
/// mutável à mesma tabela (este projeto não tem SMP, então não há outra
/// CPU que poderia fazer isso).
unsafe fn active_level_4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (level_4_table_frame, _) = Cr3::read();
    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    // SAFETY: `virt` aponta para a P4 ativa (ver comentário da função);
    // como só existe uma referência mutável para essa tabela em uso por
    // vez no fluxo do boot (ver invariante acima), desreferenciar o
    // ponteiro aqui não viola a regra de aliasing de `&mut`.
    unsafe { &mut *page_table_ptr }
}

/// Traduz um endereço virtual para o endereço físico correspondente,
/// usando a tabela de páginas ativa (FR-007).
///
/// SAFETY: mesma exigência de `active_level_4_table` sobre
/// `physical_memory_offset`.
pub unsafe fn translate_addr(addr: VirtAddr, physical_memory_offset: VirtAddr) -> Option<PhysAddr> {
    // SAFETY: repassa a mesma garantia documentada pelo chamador desta
    // função.
    let level_4_table = unsafe { active_level_4_table(physical_memory_offset) };
    // SAFETY: `physical_memory_offset` é o valor real do `BootInfo` (a
    // mesma garantia acima) — é exatamente o que `OffsetPageTable::new`
    // exige do chamador.
    let mapper = unsafe { OffsetPageTable::new(level_4_table, physical_memory_offset) };
    mapper.translate_addr(addr)
}

/// Mapeia uma página virtual de 4 KiB, ainda não mapeada, em um frame
/// físico livre obtido de `frame_allocator`, com as flags `PRESENT` e
/// `WRITABLE` — nunca `USER_ACCESSIBLE` (FR-008, FR-009).
///
/// Devolve `Err(MapToError::PageAlreadyMapped(_))` se `page` já tinha
/// mapeamento, e `Err(MapToError::FrameAllocationFailed)` se
/// `frame_allocator` não tinha mais frames a oferecer — nunca mapeia
/// parcialmente.
///
/// SAFETY: mesma exigência de `active_level_4_table` sobre
/// `physical_memory_offset`; o chamador também garante que `page` faz
/// sentido mapear (não se sobrepõe a nenhuma estrutura já em uso pelo
/// kernel).
pub unsafe fn map_page(
    page: Page<Size4KiB>,
    physical_memory_offset: VirtAddr,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    // SAFETY: mesma garantia repassada pelo chamador desta função.
    let level_4_table = unsafe { active_level_4_table(physical_memory_offset) };
    // SAFETY: mesma garantia de `physical_memory_offset` documentada acima.
    let mut mapper = unsafe { OffsetPageTable::new(level_4_table, physical_memory_offset) };

    let frame = frame_allocator
        .allocate_frame()
        .ok_or(MapToError::FrameAllocationFailed)?;
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

    // SAFETY: `frame` acabou de ser obtido do `frame_allocator` (ainda
    // não está em uso por mais ninguém) e `page` é, por contrato desta
    // função, uma página que o chamador garante não se sobrepor a
    // nenhuma estrutura já em uso — as duas condições que `map_to` exige
    // para não violar memory safety.
    let flush = unsafe { mapper.map_to(page, frame, flags, frame_allocator)? };
    flush.flush();
    Ok(())
}

/// Resumo somente-leitura de memória, calculado uma única vez no boot e
/// usado pelo comando `mem` do prompt (FR-017).
#[derive(Clone, Copy)]
pub struct MemoryInfo {
    pub usable_bytes: u64,
    pub heap_start: usize,
    pub heap_size: usize,
}

static MEMORY_INFO: Mutex<Option<MemoryInfo>> = Mutex::new(None);

/// Devolve o resumo de memória calculado por `init`.
///
/// Só é seguro chamar depois que `init` (via `proto_os::init`) já
/// retornou — a ordem de boot garante isso no fluxo normal (ver Edge
/// Case "uso do heap antes da inicialização").
pub fn info() -> MemoryInfo {
    MEMORY_INFO
        .lock()
        .as_ref()
        .copied()
        .expect("memoria nao inicializada")
}

/// Orquestra a inicialização de memória do boot: mapeia o heap inteiro
/// em frames físicos livres e inicializa o alocador global, antes de
/// calcular e guardar o resumo lido por `info()`.
///
/// Chamada uma única vez, por `proto_os::init`, depois de
/// `interrupts::init()`.
pub(crate) fn init(boot_info: &'static bootloader::BootInfo) {
    let physical_memory_offset = VirtAddr::new(boot_info.physical_memory_offset);

    // SAFETY: `physical_memory_offset` veio do `BootInfo` real entregue
    // pelo bootloader (a feature `map_physical_memory` está habilitada
    // em `Cargo.toml`), e esta é a única chamada a `active_level_4_table`
    // em andamento neste momento do boot.
    let level_4_table = unsafe { active_level_4_table(physical_memory_offset) };
    // SAFETY: mesma garantia de `physical_memory_offset` acima.
    let mut mapper = unsafe { OffsetPageTable::new(level_4_table, physical_memory_offset) };

    // SAFETY: `boot_info.memory_map` veio do mesmo `BootInfo` real
    // entregue pelo bootloader.
    let mut frame_allocator = unsafe { BootInfoFrameAllocator::init(&boot_info.memory_map) };

    crate::allocator::init_heap(&mut mapper, &mut frame_allocator)
        .expect("falha ao mapear o heap: memoria fisica insuficiente");

    let usable_bytes: u64 = boot_info
        .memory_map
        .iter()
        .filter(|region| region.region_type == MemoryRegionType::Usable)
        .map(|region| (region.range.end_frame_number - region.range.start_frame_number) * 4096)
        .sum();

    *MEMORY_INFO.lock() = Some(MemoryInfo {
        usable_bytes,
        heap_start: crate::allocator::HEAP_START,
        heap_size: crate::allocator::HEAP_SIZE,
    });
}
