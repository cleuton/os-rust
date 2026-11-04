//! Heap do kernel: faixa fixa de endereços virtuais, mapeada no boot, de
//! onde o alocador global (`Box`, `Vec`, `String`, ...) tira memória.
//!
//! Ver `research.md` (Marco 3), seções 5 a 7, para a justificativa de
//! cada decisão.

use linked_list_allocator::LockedHeap;
use x86_64::structures::paging::{
    mapper::MapToError, FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB,
};
use x86_64::VirtAddr;

/// Endereço virtual fixo de início do heap — fora de qualquer
/// mapeamento feito pelo bootloader (FR-010).
pub const HEAP_START: usize = 0x_4444_4444_0000;

/// Tamanho fixo do heap, definido em tempo de compilação — não cresce
/// (Assumptions da spec).
pub const HEAP_SIZE: usize = 100 * 1024;

/// O alocador global: atende `Box`, `Vec`, `String` e qualquer outro
/// tipo da crate `alloc` em todo o kernel, a partir do momento em que
/// `init_heap` retorna `Ok(())`. Lista encadeada de blocos livres,
/// protegida por um `spin::Mutex` interno à própria crate — o mesmo
/// padrão de sincronização já usado pelo resto do projeto (FR-016).
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// Mapeia toda a faixa do heap em frames físicos livres e inicializa o
/// alocador global. Deve ser chamada uma única vez, antes de qualquer
/// uso de `alloc` (FR-011).
pub fn init_heap(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    let page_range = {
        let heap_start = VirtAddr::new(HEAP_START as u64);
        let heap_end = heap_start + (HEAP_SIZE - 1) as u64;
        let heap_start_page = Page::containing_address(heap_start);
        let heap_end_page = Page::containing_address(heap_end);
        Page::range_inclusive(heap_start_page, heap_end_page)
    };

    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

        // SAFETY: `frame` acabou de ser obtido do `frame_allocator`
        // (ainda não está em uso por mais ninguém) e `page` pertence à
        // faixa fixa e documentada de `HEAP_START`/`HEAP_SIZE`, que não
        // colide com nenhum mapeamento feito pelo bootloader (FR-010) —
        // as duas condições que `map_to` exige para não violar memory
        // safety.
        let flush = unsafe { mapper.map_to(page, frame, flags, frame_allocator)? };
        flush.flush();
    }

    // SAFETY: todas as páginas de HEAP_START..HEAP_START+HEAP_SIZE foram
    // mapeadas no laço acima, antes deste ponto — a região inteira que
    // o alocador vai gerenciar já é memória válida e não se sobrepõe a
    // nenhuma outra estrutura (faixa fixa, exclusiva do heap).
    unsafe {
        ALLOCATOR.lock().init(HEAP_START as *mut u8, HEAP_SIZE);
    }

    Ok(())
}

/// Chamado pelo compilador quando uma alocação falha (heap sem espaço
/// contíguo suficiente). Reaproveita o `#[panic_handler]` já existente
/// (`panic::handle`, Marco 0): mesma tela legível e mesma linha na
/// serial, com o tamanho e o alinhamento pedidos (FR-015).
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    panic!(
        "heap esgotado: pedido de {} bytes (alinhamento {})",
        layout.size(),
        layout.align()
    );
}
