//! Teste de integração do heap do kernel (User Story 3, cenários 4-6,
//! exigido literalmente por FR-019): `Box`/`Vec` corretos e dentro da
//! faixa do heap, reaproveitamento de memória liberada em volume
//! (SC-004), reaproveitamento com um objeto mantido vivo, e respeito ao
//! alinhamento pedido (FR-014).

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(proto_os::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use proto_os::allocator::{HEAP_SIZE, HEAP_START};

entry_point!(main);

fn main(boot_info: &'static BootInfo) -> ! {
    proto_os::init(boot_info);
    test_main();
    proto_os::panic::halt_loop();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    proto_os::test_panic_handler(info)
}

/// Verdadeiro se `endereco` está dentro da faixa do heap
/// (`HEAP_START..HEAP_START + HEAP_SIZE`).
fn dentro_do_heap(endereco: usize) -> bool {
    (HEAP_START..HEAP_START + HEAP_SIZE).contains(&endereco)
}

#[test_case]
fn box_e_vec_tem_valores_corretos_e_ficam_no_heap() {
    // US3 cenário 4.
    let valor = Box::new(42);
    assert_eq!(*valor, 42);
    assert!(dentro_do_heap(&*valor as *const _ as usize));

    let mut vetor = Vec::new();
    for i in 0..1000 {
        vetor.push(i);
    }
    let soma_esperada: u64 = (0..1000u64).sum();
    let soma: u64 = vetor.iter().sum();
    assert_eq!(soma, soma_esperada);
    assert!(dentro_do_heap(vetor.as_ptr() as usize));
}

#[test_case]
fn alocar_e_liberar_muitas_vezes_reaproveita_o_heap() {
    // US3 cenário 5, SC-004: soma, ao longo do teste, pelo menos 10x o
    // tamanho do heap, sem nenhuma falha — só é possível porque a
    // memória liberada a cada iteração é reaproveitada pela próxima.
    let bloco = 4 * 1024; // 4 KiB por iteração
    let iteracoes = (10 * HEAP_SIZE / bloco) + 1;

    for i in 0..iteracoes {
        let vetor: Vec<u8> = alloc::vec![i as u8; bloco];
        assert_eq!(vetor.len(), bloco);
        // `vetor` sai de escopo ao final desta iteração, liberando o
        // bloco antes da próxima alocação.
    }
}

#[test_case]
fn reaproveitamento_nao_depende_do_heap_inteiro_estar_livre() {
    // US3 cenário 6: um objeto mantido vivo durante todo o teste,
    // enquanto muitos outros são alocados e liberados ao redor dele.
    let ancora = Box::new([0u8; 1024]);

    let bloco = 4 * 1024;
    let iteracoes = (10 * HEAP_SIZE / bloco) + 1;
    for i in 0..iteracoes {
        let vetor: Vec<u8> = alloc::vec![i as u8; bloco];
        assert_eq!(vetor.len(), bloco);
    }

    assert_eq!(ancora[0], 0);
}

#[test_case]
fn alocacao_respeita_alinhamento_maior_que_8_bytes() {
    // FR-014, Edge Case "Tamanho e alinhamento": uma alocação com
    // alinhamento de 4 KiB (maior que os 8 bytes que um `u64` já
    // exigiria) deve ser respeitada pelo alocador de heap.
    #[repr(align(4096))]
    #[allow(dead_code)]
    struct Pagina([u8; 4096]);

    let valor = Box::new(Pagina([0u8; 4096]));
    let endereco = &*valor as *const Pagina as usize;

    assert_eq!(endereco % 4096, 0);
    assert!(dentro_do_heap(endereco));
}
