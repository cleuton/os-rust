//! Testes de integração do modo usuário (Marco 5): carregam programas em
//! ring 3, de verdade, dentro do QEMU, e conferem o que o kernel faz com
//! eles. Cada arquivo de `tests/` é um kernel próprio, com seu próprio
//! `entry_point!`, como em `boot_integration.rs`.
//!
//! Além dos programas embutidos (`hello`, `crash`), alguns testes montam
//! ELFs sintéticos de poucos bytes (`synth_elf`), para os casos que não
//! merecem um programa no prompt: ponteiro inválido, syscall inexistente,
//! cada exceção de CPU.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os_rust::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::vec::Vec;
use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use os_rust::elf::LoadError;
use os_rust::syscall::{ERR_FAULT, ERR_INVAL};
use os_rust::user::{
    self, RunError, Termination, USER_REGION_START, USER_STACK_BOTTOM, USER_STACK_PAGES,
};
use os_rust::{memory, shell, vga_buffer};
use x86_64::VirtAddr;

entry_point!(main);

fn main(boot_info: &'static BootInfo) -> ! {
    os_rust::init(boot_info);
    test_main();
    os_rust::panic::halt_loop();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    os_rust::test_panic_handler(info)
}

/// Monta um ELF64 `ET_EXEC` x86-64 com um único segmento `PT_LOAD` `R E` em
/// `USER_REGION_START`, contendo `code`, com a entrada no primeiro byte.
/// Campos usados (ver `src/elf.rs`): cabeçalho de 64 bytes, um *program
/// header* de 56 bytes logo depois, e o código em seguida.
fn synth_elf(code: &[u8]) -> Vec<u8> {
    let offset = 64 + 56;
    let mut file = alloc::vec![0u8; offset];
    file[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    file[4] = 2; // ELF64
    file[5] = 1; // little-endian
    file[6] = 1; // versão
    file[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    file[18..20].copy_from_slice(&62u16.to_le_bytes()); // EM_X86_64
    file[24..32].copy_from_slice(&USER_REGION_START.to_le_bytes()); // e_entry
    file[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
    file[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
    file[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
    file.extend_from_slice(code);
    let h = 64;
    file[h..h + 4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    file[h + 4..h + 8].copy_from_slice(&5u32.to_le_bytes()); // R + X
    file[h + 8..h + 16].copy_from_slice(&(offset as u64).to_le_bytes()); // p_offset
    file[h + 16..h + 24].copy_from_slice(&USER_REGION_START.to_le_bytes()); // p_vaddr
    file[h + 32..h + 40].copy_from_slice(&(code.len() as u64).to_le_bytes()); // p_filesz
    file[h + 40..h + 48].copy_from_slice(&(code.len() as u64).to_le_bytes()); // p_memsz
    file
}

/// Verdadeiro se nenhuma página da região do usuário está mapeada: nem o
/// início do programa nem qualquer uma das páginas da pilha.
fn regiao_do_usuario_esta_livre() -> bool {
    if memory::user_page_flags(VirtAddr::new(USER_REGION_START)).is_some() {
        return false;
    }
    (0..USER_STACK_PAGES)
        .all(|n| memory::user_page_flags(VirtAddr::new(USER_STACK_BOTTOM + n * 4096)).is_none())
}

/// Digita uma linha no shell, byte a byte, como o teclado faria.
fn digitar(linha: &str) {
    for byte in linha.bytes() {
        shell::feed(byte);
    }
    shell::feed(b'\n');
}

#[test_case]
fn hello_escreve_a_mensagem_e_devolve_o_controle() {
    vga_buffer::clear_screen();
    let resultado = user::run("hello");
    assert_eq!(resultado, Ok(Termination::Exit { code: 0 }));
    assert!(vga_buffer::screen_contains("Ola do ring 3!"));
}

#[test_case]
fn nome_inexistente_lista_os_programas() {
    assert_eq!(user::run("xpto"), Err(RunError::UnknownProgram));

    // A mensagem do shell (`execute` é privada, então a linha é digitada).
    vga_buffer::clear_screen();
    digitar("run xpto");
    assert!(vga_buffer::screen_contains("programa desconhecido: xpto"));
    assert!(vga_buffer::screen_contains("hello"));
}

#[test_case]
fn regiao_do_usuario_fica_livre_depois_do_termino() {
    assert!(regiao_do_usuario_esta_livre());
    user::run("hello").expect("hello carrega");
    assert!(regiao_do_usuario_esta_livre());
}

#[test_case]
fn rodar_hello_varias_vezes_nao_esgota_frames() {
    for _ in 0..50 {
        assert_eq!(user::run("hello"), Ok(Termination::Exit { code: 0 }));
    }
    assert!(regiao_do_usuario_esta_livre());
}

#[test_case]
fn write_com_ponteiro_do_kernel_devolve_err_fault() {
    // mov eax,1 ; movabs rdi,0x4444_4444_0000 (início do heap do kernel) ;
    // mov esi,4 ; syscall ; mov rdi,rax ; mov eax,2 ; syscall
    // O programa sai com o resultado de `write` como código de saída.
    let code = [
        0xb8, 0x01, 0x00, 0x00, 0x00, // mov eax, 1
        0x48, 0xbf, 0x00, 0x00, 0x44, 0x44, 0x44, 0x44, 0x00, 0x00, // movabs rdi, 0x4444_4444_0000
        0xbe, 0x04, 0x00, 0x00, 0x00, // mov esi, 4
        0x0f, 0x05, // syscall
        0x48, 0x89, 0xc7, // mov rdi, rax
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2
        0x0f, 0x05, // syscall
    ];
    let resultado = user::run_image(&synth_elf(&code)).expect("ELF valido");
    match resultado {
        Termination::Exit { code } => assert_eq!(code as i64, ERR_FAULT),
        outro => panic!("termino inesperado: {:?}", outro),
    }
    assert!(regiao_do_usuario_esta_livre());
}

/// Programa que chama `write(USER_REGION_START, len)` e sai com o resultado.
fn write_com_tamanho(len: u32) -> Vec<u8> {
    let mut code = Vec::new();
    code.extend_from_slice(&[0xb8, 0x01, 0x00, 0x00, 0x00]); // mov eax, 1
    code.extend_from_slice(&[0xbf]); // mov edi, imm32
    code.extend_from_slice(&(USER_REGION_START as u32).to_le_bytes());
    code.extend_from_slice(&[0xbe]); // mov esi, imm32
    code.extend_from_slice(&len.to_le_bytes());
    code.extend_from_slice(&[0x0f, 0x05]); // syscall
    code.extend_from_slice(&[0x48, 0x89, 0xc7]); // mov rdi, rax
    code.extend_from_slice(&[0xb8, 0x02, 0x00, 0x00, 0x00]); // mov eax, 2
    code.extend_from_slice(&[0x0f, 0x05]); // syscall
    synth_elf(&code)
}

#[test_case]
fn write_grande_demais_devolve_err_inval() {
    let resultado = user::run_image(&write_com_tamanho(5000)).expect("ELF valido");
    match resultado {
        Termination::Exit { code } => assert_eq!(code as i64, ERR_INVAL),
        outro => panic!("termino inesperado: {:?}", outro),
    }
}

#[test_case]
fn write_de_tamanho_zero_devolve_zero() {
    let resultado = user::run_image(&write_com_tamanho(0)).expect("ELF valido");
    assert_eq!(resultado, Termination::Exit { code: 0 });
}

#[test_case]
fn elf_invalido_e_recusado_sem_mapear_nada() {
    // Magia errada.
    let mut ruim = synth_elf(&[0x0f, 0x0b]);
    ruim[0] = 0;
    assert_eq!(user::run_image(&ruim).unwrap_err(), LoadError::NotElf);
    assert!(regiao_do_usuario_esta_livre());

    // Segmento fora da região do usuário (p_vaddr no início da memória).
    let mut fora = synth_elf(&[0x0f, 0x0b]);
    fora[64 + 16..64 + 24].copy_from_slice(&0x1000u64.to_le_bytes());
    fora[24..32].copy_from_slice(&0x1000u64.to_le_bytes());
    assert_eq!(user::run_image(&fora).unwrap_err(), LoadError::OutOfRegion);
    assert!(regiao_do_usuario_esta_livre());

    // O kernel segue vivo e o próximo programa roda.
    assert_eq!(user::run("hello"), Ok(Termination::Exit { code: 0 }));
}

/// Confere que o resultado é uma falha de CPU com a sigla esperada e devolve
/// os campos, para os testes olharem o resto.
fn esperar_falha(resultado: Termination, sigla: &str) -> (u64, Option<u64>, Option<u64>) {
    match resultado {
        Termination::Fault {
            mnemonic,
            rip,
            error_code,
            fault_address,
            ..
        } => {
            assert_eq!(mnemonic, sigla, "excecao inesperada");
            (rip, error_code, fault_address)
        }
        outro => panic!("esperava falha {}, veio {:?}", sigla, outro),
    }
}

/// Depois de qualquer falha o kernel tem de seguir vivo: a região do usuário
/// volta a ficar livre e o próximo programa roda normalmente.
fn kernel_segue_vivo() {
    assert!(regiao_do_usuario_esta_livre());
    assert_eq!(user::run("hello"), Ok(Termination::Exit { code: 0 }));
}

#[test_case]
fn crash_termina_por_opcode_invalido_sem_derrubar_o_kernel() {
    let resultado = user::run("crash").expect("crash carrega");
    let (rip, _, _) = esperar_falha(resultado, "#UD");
    // O endereço exato depende do binário (o compilador pode pôr um `push`
    // antes do `ud2`): basta cair dentro da página de código do programa.
    assert!((USER_REGION_START..USER_REGION_START + 4096).contains(&rip));
}

#[test_case]
fn depois_de_uma_falha_hello_roda_de_novo() {
    user::run("crash").expect("crash carrega");
    kernel_segue_vivo();
}

#[test_case]
fn instrucao_privilegiada_vira_gp_em_ring_3() {
    // `hlt` só é permitida em ring 0: se o programa estivesse em ring 0, a
    // instrução teria sucesso e o teste travaria. Como ela vira #GP, o
    // programa está mesmo em ring 3.
    let resultado = user::run_image(&synth_elf(&[0xf4])).expect("ELF valido");
    esperar_falha(resultado, "#GP");
    kernel_segue_vivo();
}

#[test_case]
fn divisao_por_zero_vira_de() {
    // xor ecx, ecx ; div ecx
    let resultado = user::run_image(&synth_elf(&[0x31, 0xc9, 0xf7, 0xf1])).expect("ELF valido");
    esperar_falha(resultado, "#DE");
    kernel_segue_vivo();
}

#[test_case]
fn acesso_a_endereco_do_kernel_vira_pf() {
    // movabs rax, [0x4444_4444_0000] (início do heap do kernel, mapeado só
    // para o kernel): o acesso de ring 3 é uma violação de proteção.
    let code = [
        0x48, 0xa1, 0x00, 0x00, 0x44, 0x44, 0x44, 0x44, 0x00, 0x00,
    ];
    let resultado = user::run_image(&synth_elf(&code)).expect("ELF valido");
    let (_, _, endereco) = esperar_falha(resultado, "#PF");
    assert_eq!(endereco, Some(0x4444_4444_0000));
    kernel_segue_vivo();
}

#[test_case]
fn pilha_invalida_encerra_o_programa_sem_derrubar_o_kernel() {
    // movabs rsp, 0x8000_0000_0000_0000 ; push rax
    // O `push` acessa um endereço não canônico. Num processador real isso é
    // `#SS`; o QEMU (TCG) reporta `#PF` para o mesmo acesso. Os dois são
    // tratados e encerram só o programa, que é o que importa: sem um gate
    // para `#SS`, o hardware real escalaria para double fault (fatal).
    let code = [0x48, 0xbc, 0, 0, 0, 0, 0, 0, 0, 0x80, 0x50];
    let resultado = user::run_image(&synth_elf(&code)).expect("ELF valido");
    match resultado {
        Termination::Fault { mnemonic: "#SS" | "#PF", .. } => {}
        outro => panic!("esperava #SS ou #PF, veio {:?}", outro),
    }
    kernel_segue_vivo();
}

#[test_case]
fn int_80_sem_gate_vira_gp() {
    let resultado = user::run_image(&synth_elf(&[0xcd, 0x80])).expect("ELF valido");
    esperar_falha(resultado, "#GP");
    kernel_segue_vivo();
}

#[test_case]
fn syscall_inexistente_encerra_o_programa() {
    // mov eax, 99 ; syscall
    let code = [0xb8, 0x63, 0x00, 0x00, 0x00, 0x0f, 0x05];
    let resultado = user::run_image(&synth_elf(&code)).expect("ELF valido");
    assert_eq!(resultado, Termination::BadSyscall { number: 99 });
    kernel_segue_vivo();
}
