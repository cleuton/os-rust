//! Testes de integração do Marco 6 (interface de programação): carregam
//! programas em ring 3, de verdade, dentro do QEMU, e conferem o contrato
//! ampliado (`SYS_READ_LINE`, `SYS_ALLOC`), a biblioteca de runtime e o
//! isolamento de falhas. Cada arquivo de `tests/` é um kernel próprio.
//!
//! Nenhum teste depende de teclado físico: eles "digitam" empurrando
//! scancodes na fila do kernel (`interrupts::push_scancode`) antes de rodar o
//! programa, e o programa os lê pelo caminho real (`SYS_READ_LINE`).
//!
//! Quais User Stories e requisitos cada bloco prova:
//! - leitura de teclado com `eco` (US1, FR-005, FR-013);
//! - memória dentro de um programa (FR-013), com programas reais e ELFs
//!   sintéticos;
//! - `SYS_READ_LINE` inválida, e o prompt retomando o teclado (FR-004);
//! - isolamento com `falha_memoria` (US2, FR-007, FR-013);
//! - o guia bate com o código (US3, FR-008, FR-009).

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os_rust::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::vec::Vec;
use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use os_rust::syscall::{ERR_FAULT, ERR_INVAL, ERR_NOMEM};
use os_rust::user::{
    self, Termination, USER_HEAP_START, USER_REGION_START, USER_STACK_BOTTOM, USER_STACK_PAGES,
};
use os_rust::{interrupts, keyboard, memory, shell, vga_buffer};
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

// ---------------------------------------------------------------------------
// Ajudantes
// ---------------------------------------------------------------------------

/// Monta um ELF64 `ET_EXEC` x86-64 com um único segmento `PT_LOAD` `R E` em
/// `USER_REGION_START`, contendo `code`, com a entrada no primeiro byte. Campos
/// usados (ver `src/elf.rs`): cabeçalho de 64 bytes, um *program header* de 56
/// bytes logo depois, e o código em seguida. (Mesmo construtor de
/// `tests/user_mode.rs`, que não é editado.)
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
/// início do programa, nem o início do heap, nem as páginas da pilha.
fn regiao_do_usuario_esta_livre() -> bool {
    if memory::user_page_flags(VirtAddr::new(USER_REGION_START)).is_some() {
        return false;
    }
    if memory::user_page_flags(VirtAddr::new(USER_HEAP_START)).is_some() {
        return false;
    }
    (0..USER_STACK_PAGES)
        .all(|n| memory::user_page_flags(VirtAddr::new(USER_STACK_BOTTOM + n * 4096)).is_none())
}

/// Esvazia a fila de scancodes e solta Shift: a fila e o estado de Shift são
/// globais, então cada teste começa do zero.
fn reset_teclado() {
    while interrupts::next_scancode().is_some() {}
    keyboard::translate(0xAA); // Shift esquerdo solto
    keyboard::translate(0xB6); // Shift direito solto
}

/// "Digita" `texto` empurrando os scancodes correspondentes na fila do
/// kernel. Aceita letras, dígitos, espaço, `\n` (Enter) e `\x08` (Backspace);
/// maiúsculas empurram Shift antes e depois. A fila tem 16 posições: o ajudante
/// recusa (com `panic!`) um texto que geraria mais scancodes que isso.
fn empurrar(texto: &str) {
    let mut total = 0;
    let mut enviar = |scancode: u8| {
        interrupts::push_scancode(scancode);
        total += 1;
    };
    for ch in texto.bytes() {
        let minuscula = ch.to_ascii_lowercase();
        let scancode = match minuscula {
            b'a'..=b'z' => {
                const LINHAS: [(&[u8], u8); 3] =
                    [(b"qwertyuiop", 0x10), (b"asdfghjkl", 0x1E), (b"zxcvbnm", 0x2C)];
                let mut achado = None;
                for (letras, base) in LINHAS {
                    if let Some(pos) = letras.iter().position(|&l| l == minuscula) {
                        achado = Some(base + pos as u8);
                    }
                }
                achado.expect("letra na tabela")
            }
            b'1'..=b'9' => 0x02 + (minuscula - b'1'),
            b'0' => 0x0B,
            b' ' => 0x39,
            b'\n' => 0x1C,
            0x08 => 0x0E,
            _ => panic!("caractere sem scancode no teste: {:?}", ch as char),
        };
        if ch.is_ascii_uppercase() {
            enviar(0x2A);
            enviar(scancode);
            enviar(0xAA);
        } else {
            enviar(scancode);
        }
    }
    assert!(total <= 16, "texto gera {} scancodes; a fila tem 16", total);
}

/// Digita uma linha no shell, byte a byte, como o teclado faria (o prompt lê
/// pelo mesmo `feed` que `shell::poll_keyboard` usa).
fn digitar_no_prompt(linha: &str) {
    for byte in linha.bytes() {
        shell::feed(byte);
    }
    shell::feed(b'\n');
}

// ---------------------------------------------------------------------------
// US1: `eco` lê o teclado e responde (FR-005, FR-013)
// ---------------------------------------------------------------------------

#[test_case]
fn eco_devolve_o_texto_digitado() {
    reset_teclado();
    vga_buffer::clear_screen();
    empurrar("Ola mundo\n");
    assert_eq!(user::run("eco"), Ok(Termination::Exit { code: 0 }));
    assert!(vga_buffer::screen_contains("voce digitou: Ola mundo"));
    // A contagem de palavras usa um `Vec`: exercita o alocador da biblioteca.
    assert!(vga_buffer::screen_contains("palavras: 2"));
}

#[test_case]
fn eco_trata_backspace() {
    reset_teclado();
    vga_buffer::clear_screen();
    empurrar("abcx\x08d\n");
    assert_eq!(user::run("eco"), Ok(Termination::Exit { code: 0 }));
    assert!(vga_buffer::screen_contains("voce digitou: abcd"));
}

#[test_case]
fn eco_com_linha_vazia() {
    reset_teclado();
    vga_buffer::clear_screen();
    empurrar("\n");
    assert_eq!(user::run("eco"), Ok(Termination::Exit { code: 0 }));
    assert!(vga_buffer::screen_contains("voce digitou: "));
    assert!(vga_buffer::screen_contains("palavras: 0"));
}

#[test_case]
fn eco_termina_e_libera_a_regiao() {
    reset_teclado();
    empurrar("oi\n");
    assert_eq!(user::run("eco"), Ok(Termination::Exit { code: 0 }));
    assert!(regiao_do_usuario_esta_livre());
}

#[test_case]
fn rodar_eco_varias_vezes_nao_esgota_frames() {
    for _ in 0..30 {
        reset_teclado();
        empurrar("oi\n");
        assert_eq!(user::run("eco"), Ok(Termination::Exit { code: 0 }));
    }
    assert!(regiao_do_usuario_esta_livre());
}

// ---------------------------------------------------------------------------
// FR-013: memória dentro de um programa (`SYS_ALLOC`), com ELFs sintéticos
// ---------------------------------------------------------------------------

/// `alloc(size)`: `mov eax, 4 ; mov edi, size ; syscall`.
fn alloc_call(size: u32) -> Vec<u8> {
    let mut code = Vec::new();
    code.extend_from_slice(&[0xb8, 0x04, 0x00, 0x00, 0x00]); // mov eax, 4 (SYS_ALLOC)
    code.push(0xbf); // mov edi, imm32
    code.extend_from_slice(&size.to_le_bytes());
    code.extend_from_slice(&[0x0f, 0x05]); // syscall
    code
}

/// `exit(rax)`: `mov rdi, rax ; mov eax, 2 ; syscall`. O programa sai com o
/// resultado da última syscall, que o teste lê em `Termination::Exit { code }`.
fn exit_com_rax() -> [u8; 10] {
    [
        0x48, 0x89, 0xc7, // mov rdi, rax
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2 (SYS_EXIT)
        0x0f, 0x05, // syscall
    ]
}

/// Roda um programa sintético feito de `partes` (em sequência) e devolve o
/// código de saída.
fn rodar_sintetico(partes: &[&[u8]]) -> u64 {
    let mut code = Vec::new();
    for parte in partes {
        code.extend_from_slice(parte);
    }
    let resultado = user::run_image(&synth_elf(&code)).expect("ELF valido");
    match resultado {
        Termination::Exit { code } => code,
        outro => panic!("termino inesperado: {:?}", outro),
    }
}

#[test_case]
fn alloc_devolve_o_inicio_do_heap() {
    let code = rodar_sintetico(&[&alloc_call(4096), &exit_com_rax()]);
    assert_eq!(code, USER_HEAP_START);
}

#[test_case]
fn alloc_sucessivo_e_contiguo() {
    let code = rodar_sintetico(&[&alloc_call(4096), &alloc_call(4096), &exit_com_rax()]);
    assert_eq!(code, USER_HEAP_START + 4096);
}

#[test_case]
fn alloc_arredonda_para_paginas() {
    let code = rodar_sintetico(&[&alloc_call(1), &alloc_call(1), &exit_com_rax()]);
    assert_eq!(code, USER_HEAP_START + 4096);
}

#[test_case]
fn memoria_alocada_e_gravavel_e_zerada() {
    // alloc(4096) ; mov rbx, rax ; mov byte [rbx], 0x2a ; movzx edi, byte [rbx] ;
    // mov eax, 2 ; syscall  -> sai com o byte que leu de volta.
    let grava_e_le: &[u8] = &[
        0x48, 0x89, 0xc3, // mov rbx, rax
        0xc6, 0x03, 0x2a, // mov byte [rbx], 0x2a
        0x0f, 0xb6, 0x3b, // movzx edi, byte [rbx]
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2
        0x0f, 0x05, // syscall
    ];
    assert_eq!(rodar_sintetico(&[&alloc_call(4096), grava_e_le]), 42);

    // A área nasce zerada: lê o primeiro byte logo depois de alocar.
    let so_le: &[u8] = &[
        0x48, 0x89, 0xc3, // mov rbx, rax
        0x0f, 0xb6, 0x3b, // movzx edi, byte [rbx]
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2
        0x0f, 0x05, // syscall
    ];
    assert_eq!(rodar_sintetico(&[&alloc_call(4096), so_le]), 0);
}

#[test_case]
fn alloc_tamanho_zero_devolve_inval() {
    let code = rodar_sintetico(&[&alloc_call(0), &exit_com_rax()]);
    assert_eq!(code as i64, ERR_INVAL);
}

#[test_case]
fn alloc_alem_do_limite_devolve_nomem() {
    // 2 MiB de uma vez: mais que o heap inteiro (1 MiB).
    let de_uma_vez = rodar_sintetico(&[&alloc_call(2 * 1024 * 1024), &exit_com_rax()]);
    assert_eq!(de_uma_vez as i64, ERR_NOMEM);

    // O heap inteiro (1 MiB) cabe; qualquer página a mais já não cabe.
    let esgotado = rodar_sintetico(&[
        &alloc_call(1024 * 1024),
        &alloc_call(4096),
        &exit_com_rax(),
    ]);
    assert_eq!(esgotado as i64, ERR_NOMEM);

    // A falha não vazou nada e o heap recomeça a cada `run`: um programa novo
    // volta a receber o início do heap.
    let depois = rodar_sintetico(&[&alloc_call(4096), &exit_com_rax()]);
    assert_eq!(depois, USER_HEAP_START);
}

#[test_case]
fn heap_e_devolvido_no_fim() {
    rodar_sintetico(&[&alloc_call(1024 * 1024), &exit_com_rax()]);
    assert!(regiao_do_usuario_esta_livre());
}

#[test_case]
fn alocar_mais_que_a_memoria_fisica_sem_vazar() {
    // 150 execuções de 1 MiB somam 150 MiB, mais que a memória utilizável do
    // QEMU (~120 MiB): se o heap deixasse de devolver os frames no fim, este
    // teste falharia antes de terminar.
    for _ in 0..150 {
        let code = rodar_sintetico(&[&alloc_call(1024 * 1024), &exit_com_rax()]);
        assert_eq!(code, USER_HEAP_START);
    }
    assert!(regiao_do_usuario_esta_livre());
}

#[test_case]
fn escrita_alem_do_heap_e_pf_e_o_kernel_segue_vivo() {
    // alloc(4096) ; mov byte [rax + 4096], 1  (uma página além do fim do heap)
    let alem: &[u8] = &[0xc6, 0x80, 0x00, 0x10, 0x00, 0x00, 0x01];
    let mut code = alloc_call(4096);
    code.extend_from_slice(alem);
    let resultado = user::run_image(&synth_elf(&code)).expect("ELF valido");
    match resultado {
        Termination::Fault { mnemonic: "#PF", .. } => {}
        outro => panic!("esperava #PF, veio {:?}", outro),
    }
    assert!(regiao_do_usuario_esta_livre());
    assert_eq!(user::run("hello"), Ok(Termination::Exit { code: 0 }));
}

// ---------------------------------------------------------------------------
// SYS_READ_LINE inválida e o prompt retomando o teclado (FR-004, FR-013)
// ---------------------------------------------------------------------------

/// `read_line(ptr, len)`: `mov eax, 3 ; movabs rdi, ptr ; mov esi, len ; syscall`.
fn read_line_call(ptr: u64, len: u32) -> Vec<u8> {
    let mut code = Vec::new();
    code.extend_from_slice(&[0xb8, 0x03, 0x00, 0x00, 0x00]); // mov eax, 3 (SYS_READ_LINE)
    code.extend_from_slice(&[0x48, 0xbf]); // movabs rdi, imm64
    code.extend_from_slice(&ptr.to_le_bytes());
    code.push(0xbe); // mov esi, imm32
    code.extend_from_slice(&len.to_le_bytes());
    code.extend_from_slice(&[0x0f, 0x05]); // syscall
    code
}

// Nenhum destes testes empurra tecla: a syscall precisa falhar **antes** de
// esperar. Se esperasse, o teste travaria até o `test-timeout`.

#[test_case]
fn read_line_com_ponteiro_do_kernel_devolve_fault() {
    reset_teclado();
    // Início do heap do kernel: mapeado só para o kernel.
    let code = rodar_sintetico(&[&read_line_call(0x4444_4444_0000, 16), &exit_com_rax()]);
    assert_eq!(code as i64, ERR_FAULT);
}

#[test_case]
fn read_line_em_pagina_somente_leitura_devolve_fault() {
    reset_teclado();
    // A página de código do próprio programa (`R E`): não é gravável.
    let code = rodar_sintetico(&[&read_line_call(USER_REGION_START, 16), &exit_com_rax()]);
    assert_eq!(code as i64, ERR_FAULT);
}

#[test_case]
fn read_line_grande_demais_devolve_inval() {
    reset_teclado();
    let code = rodar_sintetico(&[&read_line_call(USER_STACK_BOTTOM, 5000), &exit_com_rax()]);
    assert_eq!(code as i64, ERR_INVAL);
}

#[test_case]
fn read_line_de_tamanho_zero_devolve_zero_sem_esperar() {
    reset_teclado();
    let code = rodar_sintetico(&[&read_line_call(USER_STACK_BOTTOM, 0), &exit_com_rax()]);
    assert_eq!(code, 0);
}

#[test_case]
fn kernel_segue_vivo_depois_de_read_line_invalida() {
    reset_teclado();
    rodar_sintetico(&[&read_line_call(0x4444_4444_0000, 16), &exit_com_rax()]);
    assert!(regiao_do_usuario_esta_livre());
    assert_eq!(user::run("hello"), Ok(Termination::Exit { code: 0 }));
}

#[test_case]
fn prompt_retoma_o_teclado_depois_do_programa() {
    reset_teclado();
    shell::feed(b'\n'); // linha do prompt vazia
    vga_buffer::clear_screen();
    empurrar("oi\n");
    assert_eq!(user::run("eco"), Ok(Termination::Exit { code: 0 }));

    // Depois do programa, o prompt é o leitor de novo.
    vga_buffer::clear_screen();
    empurrar("sobre\n");
    shell::poll_keyboard();
    assert!(vga_buffer::screen_contains(os_rust::VERSION));
}

#[test_case]
fn teclas_digitadas_com_antecedencia_chegam_ao_prompt() {
    reset_teclado();
    shell::feed(b'\n'); // linha do prompt vazia
    vga_buffer::clear_screen();

    // Digitadas **antes** de o programa rodar, e `hello` não lê o teclado:
    // ficam na fila e o prompt as recebe intactas quando o programa termina.
    empurrar("sobre\n");
    assert_eq!(user::run("hello"), Ok(Termination::Exit { code: 0 }));
    shell::poll_keyboard();
    assert!(vga_buffer::screen_contains(os_rust::VERSION));
    assert!(interrupts::next_scancode().is_none());
}

// ---------------------------------------------------------------------------
// US2: `falha_memoria` é encerrado sem derrubar o kernel (FR-007, FR-013)
// ---------------------------------------------------------------------------

#[test_case]
fn falha_memoria_termina_com_pf_em_deadbeef() {
    let resultado = user::run("falha_memoria").expect("falha_memoria carrega");
    match resultado {
        Termination::Fault {
            mnemonic,
            error_code,
            fault_address,
            ..
        } => {
            assert_eq!(mnemonic, "#PF");
            // Bits do código de erro: 0x4 = veio de ring 3, 0x2 = escrita, e o
            // bit 0 zerado = página ausente (o endereço não é mapeado).
            assert_eq!(error_code, Some(0x6));
            assert_eq!(fault_address, Some(0xdead_beef));
        }
        outro => panic!("esperava #PF, veio {:?}", outro),
    }
}

#[test_case]
fn mensagem_de_erro_de_memoria_na_tela() {
    reset_teclado();
    shell::feed(b'\n'); // linha do prompt vazia
    vga_buffer::clear_screen();
    digitar_no_prompt("run falha_memoria");
    // O aviso do próprio programa, antes de errar.
    assert!(vga_buffer::screen_contains("prestes a acessar memoria invalida"));
    // A mensagem do kernel: legível, com o tipo do erro e o endereço.
    assert!(vga_buffer::screen_contains("encerrado por erro de memoria"));
    assert!(vga_buffer::screen_contains("0xdeadbeef"));
    // E a linha depois do acesso inválido nunca é alcançada.
    assert!(!vga_buffer::screen_contains("o isolamento falhou"));
}

#[test_case]
fn kernel_segue_de_pe_depois_de_falha_memoria() {
    reset_teclado();
    user::run("falha_memoria").expect("falha_memoria carrega");
    assert!(regiao_do_usuario_esta_livre());

    // O próximo programa roda normalmente...
    assert_eq!(user::run("hello"), Ok(Termination::Exit { code: 0 }));
    empurrar("oi\n");
    assert_eq!(user::run("eco"), Ok(Termination::Exit { code: 0 }));

    // ...e o prompt continua respondendo ao teclado.
    shell::feed(b'\n');
    vga_buffer::clear_screen();
    empurrar("sobre\n");
    shell::poll_keyboard();
    assert!(vga_buffer::screen_contains(os_rust::VERSION));
}

#[test_case]
fn falhar_varias_vezes_nao_esgota_memoria() {
    for _ in 0..40 {
        match user::run("falha_memoria") {
            Ok(Termination::Fault { mnemonic: "#PF", .. }) => {}
            outro => panic!("esperava #PF, veio {:?}", outro),
        }
    }
    assert!(regiao_do_usuario_esta_livre());
}

#[test_case]
fn crash_continua_igual_ao_marco_5() {
    // O `crash` agora usa a biblioteca, mas o resultado observável é o de
    // sempre: `#UD`, com o `rip` dentro da primeira página do programa.
    let resultado = user::run("crash").expect("crash carrega");
    match resultado {
        Termination::Fault { mnemonic, rip, .. } => {
            assert_eq!(mnemonic, "#UD");
            assert!((USER_REGION_START..USER_REGION_START + 4096).contains(&rip));
        }
        outro => panic!("esperava #UD, veio {:?}", outro),
    }
}

// ---------------------------------------------------------------------------
// US3: o guia do programador bate com o código (FR-008, FR-009)
// ---------------------------------------------------------------------------

const GUIA: &str = include_str!("../GUIA_DO_PROGRAMADOR.md");
const ECO_RS: &str = include_str!("../programs/src/bin/eco.rs");
const FALHA_MEMORIA_RS: &str = include_str!("../programs/src/bin/falha_memoria.rs");

#[test_case]
fn guia_contem_o_codigo_dos_programas_de_exemplo() {
    // O guia é o texto que quem programa lê: o código que ele mostra tem de
    // ser, literalmente, o dos arquivos que rodam no QEMU.
    assert!(
        GUIA.contains(ECO_RS),
        "o codigo de programs/src/bin/eco.rs nao aparece no guia"
    );
    assert!(
        GUIA.contains(FALHA_MEMORIA_RS),
        "o codigo de programs/src/bin/falha_memoria.rs nao aparece no guia"
    );
}

#[test_case]
fn guia_cita_o_contrato_e_a_biblioteca() {
    for termo in [
        "SYSCALLS.md",
        "SYS_WRITE",
        "SYS_EXIT",
        "SYS_READ_LINE",
        "SYS_ALLOC",
        "entry!(main)",
        "programs/src/bin/",
        "read_line",
        "println!",
    ] {
        assert!(GUIA.contains(termo), "o guia nao cita `{}`", termo);
    }
}

#[test_case]
fn nomes_de_programas_sao_unicos_e_ordenados() {
    let nomes: Vec<&str> = user::program_names().collect();
    for esperado in ["crash", "eco", "falha_memoria", "hello"] {
        assert!(nomes.contains(&esperado), "programa ausente: {}", esperado);
    }
    // Sem repetição, nem comparando sem diferenciar maiúsculas de minúsculas
    // (um sistema de arquivos que ignora a caixa juntaria os dois).
    for (i, a) in nomes.iter().enumerate() {
        for b in &nomes[i + 1..] {
            assert!(!a.eq_ignore_ascii_case(b), "nomes repetidos: {} e {}", a, b);
        }
    }
    // Em ordem alfabética.
    assert!(nomes.windows(2).all(|par| par[0] < par[1]));
}
