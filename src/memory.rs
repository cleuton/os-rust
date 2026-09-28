//! Memória física: mapa de memória do bootloader, alocador de frames de
//! 4 KiB, e tradução/criação de mapeamentos na tabela de páginas ativa,
//! inclusive as páginas de programas de usuário (ring 3).
//!
//! A superfície pública deste módulo (`BootInfoFrameAllocator`,
//! `translate_addr`, `map_page`, `init`, `info`, e as funções de usuário
//! `claim_user_region`, `map_user_page`, `unmap_user_range`,
//! `user_page_flags`, `zero_frame`, `fill_frame`) é a única forma, no
//! projeto, de ler o mapa de memória do bootloader e de criar ou
//! traduzir mapeamentos na tabela de páginas ativa.

use alloc::vec::Vec;
use bootloader::bootinfo::{MemoryMap, MemoryRegionType};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use x86_64::{
    structures::paging::{
        mapper::{MapToError, TranslateResult, UnmapError},
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame,
        Size4KiB, Translate,
    },
    PhysAddr, VirtAddr,
};

/// Alocador de frames físicos de 4 KiB a partir do mapa de memória do
/// bootloader: só entrega frames de regiões `Usable`, nunca repete um
/// frame já entregue por si mesma, e devolve `None` quando não há mais
/// frames utilizáveis a oferecer.
///
/// Cada instância percorre o mapa desde o início: a garantia de "nunca
/// repetir um frame" vale por instância, não como reserva global entre
/// instâncias distintas.
pub struct BootInfoFrameAllocator {
    memory_map: &'static MemoryMap,
    next: usize,
    /// Frames devolvidos por `recycle` (páginas de programas de usuário já
    /// encerrados), entregues de novo antes de avançar `next`. Vazia, e
    /// sem alocar nada, até o primeiro `recycle`: o alocador é usado para
    /// montar o próprio heap, antes de o heap existir.
    recycled: Vec<PhysFrame<Size4KiB>>,
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
            recycled: Vec::new(),
        }
    }

    /// Devolve um frame que este alocador entregou antes e que ninguém mais
    /// usa (o chamador acabou de desmapeá-lo), para ser entregue de novo.
    /// Aloca no heap (cresce `recycled`), então só pode ser chamada depois
    /// de o heap existir e nunca de dentro de um handler de exceção.
    pub fn recycle(&mut self, frame: PhysFrame<Size4KiB>) {
        self.recycled.push(frame);
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
// mesmo `memory_map` para entregar frames). Um frame de `recycled` só
// volta a ser entregue depois de `recycle`, que o chamador só invoca
// depois de desmapear o frame: então nunca há dois donos ao mesmo tempo.
unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        if let Some(frame) = self.recycled.pop() {
            return Some(frame);
        }
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
/// usando a tabela de páginas ativa.
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
/// `WRITABLE` — nunca `USER_ACCESSIBLE`: páginas de programas de usuário
/// passam só por `map_user_page`, mais abaixo, que usa o alocador global.
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

/// Estado global de frames: o alocador (que continua de onde o heap parou)
/// e o `physical_memory_offset` do `BootInfo`, necessário para montar um
/// `OffsetPageTable` sobre a tabela de páginas ativa a qualquer momento.
struct FrameState {
    allocator: BootInfoFrameAllocator,
    physical_memory_offset: VirtAddr,
}

static FRAME_STATE: Mutex<Option<FrameState>> = Mutex::new(None);

/// Verdadeiro depois que `claim_user_region` confirmou, uma única vez, que a
/// região do usuário estava livre e que as tabelas de página dela são deste
/// kernel.
static USER_REGION_CLAIMED: AtomicBool = AtomicBool::new(false);

/// Executa `f` com o estado global de frames. Só é seguro chamar depois que
/// `init` já retornou (a ordem de boot garante isso, ver `lib.rs::init`).
fn with_frame_state<R>(f: impl FnOnce(&mut FrameState) -> R) -> R {
    let mut guard = FRAME_STATE.lock();
    let state = guard.as_mut().expect("memoria nao inicializada");
    f(state)
}

/// Monta um `OffsetPageTable` sobre a tabela de páginas ativa.
///
/// SAFETY: `state.physical_memory_offset` é o valor real do `BootInfo`
/// (guardado por `init`), e o chamador segura o lock de `FRAME_STATE` durante
/// todo o uso do mapper devolvido, então não existe outra referência mutável
/// para a P4 em uso (este projeto não tem SMP nem handlers que mexam nela).
unsafe fn active_mapper(state: &FrameState) -> OffsetPageTable<'static> {
    // SAFETY: repassa a garantia documentada acima.
    let level_4_table = unsafe { active_level_4_table(state.physical_memory_offset) };
    // SAFETY: `physical_memory_offset` é o valor real do `BootInfo`, o que
    // `OffsetPageTable::new` exige do chamador.
    unsafe { OffsetPageTable::new(level_4_table, state.physical_memory_offset) }
}

/// Reivindica a região do usuário para este kernel. No **primeiro** uso
/// confere que a entrada P3 da região (dentro da entrada P4 dela) está
/// vazia, isto é, que nem o bootloader nem o kernel a usam, e devolve
/// falso se não estiver. Depois de confirmada, as tabelas de página
/// criadas ali passam a ser deste kernel e permanecem entre execuções
/// (`Mapper::unmap` não libera tabelas), então os usos seguintes devolvem
/// verdadeiro sem olhar de novo.
pub fn claim_user_region() -> bool {
    if USER_REGION_CLAIMED.load(Ordering::Acquire) {
        return true;
    }
    let start = VirtAddr::new(crate::user::USER_REGION_START);
    let free = with_frame_state(|state| {
        // SAFETY: ver `active_mapper`; só lê entradas da P4 e da P3.
        let level_4_table = unsafe { active_level_4_table(state.physical_memory_offset) };
        let p4_entry = &level_4_table[start.p4_index()];
        if p4_entry.is_unused() {
            return true;
        }
        let p3_virt = state.physical_memory_offset + p4_entry.addr().as_u64();
        // SAFETY: `p4_entry` está em uso e aponta para uma tabela P3 real; a
        // janela de memória física a mapeia em `p3_virt`.
        let p3_table: &PageTable = unsafe { &*p3_virt.as_ptr::<PageTable>() };
        p3_table[start.p3_index()].is_unused()
    });
    if free {
        USER_REGION_CLAIMED.store(true, Ordering::Release);
    }
    free
}

/// Mapeia uma página do usuário num frame físico livre (do alocador global,
/// que reaproveita frames de programas encerrados) com as `flags` dadas, e
/// devolve o frame para o carregador preenchê-lo (`zero_frame`/`fill_frame`).
/// `flags` precisa conter `PRESENT` e `USER_ACCESSIBLE`: a crate `x86_64`
/// propaga `USER_ACCESSIBLE` para as tabelas-pai, inclusive a entrada da P4
/// que o kernel já usa; isso não expõe o kernel, porque o acesso de ring 3
/// exige o bit em **todos** os níveis e as páginas do kernel não o têm.
///
/// Se o mapeamento falha (por exemplo, página já mapeada), o frame obtido
/// é devolvido ao alocador em vez de vazar.
pub fn map_user_page(
    page: Page<Size4KiB>,
    flags: PageTableFlags,
) -> Result<PhysFrame<Size4KiB>, MapToError<Size4KiB>> {
    assert!(
        flags.contains(PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE),
        "map_user_page exige PRESENT e USER_ACCESSIBLE"
    );
    with_frame_state(|state| {
        // SAFETY: ver `active_mapper`; o lock de `FRAME_STATE` está preso.
        let mut mapper = unsafe { active_mapper(state) };
        let frame = state
            .allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        // SAFETY: `frame` acabou de sair do alocador (ninguém mais o usa) e
        // `page` está na região do usuário, que só programas carregados por
        // este módulo ocupam: as duas condições que `map_to` exige para não
        // violar memory safety.
        let result = unsafe { mapper.map_to(page, frame, flags, &mut state.allocator) };
        match result {
            Ok(flush) => {
                flush.flush();
                Ok(frame)
            }
            Err(error) => {
                state.allocator.recycle(frame);
                Err(error)
            }
        }
    })
}

/// Desmapeia `pages` páginas a partir de `start` e devolve os frames ao
/// alocador. Páginas que não estavam mapeadas são ignoradas (um carregador
/// que falhou no meio chama isto com o intervalo inteiro que tentou mapear).
pub fn unmap_user_range(start: VirtAddr, pages: u64) {
    with_frame_state(|state| {
        // SAFETY: ver `active_mapper`; o lock de `FRAME_STATE` está preso.
        let mut mapper = unsafe { active_mapper(state) };
        for i in 0..pages {
            let page = Page::<Size4KiB>::containing_address(start + i * 4096);
            match mapper.unmap(page) {
                Ok((frame, flush)) => {
                    flush.flush();
                    state.allocator.recycle(frame);
                }
                Err(UnmapError::PageNotMapped) => {}
                Err(_) => panic!("desmapeamento inesperado de pagina de usuario"),
            }
        }
    });
}

/// Flags da entrada de nível mais baixo que mapeia `addr`, ou `None` se o
/// endereço não está mapeado. `sys_write` a usa para conferir, página por
/// página, que o buffer que um programa passou é dele (mapeado e com
/// `USER_ACCESSIBLE`); os testes, para conferir que uma região ficou livre.
pub fn user_page_flags(addr: VirtAddr) -> Option<PageTableFlags> {
    with_frame_state(|state| {
        // SAFETY: ver `active_mapper`; o lock de `FRAME_STATE` está preso.
        let mapper = unsafe { active_mapper(state) };
        match mapper.translate(addr) {
            TranslateResult::Mapped { flags, .. } => Some(flags),
            _ => None,
        }
    })
}

/// Zera um frame inteiro pela janela da memória física
/// (`physical_memory_offset + endereço do frame`), nunca pelo endereço
/// virtual do usuário: assim a página do usuário pode ficar somente-leitura
/// (W^X) sem que o kernel dependa de `CR0.WP` para preenchê-la.
pub fn zero_frame(frame: PhysFrame<Size4KiB>) {
    with_frame_state(|state| {
        let virt = state.physical_memory_offset + frame.start_address().as_u64();
        // SAFETY: `virt` é a janela de 4 KiB de um frame que o chamador
        // acabou de obter de `map_user_page` e que só o programa carregado
        // usará depois; nenhuma outra referência a ele existe agora.
        unsafe { core::ptr::write_bytes(virt.as_mut_ptr::<u8>(), 0, 4096) };
    });
}

/// Copia `bytes` para dentro de um frame, a partir de `offset`, pela janela
/// da memória física (ver `zero_frame`). `offset + bytes.len()` não pode
/// passar de 4096.
pub fn fill_frame(frame: PhysFrame<Size4KiB>, offset: usize, bytes: &[u8]) {
    assert!(offset + bytes.len() <= 4096, "fill_frame passou do fim do frame");
    with_frame_state(|state| {
        let virt = state.physical_memory_offset + frame.start_address().as_u64() + offset as u64;
        // SAFETY: o intervalo `offset..offset + len` cabe no frame (conferido
        // acima) e o frame é exclusivo do programa em carga, como em
        // `zero_frame`; `bytes` e o frame nunca se sobrepõem (um é imagem
        // ELF embutida, o outro memória física recém-alocada).
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), virt.as_mut_ptr::<u8>(), bytes.len());
        }
    });
}

/// Resumo somente-leitura de memória, calculado uma única vez no boot e
/// usado pelo comando `mem` do prompt para mostrar memória utilizável,
/// endereço e tamanho do heap na tela.
#[derive(Clone, Copy)]
pub struct MemoryInfo {
    pub usable_bytes: u64,
    pub heap_start: usize,
    pub heap_size: usize,
}

static MEMORY_INFO: Mutex<Option<MemoryInfo>> = Mutex::new(None);

/// Devolve o resumo de memória calculado por `init`.
///
/// Só é seguro chamar depois que `init` (via `os_rust::init`) já
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
/// Chamada uma única vez, por `os_rust::init`, depois de
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

    // O alocador continua de onde o heap parou: guardá-lo (em vez de
    // descartá-lo) é o que impede que um alocador novo, criado depois,
    // entregue de novo frames que o heap já usa (a garantia de "nunca
    // repetir frame" vale por instância, ver `BootInfoFrameAllocator`).
    *FRAME_STATE.lock() = Some(FrameState {
        allocator: frame_allocator,
        physical_memory_offset,
    });

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
