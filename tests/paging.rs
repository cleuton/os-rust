//! Teste de integração de tradução de endereços e criação de
//! mapeamentos (User Story 3, cenários 1-3): traduzir um endereço já
//! mapeado pelo bootloader, traduzir um endereço não mapeado, e mapear
//! uma página nova (incluindo o erro explícito de mapear a mesma página
//! duas vezes) (FR-007, FR-008, FR-009).

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(proto_os::test_runner)]
#![reexport_test_harness_main = "test_main"]

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use proto_os::memory::{self, BootInfoFrameAllocator};
use x86_64::structures::paging::{mapper::MapToError, Page};
use x86_64::VirtAddr;

/// Guarda o `boot_info` recebido por `main`, para que cada `#[test_case]`
/// possa reconstruir `physical_memory_offset`/`memory_map` sem manter
/// nenhum `OffsetPageTable`/alocador de frames vivo entre os testes
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

fn boot_info() -> &'static BootInfo {
    BOOT_INFO.call_once(|| unreachable!("main já chamou call_once antes de test_main"))
}

fn physical_memory_offset() -> VirtAddr {
    VirtAddr::new(boot_info().physical_memory_offset)
}

#[test_case]
fn traduz_endereco_ja_mapeado_pelo_bootloader() {
    // O buffer de texto VGA, em uso desde o Marco 0, sempre mapeado pelo
    // bootloader antes do kernel rodar (US3 cenário 1).
    let vga = VirtAddr::new(0xb8000);

    // SAFETY: `physical_memory_offset()` veio do `BootInfo` real
    // entregue pelo bootloader (mesma garantia documentada em `main`).
    let resultado = unsafe { memory::translate_addr(vga, physical_memory_offset()) };

    assert!(resultado.is_some());
}

#[test_case]
fn traduz_endereco_nao_mapeado_devolve_none() {
    // Endereço deliberadamente fora de qualquer região mapeada: acima do
    // que uma VM de teste com uma faixa modesta de RAM cobre (mapeamento
    // completo da física) e bem longe do heap e da imagem do kernel
    // (US3 cenário 2).
    let nao_mapeado = VirtAddr::new(0xdead_beaf_000);

    // SAFETY: mesma garantia da função acima.
    let resultado = unsafe { memory::translate_addr(nao_mapeado, physical_memory_offset()) };

    assert!(resultado.is_none());
}

#[test_case]
fn mapear_pagina_nova_permite_ler_e_escrever_nela() {
    // Página virtual fora do heap e de qualquer mapeamento existente,
    // dedicada só a este teste (US3 cenário 3).
    let pagina = Page::containing_address(VirtAddr::new(0x_2222_2222_0000));

    // SAFETY: `boot_info().memory_map` veio do `BootInfo` real entregue
    // pelo bootloader.
    let mut frame_allocator = unsafe { BootInfoFrameAllocator::init(&boot_info().memory_map) };

    // SAFETY: `pagina` não se sobrepõe a nenhuma estrutura em uso pelo
    // kernel (heap, imagem do kernel, mapeamento da física completa) —
    // é um endereço reservado só para este teste.
    unsafe { memory::map_page(pagina, physical_memory_offset(), &mut frame_allocator) }
        .expect("mapear uma pagina nova nao deveria falhar");

    let ponteiro = pagina.start_address().as_mut_ptr::<u64>();
    // SAFETY: `pagina` acabou de ser mapeada em um frame físico válido
    // com a flag WRITABLE, então escrever e ler um `u64` alinhado no
    // início dela é uma operação de memória válida.
    unsafe {
        ponteiro.write(0x1234_5678_9abc_def0);
        assert_eq!(ponteiro.read(), 0x1234_5678_9abc_def0);
    }

    // Mapear a mesma página de novo é um erro explícito, nunca uma
    // sobrescrita silenciosa (Edge Case "mapeamento sobre página já
    // mapeada").
    // SAFETY: mesma garantia acima.
    let resultado =
        unsafe { memory::map_page(pagina, physical_memory_offset(), &mut frame_allocator) };
    assert!(matches!(resultado, Err(MapToError::PageAlreadyMapped(_))));
}
