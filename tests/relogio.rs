//! Testes de integração do Marco 9 (driver de RTC): rodam dentro do QEMU, com o
//! relógio de verdade, e por isso conferem **faixas e ordem**, nunca a hora
//! exata. A conversão do chip (BCD, 12 horas, século, janela de atualização)
//! é provada em `src/rtc.rs`, com registradores fabricados. Cada arquivo de
//! `tests/` é um kernel próprio.
//!
//! O que cada bloco prova:
//! - o driver lê uma data plausível, e duas leituras seguidas não andam para
//!   trás;
//! - o comando `data` mostra uma linha no formato `AAAA-MM-DD HH:MM:SS UTC`;
//! - `SYS_TIME`: resultado, tamanho errado, ponteiro inválido, e o kernel
//!   seguindo vivo;
//! - `run hora` ponta a ponta, e dois `hora` ao mesmo tempo;
//! - os programas dos marcos anteriores continuam rodando.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os_rust::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use os_rust::syscall::{ERR_FAULT, ERR_INVAL};
use os_rust::user::{self, Termination, USER_HEAP_START, USER_REGION_END, USER_REGION_START, USER_STACK_BOTTOM};
use os_rust::{rtc, shell, vga_buffer};

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

// ---------------------------------------------------------------------------
// Ajudantes
// ---------------------------------------------------------------------------

/// A data como tupla, para comparar duas leituras na ordem do tempo.
fn tupla(t: &abi::DateTime) -> (u16, u8, u8, u8, u8, u8) {
    (t.year, t.month, t.day, t.hour, t.minute, t.second)
}

/// Monta um ELF64 `ET_EXEC` x86-64 com um único segmento `PT_LOAD` `R E` em
/// `USER_REGION_START`, contendo `code`, com a entrada no primeiro byte (mesmo
/// construtor de `tests/user_runtime.rs`).
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

/// `time(ptr, len)`: `mov eax, 10 ; movabs rdi, ptr ; mov esi, len ; syscall`.
fn time_call(ptr: u64, len: u32) -> Vec<u8> {
    let mut code = Vec::new();
    code.extend_from_slice(&[0xb8, 0x0a, 0x00, 0x00, 0x00]); // mov eax, 10 (SYS_TIME)
    code.extend_from_slice(&[0x48, 0xbf]); // movabs rdi, imm64
    code.extend_from_slice(&ptr.to_le_bytes());
    code.push(0xbe); // mov esi, imm32
    code.extend_from_slice(&len.to_le_bytes());
    code.extend_from_slice(&[0x0f, 0x05]); // syscall
    code
}

/// `exit(rax)`: `mov rdi, rax ; mov eax, 2 ; syscall`. O programa sai com o
/// resultado da última syscall.
const EXIT_COM_RAX: [u8; 10] = [
    0x48, 0x89, 0xc7, // mov rdi, rax
    0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2 (SYS_EXIT)
    0x0f, 0x05, // syscall
];

/// Roda um programa sintético e devolve o código de saída.
fn rodar_sintetico(partes: &[&[u8]]) -> u64 {
    let mut code = Vec::new();
    for parte in partes {
        code.extend_from_slice(parte);
    }
    match user::run_image(&synth_elf(&code)).expect("ELF valido") {
        Termination::Exit { code } => code,
        outro => panic!("termino inesperado: {:?}", outro),
    }
}

/// Digita uma linha no shell, como o teclado faria.
fn digitar_no_prompt(linha: &str) {
    for byte in linha.bytes() {
        shell::feed(byte);
    }
    shell::feed(b'\n');
}

/// As linhas da tela no formato `dddd-dd-dd dd:dd:dd UTC`, sem o ` UTC`.
fn linhas_de_hora_na_tela() -> Vec<String> {
    let mut linhas = Vec::new();
    for row in 0..25 {
        let bytes = vga_buffer::screen_row_bytes(row);
        if bytes[4] == b'-' && &bytes[19..23] == b" UTC" {
            if let Ok(texto) = core::str::from_utf8(&bytes[..19]) {
                linhas.push(String::from(texto));
            }
        }
    }
    linhas
}

// ---------------------------------------------------------------------------
// O driver
// ---------------------------------------------------------------------------

#[test_case]
fn o_relogio_devolve_uma_data_plausivel() {
    let t = rtc::read().expect("o RTC do QEMU responde");
    assert!((2020..=2099).contains(&t.year));
    assert!((1..=12).contains(&t.month));
    assert!((1..=31).contains(&t.day));
    assert!(t.hour <= 23 && t.minute <= 59 && t.second <= 59);
    assert_eq!(t._pad, 0);
}

#[test_case]
fn duas_leituras_seguidas_nao_andam_para_tras() {
    let a = rtc::read().expect("primeira leitura");
    let b = rtc::read().expect("segunda leitura");
    assert!(tupla(&a) <= tupla(&b));
}

// ---------------------------------------------------------------------------
// O comando `data`
// ---------------------------------------------------------------------------

#[test_case]
fn data_mostra_uma_linha_no_formato() {
    vga_buffer::clear_screen();
    digitar_no_prompt("data");
    assert_eq!(vga_buffer::screen_count_clock_lines(), 1);
}

#[test_case]
fn data_cai_entre_duas_leituras_do_driver() {
    let antes = rtc::read().expect("antes");
    vga_buffer::clear_screen();
    digitar_no_prompt("data");
    let depois = rtc::read().expect("depois");
    let linhas = linhas_de_hora_na_tela();
    assert_eq!(linhas.len(), 1);
    // O formato ISO é ordenável como texto.
    assert!(alloc::format!("{}", antes).as_str() <= linhas[0].as_str());
    assert!(linhas[0].as_str() <= alloc::format!("{}", depois).as_str());
}

// ---------------------------------------------------------------------------
// SYS_TIME
// ---------------------------------------------------------------------------

#[test_case]
fn time_devolve_zero_e_escreve_o_ano_no_buffer() {
    // Depois da syscall, `rdi` ainda é o ponteiro (o kernel o preserva):
    // `movzx edi, word [rdi]` lê o ano e o programa sai com ele.
    const LE_O_ANO_E_SAI: [u8; 10] = [
        0x0f, 0xb7, 0x3f, // movzx edi, word ptr [rdi]
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2 (SYS_EXIT)
        0x0f, 0x05, // syscall
    ];
    let ano = rodar_sintetico(&[&time_call(USER_STACK_BOTTOM, 8), &LE_O_ANO_E_SAI]);
    assert!((2020..=2099).contains(&ano), "ano {}", ano);
}

#[test_case]
fn time_devolve_zero_em_sucesso_e_zera_o_alinhamento() {
    // `movzx edi, byte [rdi + 7]`: o byte de alinhamento, que tem de ser 0.
    const LE_O_ALINHAMENTO_E_SAI: [u8; 11] = [
        0x0f, 0xb6, 0x7f, 0x07, // movzx edi, byte ptr [rdi + 7]
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2
        0x0f, 0x05, // syscall
    ];
    assert_eq!(rodar_sintetico(&[&time_call(USER_STACK_BOTTOM, 8), &LE_O_ALINHAMENTO_E_SAI]), 0);
    assert_eq!(rodar_sintetico(&[&time_call(USER_STACK_BOTTOM, 8), &EXIT_COM_RAX]), 0);
}

#[test_case]
fn time_com_tamanho_errado_devolve_inval() {
    for len in [0, 7, 9, 4096] {
        let code = rodar_sintetico(&[&time_call(USER_STACK_BOTTOM, len), &EXIT_COM_RAX]);
        assert_eq!(code as i64, ERR_INVAL, "len {}", len);
    }
}

#[test_case]
fn time_confere_o_tamanho_antes_do_ponteiro() {
    let code = rodar_sintetico(&[&time_call(0x4444_4444_0000, 7), &EXIT_COM_RAX]);
    assert_eq!(code as i64, ERR_INVAL);
}

#[test_case]
fn time_com_ponteiro_invalido_devolve_fault() {
    let invalidos = [
        0x4444_4444_0000,         // memória só do kernel
        USER_REGION_START,        // página de código do programa: não é gravável
        USER_HEAP_START,          // dentro da região, mas não mapeado
        USER_REGION_END - 4,      // começa na pilha e termina fora da região
        0,                        // ponteiro nulo
    ];
    for ptr in invalidos {
        let code = rodar_sintetico(&[&time_call(ptr, 8), &EXIT_COM_RAX]);
        assert_eq!(code as i64, ERR_FAULT, "ptr {:#x}", ptr);
    }
}

#[test_case]
fn kernel_segue_vivo_depois_de_time_invalida() {
    rodar_sintetico(&[&time_call(0x4444_4444_0000, 8), &EXIT_COM_RAX]);
    assert_eq!(user::run("hello"), Ok(Termination::Exit { code: 0 }));
}

// ---------------------------------------------------------------------------
// `run hora`
// ---------------------------------------------------------------------------

#[test_case]
fn run_hora_escreve_a_data_e_o_prompt_volta() {
    let antes = rtc::read().expect("antes");
    vga_buffer::clear_screen();
    digitar_no_prompt("run hora");
    let depois = rtc::read().expect("depois");
    let linhas = linhas_de_hora_na_tela();
    assert_eq!(linhas.len(), 1);
    assert!(alloc::format!("{}", antes).as_str() <= linhas[0].as_str());
    assert!(linhas[0].as_str() <= alloc::format!("{}", depois).as_str());
    // O prompt voltou: um comando seguinte funciona.
    vga_buffer::clear_screen();
    digitar_no_prompt("sobre");
    assert!(vga_buffer::screen_contains(os_rust::VERSION));
}

#[test_case]
fn run_hora_e_data_concordam_a_menos_de_alguns_segundos() {
    // As duas saídas cabem entre uma leitura antes e outra depois dos dois
    // comandos: nenhuma delas pode estar fora desse intervalo, nem a segunda
    // antes da primeira.
    let antes = rtc::read().expect("antes");
    vga_buffer::clear_screen();
    digitar_no_prompt("data");
    digitar_no_prompt("run hora");
    let depois = rtc::read().expect("depois");
    let linhas = linhas_de_hora_na_tela();
    assert_eq!(linhas.len(), 2);
    assert!(linhas[0].as_str() <= linhas[1].as_str());
    assert!(alloc::format!("{}", antes).as_str() <= linhas[0].as_str());
    assert!(linhas[1].as_str() <= alloc::format!("{}", depois).as_str());
}

#[test_case]
fn dois_programas_hora_ao_mesmo_tempo_recebem_horas_validas() {
    vga_buffer::clear_screen();
    digitar_no_prompt("run hora hora");
    let linhas = linhas_de_hora_na_tela();
    assert_eq!(linhas.len(), 2);
    assert!(linhas[0].as_str() <= linhas[1].as_str());
}

// ---------------------------------------------------------------------------
// Regressão
// ---------------------------------------------------------------------------

#[test_case]
fn os_programas_dos_marcos_anteriores_continuam_rodando() {
    assert_eq!(user::run("hello"), Ok(Termination::Exit { code: 0 }));
    vga_buffer::clear_screen();
    digitar_no_prompt("run ping pong");
    assert!(vga_buffer::screen_contains("ping 1"));
    assert!(vga_buffer::screen_contains("pong 1"));
    vga_buffer::clear_screen();
    digitar_no_prompt("ls /ram");
    assert!(vga_buffer::screen_contains("ola.txt"));
}
