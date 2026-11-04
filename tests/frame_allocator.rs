//! Teste de integração do alocador de frames físicos (User Story 2):
//! frame alinhado e utilizável, sem repetição ao longo de várias
//! chamadas, e sinal explícito de esgotamento (FR-002, FR-003, FR-004).

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(proto_os::test_runner)]
#![reexport_test_harness_main = "test_main"]

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use proto_os::memory::BootInfoFrameAllocator;
use x86_64::structures::paging::FrameAllocator;

/// Guarda o `boot_info` recebido por `main`, para que cada `#[test_case]`
/// (que não recebe parâmetros) possa construir sua própria instância de
/// `BootInfoFrameAllocator` a partir do mesmo mapa de memória — sem
/// manter nenhum alocador ou mapeamento vivo entre os testes
/// (`research.md`, seção 12).
static BOOT_INFO: spin::Once<&'static BootInfo> = spin::Once::new();

entry_point!(main);

fn main(boot_info: &'static BootInfo) -> ! {
    proto_os::init(boot_info);
    BOOT_INFO.call_once(|| boot_info);
    test_main();
    proto_os::panic::halt_loop();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    proto_os::test_panic_handler(info)
}

fn memory_map() -> &'static bootloader::bootinfo::MemoryMap {
    &BOOT_INFO.call_once(|| unreachable!("main já chamou call_once antes de test_main")).memory_map
}

#[test_case]
fn frame_obtido_esta_alinhado_e_utilizavel() {
    // SAFETY: `memory_map()` veio do `BootInfo` real entregue pelo
    // bootloader (mesma garantia documentada em `main`).
    let mut allocator = unsafe { BootInfoFrameAllocator::init(memory_map()) };
    let frame = allocator
        .allocate_frame()
        .expect("deveria haver memoria utilizavel em uma VM de teste");
    assert!(frame.start_address().is_aligned(4096u64));
}

#[test_case]
fn frames_entregues_nunca_se_repetem() {
    // SAFETY: mesma garantia da função acima.
    let mut allocator = unsafe { BootInfoFrameAllocator::init(memory_map()) };

    const AMOSTRA: usize = 2000;
    let mut enderecos = [0u64; AMOSTRA];
    for endereco in enderecos.iter_mut() {
        let frame = allocator
            .allocate_frame()
            .expect("deveria haver memoria utilizavel suficiente para a amostra");
        *endereco = frame.start_address().as_u64();
    }

    for i in 0..AMOSTRA {
        for j in (i + 1)..AMOSTRA {
            assert_ne!(
                enderecos[i], enderecos[j],
                "o alocador entregou o mesmo frame duas vezes"
            );
        }
    }
}

/// Um mapa de memória pequeno e sintético (32 frames utilizáveis), só
/// para este teste: `allocate_frame` custa O(próximo índice) por chamada
/// (ver `research.md` seção 3 — simplicidade sobre eficiência), então
/// esgotar o mapa real do boot (dezenas de milhares de frames numa VM de
/// teste comum) seria demorado sem testar nada a mais do que esgotar um
/// mapa pequeno já testa.
fn mapa_pequeno() -> &'static bootloader::bootinfo::MemoryMap {
    use bootloader::bootinfo::{FrameRange, MemoryMap, MemoryRegion, MemoryRegionType};

    static MAPA: spin::Once<MemoryMap> = spin::Once::new();
    MAPA.call_once(|| {
        let mut map = MemoryMap::new();
        map.add_region(MemoryRegion {
            range: FrameRange {
                start_frame_number: 0x100,
                end_frame_number: 0x100 + 32,
            },
            region_type: MemoryRegionType::Usable,
        });
        map
    })
}

#[test_case]
fn esgotar_a_memoria_utilizavel_devolve_none() {
    // SAFETY: `mapa_pequeno()` é um `MemoryMap` construído por este
    // próprio teste (não descreve memória física real), mas tem o mesmo
    // formato que `BootInfoFrameAllocator::init` espera — não há memória
    // real sendo lida ou escrita a partir dos frames que ele descreve,
    // só a contagem de quantos existem.
    let mut allocator = unsafe { BootInfoFrameAllocator::init(mapa_pequeno()) };

    while allocator.allocate_frame().is_some() {}

    assert!(allocator.allocate_frame().is_none());
}
