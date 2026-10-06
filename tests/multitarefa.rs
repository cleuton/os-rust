//! Testes de integração do Marco 7 (multitarefa): carregam vários programas em
//! ring 3 ao mesmo tempo, de verdade, dentro do QEMU, e conferem a troca de
//! contexto cooperativa e preemptiva, o isolamento de memória, o isolamento de
//! falhas e o teclado com mais de um programa. Cada arquivo de `tests/` é um
//! kernel próprio.
//!
//! Nenhum teste depende de teclado físico: eles "digitam" empurrando scancodes
//! na fila do kernel (`interrupts::push_scancode`) e os programas os leem pelo
//! caminho real (`SYS_READ_LINE`). Nenhum depende de tempo de relógio: os
//! testes de preempção conferem propriedades observáveis (todos terminam,
//! houve troca causada pelo timer, a ordem dos términos).
//!
//! Os programas de apoio são ELFs montados à mão, com um mini-assembler de
//! alguns bytes de código de máquina (`Asm`, abaixo), cada instrução com o
//! assembly equivalente em comentário.

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
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use os_rust::user::{
    self, Finished, RunReport, Termination, USER_HEAP_START, USER_REGION_START, USER_STACK_BOTTOM,
    USER_STACK_PAGES,
};
use os_rust::{interrupts, keyboard, memory, scheduler, shell, timer, vga_buffer};
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

/// Laço "grande": atravessa dezenas de ticks do timer mesmo numa máquina muito
/// rápida. Usado pelos programas que precisam ser interrompidos.
const LACO_GRANDE: u32 = 200_000_000;

/// Laço "pequeno": termina em microssegundos, bem antes do primeiro tick.
const LACO_PEQUENO: u32 = 1_000;

/// Monta um ELF64 `ET_EXEC` x86-64 com um único segmento `PT_LOAD` `R E` em
/// `USER_REGION_START`, contendo `code`, com a entrada no primeiro byte. Campos
/// usados (ver `src/elf.rs`): cabeçalho de 64 bytes, um *program header* de 56
/// bytes logo depois, e o código em seguida.
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

/// Os registradores de uso geral, com o número que o código de máquina usa.
#[derive(Clone, Copy)]
#[allow(dead_code)]
enum Reg {
    Rax = 0,
    Rcx = 1,
    Rdx = 2,
    Rbx = 3,
    Rsp = 4,
    Rbp = 5,
    Rsi = 6,
    Rdi = 7,
    R8 = 8,
    R9 = 9,
    R10 = 10,
    R11 = 11,
    R12 = 12,
    R13 = 13,
    R14 = 14,
    R15 = 15,
}

use Reg::*;

/// Um mini-assembler: só as instruções que os programas de apoio usam.
struct Asm(Vec<u8>);

impl Asm {
    fn new() -> Asm {
        Asm(Vec::new())
    }

    fn pos(&self) -> usize {
        self.0.len()
    }

    /// `mov r32, imm32` (zera a metade alta do registrador de 64 bits).
    fn mov32(&mut self, reg: Reg, imm: u32) {
        let n = reg as u8;
        if n >= 8 {
            self.0.push(0x41); // REX.B
        }
        self.0.push(0xB8 + (n & 7));
        self.0.extend_from_slice(&imm.to_le_bytes());
    }

    /// `movabs r64, imm64`.
    fn movabs(&mut self, reg: Reg, imm: u64) {
        let n = reg as u8;
        self.0.push(0x48 | (n >> 3)); // REX.W (+ REX.B)
        self.0.push(0xB8 + (n & 7));
        self.0.extend_from_slice(&imm.to_le_bytes());
    }

    /// `syscall`.
    fn syscall(&mut self) {
        self.0.extend_from_slice(&[0x0F, 0x05]);
    }

    /// `cmp a, b` (calcula `a - b` e liga as flags).
    fn cmp(&mut self, a: Reg, b: Reg) {
        let (a, b) = (a as u8, b as u8);
        self.0.push(0x48 | ((b >> 3) << 2) | (a >> 3)); // REX.W, REX.R, REX.B
        self.0.push(0x39);
        self.0.push(0xC0 | ((b & 7) << 3) | (a & 7));
    }

    /// `cmp r64, 0`.
    fn cmp_zero(&mut self, reg: Reg) {
        let n = reg as u8;
        self.0.push(0x48 | (n >> 3));
        self.0.push(0x83);
        self.0.push(0xF8 | (n & 7));
        self.0.push(0x00);
    }

    /// `push r64`.
    fn push(&mut self, reg: Reg) {
        let n = reg as u8;
        if n >= 8 {
            self.0.push(0x41);
        }
        self.0.push(0x50 + (n & 7));
    }

    /// `pop r64`.
    fn pop(&mut self, reg: Reg) {
        let n = reg as u8;
        if n >= 8 {
            self.0.push(0x41);
        }
        self.0.push(0x58 + (n & 7));
    }

    /// `mov [base], src` (64 bits; `base` não pode ser `rsp`, `rbp`, `r12`, `r13`).
    fn store(&mut self, base: Reg, src: Reg) {
        let (base, src) = (base as u8, src as u8);
        self.0.push(0x48 | ((src >> 3) << 2) | (base >> 3));
        self.0.push(0x89);
        self.0.push(((src & 7) << 3) | (base & 7));
    }

    /// `mov dst, [base]` (64 bits; mesma restrição de `store`).
    fn load(&mut self, dst: Reg, base: Reg) {
        let (dst, base) = (dst as u8, base as u8);
        self.0.push(0x48 | ((dst >> 3) << 2) | (base >> 3));
        self.0.push(0x8B);
        self.0.push(((dst & 7) << 3) | (base & 7));
    }

    /// `exit(code)`: `mov edi, code; mov eax, 2; syscall`.
    fn exit(&mut self, code: u32) {
        self.mov32(Rdi, code);
        self.mov32(Rax, 2);
        self.syscall();
    }

    /// Tamanho em bytes de `exit`: 5 + 5 + 2.
    const EXIT_LEN: u8 = 12;

    /// Depois de uma comparação: se as duas coisas eram **iguais**, pula o
    /// `exit(code)` que vem logo depois (`je +12`); senão, o programa sai com
    /// `code` (um número que diz qual conferência falhou).
    fn exige_igual_ou_sai(&mut self, code: u32) {
        self.0.extend_from_slice(&[0x74, Asm::EXIT_LEN]); // je +12
        self.exit(code);
    }

    /// Confere que `reg` vale `esperado`, ou sai com `code`. Usa `rax` de
    /// rascunho, então `reg` não pode ser `rax`.
    fn confere(&mut self, reg: Reg, esperado: u64, code: u32) {
        self.movabs(Rax, esperado); // movabs rax, esperado
        self.cmp(reg, Rax); // cmp reg, rax
        self.exige_igual_ou_sai(code);
    }

    fn into_elf(self) -> Vec<u8> {
        synth_elf(&self.0)
    }
}

/// Os registradores que o contrato manda preservar numa syscall, e que
/// `elf_cede_e_confere` confere (`rcx` e `r11` ficam de fora: o contrato os
/// declara destruídos).
const REGISTRADORES: [Reg; 12] = [Rbx, Rbp, Rdi, Rsi, Rdx, R8, R9, R10, R12, R13, R14, R15];

/// Programa que prova que o contexto sobrevive a uma troca: pede uma página de
/// heap e grava um valor; carrega um valor diferente em cada registrador
/// preservado; empilha outro valor; **cede a CPU**; e ao voltar confere tudo
/// (registradores, pilha e heap). Sai com `0` se tudo está igual, ou com
/// `10 + n` se a conferência `n` falhou.
fn elf_cede_e_confere(seed: u64) -> Vec<u8> {
    let valor = |k: u64| (seed << 32) | (0x0101_0101 * (k + 1));
    let mut a = Asm::new();
    // SYS_ALLOC(4096): rax = 0x6000_0000 (o heap começa lá).
    a.mov32(Rax, 4);
    a.mov32(Rdi, 4096);
    a.syscall();
    // Grava um valor no heap: movabs rdx, valor_heap; mov [rax], rdx.
    a.movabs(Rdx, valor(100));
    a.store(Rax, Rdx);
    // Carrega um valor diferente em cada registrador preservado.
    for (k, reg) in REGISTRADORES.iter().enumerate() {
        a.movabs(*reg, valor(k as u64));
    }
    // Empilha um valor: movabs rax, valor_pilha; push rax.
    a.movabs(Rax, valor(101));
    a.push(Rax);
    // SYS_YIELD.
    a.mov32(Rax, 5);
    a.syscall();
    // Ao voltar: `rax` vale 0 (resultado de `yield`) e o resto está como estava.
    for (k, reg) in REGISTRADORES.iter().enumerate() {
        a.confere(*reg, valor(k as u64), 10 + k as u32);
    }
    // Pilha: pop rax; movabs rcx, valor_pilha; cmp rax, rcx.
    a.pop(Rax);
    a.movabs(Rcx, valor(101));
    a.cmp(Rax, Rcx);
    a.exige_igual_ou_sai(30);
    // Heap: mov eax, 0x6000_0000; mov rax, [rax]; movabs rcx, valor_heap; cmp rax, rcx.
    a.mov32(Rax, USER_HEAP_START as u32);
    a.load(Rax, Rax);
    a.movabs(Rcx, valor(100));
    a.cmp(Rax, Rcx);
    a.exige_igual_ou_sai(31);
    a.exit(0);
    a.into_elf()
}

/// Programa que só chama `exit(code)`.
fn elf_sai(code: u8) -> Vec<u8> {
    let mut a = Asm::new();
    a.exit(code as u32);
    a.into_elf()
}

/// Programa que chama `SYS_YIELD` duas vezes **sozinho** (nenhum outro programa
/// pronto): confere que `rax` vale 0 depois de cada uma e que `rbx` e `r12`
/// não mudaram. Sai com `0`, ou com `40 + n`.
fn elf_yield_sozinho() -> Vec<u8> {
    let mut a = Asm::new();
    a.mov32(Rbx, 0x1234_5678);
    a.mov32(R12, 0x9ABC_DEF0);
    for volta in 0..2 {
        a.mov32(Rax, 5); // SYS_YIELD
        a.syscall();
        a.cmp_zero(Rax); // rax == 0 ?
        a.exige_igual_ou_sai(40 + volta);
    }
    a.movabs(Rcx, 0x1234_5678);
    a.cmp(Rbx, Rcx);
    a.exige_igual_ou_sai(42);
    a.movabs(Rcx, 0x9ABC_DEF0);
    a.cmp(R12, Rcx);
    a.exige_igual_ou_sai(43);
    a.exit(0);
    a.into_elf()
}

/// Programa que pede uma página de heap, grava `valor` no começo dela, cede a
/// CPU e, ao voltar, confere que o valor continua lá. Sai com `0` ou com `50`.
fn elf_grava_e_cede(valor: u64) -> Vec<u8> {
    let mut a = Asm::new();
    a.mov32(Rax, 4); // SYS_ALLOC
    a.mov32(Rdi, 4096);
    a.syscall(); // rax = USER_HEAP_START
    a.movabs(Rdx, valor);
    a.store(Rax, Rdx); // mov [rax], rdx
    a.mov32(Rax, 5); // SYS_YIELD
    a.syscall();
    a.mov32(Rax, USER_HEAP_START as u32);
    a.load(Rax, Rax); // mov rax, [rax]
    a.movabs(Rcx, valor);
    a.cmp(Rax, Rcx);
    a.exige_igual_ou_sai(50);
    a.exit(0);
    a.into_elf()
}

/// Programa que pede uma página de heap no **mesmo endereço** que
/// `elf_grava_e_cede` e confere que ela está zerada. Sai com `0`, ou com `51`
/// se encontrou o valor de outro programa.
fn elf_le_e_exige_zero() -> Vec<u8> {
    let mut a = Asm::new();
    a.mov32(Rax, 4); // SYS_ALLOC
    a.mov32(Rdi, 4096);
    a.syscall(); // rax = USER_HEAP_START
    a.load(Rdx, Rax); // mov rdx, [rax]
    a.cmp_zero(Rdx);
    a.exige_igual_ou_sai(51);
    a.exit(0);
    a.into_elf()
}

/// Programa que conta de `iteracoes` até zero (`dec ecx; jnz`), sem nunca
/// ceder a CPU, e sai com `0`.
fn elf_laco(iteracoes: u32) -> Vec<u8> {
    let mut a = Asm::new();
    a.mov32(Rcx, iteracoes);
    // laco: dec ecx; jnz laco
    a.0.extend_from_slice(&[0xFF, 0xC9, 0x75, 0xFC]);
    a.exit(0);
    a.into_elf()
}

/// Programa que escreve `linhas` vezes, com `SYS_WRITE`, uma linha de 40 vezes
/// `letra` e `\n`, esperando um laço longo entre uma escrita e outra para
/// atravessar vários ticks do timer. Sai com `0`.
fn elf_escreve_linhas(letra: u8, linhas: u32) -> Vec<u8> {
    let mut a = Asm::new();
    a.mov32(R12, linhas); // mov r12d, linhas
    let laco = a.pos();
    a.mov32(Rax, 1); // SYS_WRITE
    // lea rdi, [rip + linha]: o deslocamento é preenchido adiante.
    a.0.extend_from_slice(&[0x48, 0x8D, 0x3D]);
    let desloc = a.pos();
    a.0.extend_from_slice(&[0, 0, 0, 0]);
    let depois_do_lea = a.pos();
    a.mov32(Rsi, 41); // mov esi, 41
    a.syscall();
    a.mov32(Rcx, 20_000_000); // espera: mov ecx, 20_000_000
    a.0.extend_from_slice(&[0xFF, 0xC9, 0x75, 0xFC]); // espera: dec ecx; jnz espera
    // dec r12d; jnz laco
    a.0.extend_from_slice(&[0x41, 0xFF, 0xCC]);
    let depois_do_jnz = a.pos() + 2;
    let salto = (laco as isize - depois_do_jnz as isize) as i8;
    a.0.extend_from_slice(&[0x75, salto as u8]);
    a.exit(0);
    // A linha vem logo depois do código.
    let linha = a.pos();
    let disp = (linha - depois_do_lea) as u32;
    a.0[desloc..desloc + 4].copy_from_slice(&disp.to_le_bytes());
    for _ in 0..40 {
        a.0.push(letra);
    }
    a.0.push(b'\n');
    a.into_elf()
}

/// Verdadeiro se nenhuma página da região do usuário está mapeada (no espaço do
/// kernel, que é o que vale fora de uma execução): nem o início do programa,
/// nem o início do heap, nem as páginas da pilha.
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
    x86_64::instructions::interrupts::without_interrupts(|| {
        while interrupts::next_scancode().is_some() {}
    });
    keyboard::translate(0xAA); // Shift esquerdo solto
    keyboard::translate(0xB6); // Shift direito solto
}

/// Verdadeiro se a fila de scancodes está vazia. **Consome** uma tecla se
/// houver: serve para conferir no fim de um teste que nada sobrou, nunca no
/// meio de um em que as teclas ainda importam.
fn fila_de_scancodes_vazia() -> bool {
    x86_64::instructions::interrupts::without_interrupts(|| interrupts::next_scancode().is_none())
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
/// pelo mesmo `feed` que `shell::poll_keyboard` usa), e aperta Enter.
/// `shell::execute` é privada, então é este o caminho para rodar um comando.
fn digitar_no_prompt(linha: &str) {
    for byte in linha.bytes() {
        shell::feed(byte);
    }
    shell::feed(b'\n');
}

/// As linhas **não vazias** da tela, sem os espaços do fim.
fn linhas_da_tela() -> Vec<String> {
    let mut linhas = Vec::new();
    for row in 0..25 {
        let bytes = vga_buffer::screen_row_bytes(row);
        let texto: String = bytes
            .iter()
            .map(|&b| if (0x20..=0x7e).contains(&b) { b as char } else { '?' })
            .collect();
        let texto = String::from(texto.trim_end());
        if !texto.is_empty() {
            linhas.push(texto);
        }
    }
    linhas
}

/// Índice da primeira linha que contém `texto`; `panic!` clara se não houver.
fn posicao(linhas: &[String], texto: &str) -> usize {
    linhas
        .iter()
        .position(|linha| linha.contains(texto))
        .unwrap_or_else(|| panic!("a tela nao tem a linha {:?}: {:?}", texto, linhas))
}

/// A imagem ELF de um programa embutido, para misturar programas reais e
/// sintéticos em `user::run_images`.
fn imagem_de(nome: &str) -> &'static [u8] {
    os_rust::programs::PROGRAMS
        .iter()
        .find(|programa| programa.name == nome)
        .unwrap_or_else(|| panic!("programa embutido ausente: {}", nome))
        .image
}

/// Roda os programas dados e exige que o pedido seja aceito.
fn rodar(imagens: &[(&'static str, &[u8])]) -> RunReport {
    user::run_images(imagens, None).expect("o pedido de run foi recusado")
}

/// O motivo do término de `nome` no relatório.
fn termino_de<'a>(relatorio: &'a RunReport, nome: &str) -> &'a Finished {
    relatorio
        .finished
        .iter()
        .find(|f| f.name == nome)
        .unwrap_or_else(|| panic!("{} nao aparece nos terminos: {:?}", nome, relatorio.finished))
}

fn saiu_com_zero(f: &Finished) -> bool {
    f.termination == Termination::Exit { code: 0 }
}

// ---------------------------------------------------------------------------
// US1: o contexto cooperativo
// ---------------------------------------------------------------------------

#[test_case]
fn contexto_cooperativo_preserva_registradores_pilha_e_memoria() {
    let a = elf_cede_e_confere(0xA1);
    let b = elf_cede_e_confere(0xB2);
    let relatorio = rodar(&[("a", &a[..]), ("b", &b[..])]);
    // `a` e `b` carregam valores diferentes e se alternam por `SYS_YIELD`: se
    // o kernel perdesse um registrador, a pilha ou o heap de qualquer um, o
    // programa sairia com um código diferente de 0.
    assert_eq!(
        termino_de(&relatorio, "a").termination,
        Termination::Exit { code: 0 }
    );
    assert_eq!(
        termino_de(&relatorio, "b").termination,
        Termination::Exit { code: 0 }
    );
}

#[test_case]
fn ping_e_pong_alternam_a_saida() {
    vga_buffer::clear_screen();
    let relatorio = user::run_all(&["ping", "pong"], None).expect("run ping pong");
    assert!(saiu_com_zero(termino_de(&relatorio, "ping")));
    assert!(saiu_com_zero(termino_de(&relatorio, "pong")));

    let linhas = linhas_da_tela();
    let mut anterior = None;
    for i in 1..=4 {
        for nome in ["ping", "pong"] {
            let atual = posicao(&linhas, &alloc::format!("{} {}", nome, i));
            if let Some(anterior) = anterior {
                assert!(atual > anterior, "{} {} saiu fora de ordem: {:?}", nome, i, linhas);
            }
            anterior = Some(atual);
        }
    }
}

#[test_case]
fn o_que_sobra_segue_ate_o_fim() {
    vga_buffer::clear_screen();
    let curto = elf_sai(0);
    let relatorio = rodar(&[("curto", &curto[..]), ("pong", imagem_de("pong"))]);
    // O que sai primeiro não impede o outro de completar a saída.
    assert_eq!(relatorio.finished[0].name, "curto");
    assert_eq!(relatorio.finished[1].name, "pong");
    assert!(saiu_com_zero(&relatorio.finished[1]));
    let linhas = linhas_da_tela();
    posicao(&linhas, "pong 4");
}

#[test_case]
fn yield_sem_outro_pronto_volta_na_hora() {
    let sozinho = elf_yield_sozinho();
    let relatorio = rodar(&[("sozinho", &sozinho[..])]);
    assert_eq!(
        relatorio.finished[0].termination,
        Termination::Exit { code: 0 }
    );
}

// ---------------------------------------------------------------------------
// US1: o comando `run`
// ---------------------------------------------------------------------------

#[test_case]
fn run_de_um_programa_so_igual_ao_marco_6() {
    reset_teclado();
    vga_buffer::clear_screen();
    digitar_no_prompt("run hello");
    assert!(vga_buffer::screen_contains("Ola do ring 3!"));
    // O mesmo programa, pela API de um programa só do Marco 6.
    assert_eq!(user::run("hello"), Ok(Termination::Exit { code: 0 }));
}

#[test_case]
fn run_com_nome_desconhecido_recusa_tudo() {
    reset_teclado();
    vga_buffer::clear_screen();
    let antes = memory::frames_outstanding();
    digitar_no_prompt("run ping xyz");
    assert!(vga_buffer::screen_contains("programa desconhecido: xyz"));
    assert!(
        !vga_buffer::screen_contains("ping 1"),
        "nenhum programa pode ter sido iniciado"
    );
    assert_eq!(memory::frames_outstanding(), antes);
}

#[test_case]
fn run_com_mais_que_o_maximo_recusa_tudo() {
    reset_teclado();
    vga_buffer::clear_screen();
    let antes = memory::frames_outstanding();
    digitar_no_prompt("run ping pong ping pong hello");
    assert!(vga_buffer::screen_contains("no maximo 4 programas"));
    assert!(
        !vga_buffer::screen_contains("ping 1"),
        "nenhum programa pode ter sido iniciado"
    );
    assert_eq!(memory::frames_outstanding(), antes);
}

#[test_case]
fn mesmo_nome_duas_vezes_sao_instancias_independentes() {
    vga_buffer::clear_screen();
    let relatorio = user::run_all(&["ping", "ping"], None).expect("run ping ping");
    assert_eq!(relatorio.finished.len(), 2);
    assert!(relatorio.finished.iter().all(saiu_com_zero));
    // Cada instância tem a memória dela: as duas contam de 1 a 4, intercaladas.
    let pings = linhas_da_tela()
        .iter()
        .filter(|linha| linha.starts_with("ping "))
        .count();
    assert_eq!(pings, 8);
}

#[test_case]
fn rodar_dois_programas_100_vezes_devolve_todos_os_frames() {
    // Uma rodada para aquecer: cria as tabelas e a lista de frames reciclados.
    user::run_all(&["ping", "pong"], None).expect("rodada de aquecimento");
    let antes = memory::frames_outstanding();
    for _ in 0..100 {
        let relatorio = user::run_all(&["ping", "pong"], None).expect("run ping pong");
        assert_eq!(relatorio.finished.len(), 2);
    }
    assert_eq!(
        memory::frames_outstanding(),
        antes,
        "frames vazaram em 100 execucoes"
    );
    assert!(regiao_do_usuario_esta_livre());
}

#[test_case]
fn prompt_volta_a_ler_o_teclado_depois_do_ultimo_programa() {
    reset_teclado();
    vga_buffer::clear_screen();
    digitar_no_prompt("run ping pong");
    digitar_no_prompt("echo ok");
    assert!(vga_buffer::screen_contains("ok"));
    assert!(fila_de_scancodes_vazia());
}

#[test_case]
fn programas_simultaneos_nao_enxergam_a_memoria_um_do_outro() {
    let grava = elf_grava_e_cede(0xA5A5_A5A5_A5A5_A5A5);
    let le = elf_le_e_exige_zero();
    // Os dois usam o mesmo endereço virtual de heap. `grava` escreve o valor e
    // cede a CPU; `le` roda nesse intervalo, lê o mesmo endereço e só sai com 0
    // se achar zero. Se o espaço fosse compartilhado, `le` leria o valor do
    // outro (e sairia com 51).
    let relatorio = rodar(&[("grava", &grava[..]), ("le", &le[..])]);
    assert_eq!(
        termino_de(&relatorio, "le").termination,
        Termination::Exit { code: 0 },
        "o segundo programa enxergou a memoria do primeiro"
    );
    assert_eq!(
        termino_de(&relatorio, "grava").termination,
        Termination::Exit { code: 0 },
        "o primeiro programa perdeu o valor que gravou"
    );
    assert!(regiao_do_usuario_esta_livre());
}

#[test_case]
fn a_conferencia_dos_programas_de_apoio_detecta_diferenca() {
    // Prova que `exige_igual_ou_sai` não é vazia: um registrador que **não**
    // vale o esperado faz o programa sair com o código da conferência, em vez
    // de 0. Sem isto, os testes acima passariam mesmo com um assembler errado.
    let mut a = Asm::new();
    a.mov32(Rbx, 1);
    a.confere(Rbx, 2, 77);
    a.exit(0);
    let programa = a.into_elf();
    let relatorio = rodar(&[("errado", &programa[..])]);
    assert_eq!(
        relatorio.finished[0].termination,
        Termination::Exit { code: 77 }
    );
}

// ---------------------------------------------------------------------------
// US2: a preempção
// ---------------------------------------------------------------------------

/// Espera, com as interrupções ligadas, até o timer avançar `quantos` ticks, e
/// as desliga de novo. Tem um teto de voltas: se o timer não estiver vivo (ou o
/// EOI faltou), falha com uma mensagem clara em vez de travar.
fn esperar_ticks(quantos: u64) {
    let inicio = timer::ticks();
    x86_64::instructions::interrupts::enable();
    let mut voltas = 0;
    while timer::ticks() < inicio + quantos {
        x86_64::instructions::interrupts::enable_and_hlt();
        voltas += 1;
        assert!(
            voltas < 10_000,
            "o timer nao avancou {} ticks (parou em {})",
            quantos,
            timer::ticks() - inicio
        );
    }
    x86_64::instructions::interrupts::disable();
}

#[test_case]
fn contadores_nao_cedem_a_cpu() {
    // A alternância de `contador_a` e `contador_b` só prova a preempção se
    // nenhum dos dois pede a vez: o nome da syscall não pode aparecer no texto
    // deles (nem nos comentários). O nome é montado em pedaços para este
    // arquivo não conter o texto que procura.
    let ceder = ["yi", "eld"].concat();
    for (arquivo, texto) in [
        ("contador_a.rs", include_str!("../programs/src/bin/contador_a.rs")),
        ("contador_b.rs", include_str!("../programs/src/bin/contador_b.rs")),
    ] {
        assert!(
            !texto.contains(ceder.as_str()),
            "{} cita a syscall de ceder a CPU",
            arquivo
        );
    }
}

#[test_case]
fn preempcao_intercala_dois_programas_que_nunca_cedem() {
    vga_buffer::clear_screen();
    let relatorio =
        user::run_all(&["contador_a", "contador_b"], None).expect("run contador_a contador_b");
    assert!(saiu_com_zero(termino_de(&relatorio, "contador_a")));
    assert!(saiu_com_zero(termino_de(&relatorio, "contador_b")));
    // Nenhum dos dois cede a CPU, então toda troca foi causada pelo timer.
    assert!(
        relatorio.preemptions >= 2,
        "o timer so trocou {} vezes",
        relatorio.preemptions
    );
    // Cada um rodou em mais de uma fatia antes de terminar: progrediram
    // alternadamente.
    for f in &relatorio.finished {
        assert!(f.slices >= 2, "{} rodou numa fatia so", f.name);
    }
    // E a saída dos dois aparece intercalada: depois da primeira linha de um
    // vem uma do outro antes de a oitava do primeiro.
    let linhas = linhas_da_tela();
    let a8 = posicao(&linhas, "A: 8");
    let b8 = posicao(&linhas, "B: 8");
    let b1 = posicao(&linhas, "B: 1");
    let a1 = posicao(&linhas, "A: 1");
    assert!(b1 < a8 && a1 < b8, "as saidas nao se intercalaram: {:?}", linhas);
}

#[test_case]
fn programa_curto_termina_antes_do_longo() {
    // O longo é o primeiro da lista: começa a rodar antes e, sem preempção,
    // monopolizaria a CPU. Com o timer, o curto termina primeiro.
    let longo = elf_laco(LACO_GRANDE);
    let curto = elf_laco(LACO_PEQUENO);
    let relatorio = rodar(&[("longo", &longo[..]), ("curto", &curto[..])]);
    assert_eq!(relatorio.finished[0].name, "curto");
    assert_eq!(relatorio.finished[1].name, "longo");
    assert!(
        termino_de(&relatorio, "longo").slices >= 2,
        "o longo nao foi interrompido"
    );
}

#[test_case]
fn timer_em_ring_0_nao_troca_de_contexto() {
    reset_teclado();
    vga_buffer::clear_screen();
    // Nenhum programa em execução: o tick só conta, avisa o PIC e volta. Nada
    // quebra e o prompt segue funcionando.
    esperar_ticks(3);
    digitar_no_prompt("echo ok");
    assert!(vga_buffer::screen_contains("ok"));
}

#[test_case]
fn eoi_nunca_fica_pendente_depois_de_trocas() {
    // Depois de muitas trocas (as dos contadores), a IRQ0 continua chegando: se
    // o EOI tivesse faltado em algum caminho, o PIC pararia de entregá-la e
    // `esperar_ticks` falharia com mensagem clara.
    user::run_all(&["contador_a", "contador_b"], None).expect("run contador_a contador_b");
    esperar_ticks(3);
}

#[test_case]
fn write_sai_inteiro_mesmo_sob_preempcao() {
    vga_buffer::clear_screen();
    let a = elf_escreve_linhas(b'a', 10);
    let b = elf_escreve_linhas(b'b', 10);
    let relatorio = rodar(&[("a", &a[..]), ("b", &b[..])]);
    assert!(
        relatorio.preemptions >= 1,
        "o teste so vale se o timer trocou de programa"
    );
    // Cada `write` entrega a linha inteira: nenhuma linha da tela mistura as
    // duas letras (um `write` partido por uma troca deixaria uma linha mista).
    let linhas = linhas_da_tela();
    assert_eq!(linhas.len(), 20, "esperava 20 linhas, veio {:?}", linhas);
    for linha in &linhas {
        let letra = linha.chars().next().expect("linha vazia");
        assert!(letra == 'a' || letra == 'b', "linha estranha: {:?}", linha);
        assert!(
            linha.len() == 40 && linha.chars().all(|c| c == letra),
            "uma linha saiu partida ou misturada: {:?}",
            linha
        );
    }
}

// ---------------------------------------------------------------------------
// US3: uma falha em um programa não derruba o outro
// ---------------------------------------------------------------------------

/// Quantas linhas da tela começam com `prefixo`.
fn linhas_que_comecam_com(prefixo: &str) -> usize {
    linhas_da_tela()
        .iter()
        .filter(|linha| linha.starts_with(prefixo))
        .count()
}

#[test_case]
fn falha_de_um_programa_nao_afeta_o_outro() {
    let relatorio = user::run_all(&["falha_memoria", "contador_a"], None)
        .expect("run falha_memoria contador_a");
    match termino_de(&relatorio, "falha_memoria").termination {
        Termination::Fault { mnemonic: "#PF", fault_address: Some(0xdead_beef), .. } => {}
        outro => panic!("esperava #PF em 0xdeadbeef, veio {:?}", outro),
    }
    assert!(saiu_com_zero(termino_de(&relatorio, "contador_a")));
    // Quem falhou terminou primeiro, e o outro seguiu até o fim.
    assert_eq!(relatorio.finished[0].name, "falha_memoria");
}

#[test_case]
fn a_mensagem_de_erro_aparece_na_hora_e_o_outro_completa_a_saida() {
    reset_teclado();
    vga_buffer::clear_screen();
    // Pelo prompt, que é quem mostra a mensagem de término (na hora em que cada
    // programa termina).
    digitar_no_prompt("run falha_memoria contador_a");
    let linhas = linhas_da_tela();
    let erro = posicao(&linhas, "[run] falha_memoria encerrado por erro de memoria");
    let ultima = posicao(&linhas, "A: 8");
    assert!(erro < ultima, "a mensagem de erro devia vir antes do fim do contador");
    // O contador escreveu as 8 linhas dele inteiras.
    assert_eq!(linhas_que_comecam_com("A: "), 8);
}

#[test_case]
fn falha_de_cada_tipo_nao_derruba_o_vizinho() {
    vga_buffer::clear_screen();
    // `crash` provoca um `#UD`; `ping` segue e escreve as quatro linhas.
    let relatorio = user::run_all(&["crash", "ping"], None).expect("run crash ping");
    match termino_de(&relatorio, "crash").termination {
        Termination::Fault { mnemonic: "#UD", .. } => {}
        outro => panic!("esperava #UD, veio {:?}", outro),
    }
    assert!(saiu_com_zero(termino_de(&relatorio, "ping")));
    assert_eq!(linhas_que_comecam_com("ping "), 4);
}

#[test_case]
fn depois_da_falha_o_prompt_e_run_hello_funcionam() {
    reset_teclado();
    vga_buffer::clear_screen();
    digitar_no_prompt("run falha_memoria contador_a");
    vga_buffer::clear_screen();
    digitar_no_prompt("run hello");
    assert!(vga_buffer::screen_contains("Ola do ring 3!"));
    assert!(regiao_do_usuario_esta_livre());
}

#[test_case]
fn exit_de_um_programa_nao_encerra_os_outros() {
    let sai = elf_sai(7);
    let relatorio = rodar(&[("sai7", &sai[..]), ("contador_a", imagem_de("contador_a"))]);
    assert_eq!(
        termino_de(&relatorio, "sai7").termination,
        Termination::Exit { code: 7 }
    );
    assert!(saiu_com_zero(termino_de(&relatorio, "contador_a")));
}

/// Os frames em uso no instante de cada término, na ordem em que aconteceram.
static FRAMES_NO_TERMINO: [AtomicUsize; 3] =
    [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];
/// Quantos términos já foram anotados.
static TERMINOS_ANOTADOS: AtomicUsize = AtomicUsize::new(0);

/// Aviso de término usado por `recursos_do_que_falhou_voltam_com_o_outro_vivo`.
/// O escalonador o chama **antes** de devolver os recursos de quem terminou.
fn anota_frames(_terminou: &Finished) {
    let indice = TERMINOS_ANOTADOS.fetch_add(1, Ordering::Relaxed);
    if indice < FRAMES_NO_TERMINO.len() {
        FRAMES_NO_TERMINO[indice].store(memory::frames_outstanding(), Ordering::Relaxed);
    }
}

#[test_case]
fn recursos_do_que_falhou_voltam_com_o_outro_vivo() {
    let curto = elf_sai(0);
    let laco = elf_laco(LACO_GRANDE);
    let antes = memory::frames_outstanding();
    TERMINOS_ANOTADOS.store(0, Ordering::Relaxed);
    let relatorio = user::run_images(
        &[
            ("falha_memoria", imagem_de("falha_memoria")),
            ("curto", &curto[..]),
            ("laco", &laco[..]),
        ],
        Some(anota_frames),
    )
    .expect("run das tres tarefas");
    assert_eq!(relatorio.finished.len(), 3);
    let [primeiro, segundo, terceiro] = [
        FRAMES_NO_TERMINO[0].load(Ordering::Relaxed),
        FRAMES_NO_TERMINO[1].load(Ordering::Relaxed),
        FRAMES_NO_TERMINO[2].load(Ordering::Relaxed),
    ];
    // O aviso vem antes de os recursos de quem terminou voltarem. Então, no
    // primeiro término, as três tarefas ainda tinham os frames delas; no
    // segundo, os do primeiro já tinham voltado, com as outras duas vivas; no
    // terceiro, os do segundo também. A ordem dos términos não importa: o que
    // se prova é que cada tarefa devolve os seus frames na hora em que termina,
    // enquanto as outras continuam vivas.
    assert!(primeiro > antes, "as tres tarefas deviam ter frames em uso");
    assert!(
        segundo < primeiro,
        "os frames do primeiro a terminar nao voltaram ({} -> {})",
        primeiro,
        segundo
    );
    assert!(
        terceiro < segundo,
        "os frames do segundo a terminar nao voltaram ({} -> {})",
        segundo,
        terceiro
    );
    // No fim, tudo voltou.
    assert_eq!(memory::frames_outstanding(), antes);
}

// ---------------------------------------------------------------------------
// US4: o teclado com mais de um programa
// ---------------------------------------------------------------------------

/// As linhas da tela que **começam** com `prefixo`, na ordem em que aparecem.
fn linhas_com_prefixo(prefixo: &str) -> Vec<String> {
    linhas_da_tela()
        .into_iter()
        .filter(|linha| linha.starts_with(prefixo))
        .collect()
}

#[test_case]
fn teclado_vai_ao_que_pediu_primeiro() {
    reset_teclado();
    vga_buffer::clear_screen();
    // As duas linhas já estão na fila quando os programas pedem: `eco` pede
    // primeiro (é o primeiro da lista) e recebe a primeira; `eco2` recebe a
    // segunda.
    empurrar("ab\ncd\n");
    let relatorio = user::run_all(&["eco", "eco2"], None).expect("run eco eco2");
    assert!(saiu_com_zero(termino_de(&relatorio, "eco")));
    assert!(saiu_com_zero(termino_de(&relatorio, "eco2")));
    assert_eq!(linhas_com_prefixo("voce digitou:"), ["voce digitou: ab"]);
    assert_eq!(linhas_com_prefixo("eco2: voce digitou:"), ["eco2: voce digitou: cd"]);
    // Nenhuma tecla se perdeu nem sobrou.
    assert!(fila_de_scancodes_vazia());
}

#[test_case]
fn tecla_sobrando_continua_na_fila_para_o_prompt() {
    reset_teclado();
    vga_buffer::clear_screen();
    // Um leitor só: `eco` consome `ab`; `cd` e o Enter ficam na fila.
    empurrar("ab\ncd\n");
    let relatorio = user::run_all(&["eco"], None).expect("run eco");
    assert!(saiu_com_zero(termino_de(&relatorio, "eco")));
    // O prompt volta a ser o leitor e recebe o que sobrou: `cd` vira um comando.
    // (Olhar a fila para ver se há algo sobrando consumiria uma tecla; é o
    // próprio prompt que prova que `cd` continuou lá.)
    shell::poll_keyboard();
    assert!(
        vga_buffer::screen_contains("comando desconhecido: cd"),
        "tela: {:?}",
        linhas_da_tela()
    );
}

/// Quantas vezes o gancho de teste foi chamado na rodada atual.
static CHAMADAS: AtomicUsize = AtomicUsize::new(0);

/// Gancho do teste dos programas bloqueados: com os dois programas esperando o
/// teclado, só "digita" depois de o kernel ter dormido várias vezes. As três
/// primeiras chamadas vêm da partida e dos dois bloqueios; as seguintes só
/// podem vir do laço ocioso.
fn digita_depois_de_dormir() {
    let chamada = CHAMADAS.fetch_add(1, Ordering::Relaxed) + 1;
    if chamada == 10 {
        empurrar("ab\n");
    } else if chamada == 20 {
        empurrar("cd\n");
    }
}

#[test_case]
fn programas_bloqueados_no_teclado_nao_consomem_cpu() {
    reset_teclado();
    vga_buffer::clear_screen();
    CHAMADAS.store(0, Ordering::Relaxed);
    scheduler::set_poll_hook(Some(digita_depois_de_dormir));
    let ticks_antes = timer::ticks();
    let relatorio = user::run_all(&["eco", "eco2"], None);
    scheduler::set_poll_hook(None);
    let relatorio = relatorio.expect("run eco eco2");
    let ticks = timer::ticks() - ticks_antes;

    assert!(saiu_com_zero(termino_de(&relatorio, "eco")));
    assert!(saiu_com_zero(termino_de(&relatorio, "eco2")));
    assert_eq!(linhas_com_prefixo("voce digitou:"), ["voce digitou: ab"]);
    assert_eq!(linhas_com_prefixo("eco2: voce digitou:"), ["eco2: voce digitou: cd"]);

    // O kernel dormiu esperando as teclas, e **dormiu de verdade**: cada volta
    // do laço ocioso foi provocada por uma interrupção (um tick), então não há
    // mais voltas do que ticks. Um laço ocupado daria milhares.
    assert!(
        relatorio.idle_loops >= 5,
        "o kernel nao ficou ocioso: {} voltas",
        relatorio.idle_loops
    );
    assert!(
        (relatorio.idle_loops as u64) <= ticks + 3,
        "{} voltas ociosas para {} ticks: o kernel esta girando em laco ocupado",
        relatorio.idle_loops,
        ticks
    );
}

/// Verdadeiro depois que o gancho de `bloqueado_nao_impede_o_pronto_de_rodar`
/// empurrou a linha.
static LINHA_ENVIADA: AtomicBool = AtomicBool::new(false);

/// Gancho: só "digita" quando o último `ping` já está na tela, isto é, depois
/// que o programa pronto terminou a saída dele.
fn digita_depois_do_ping() {
    if !LINHA_ENVIADA.load(Ordering::Relaxed) && vga_buffer::screen_contains("ping 4") {
        LINHA_ENVIADA.store(true, Ordering::Relaxed);
        empurrar("hi\n");
    }
}

#[test_case]
fn bloqueado_nao_impede_o_pronto_de_rodar() {
    reset_teclado();
    vga_buffer::clear_screen();
    LINHA_ENVIADA.store(false, Ordering::Relaxed);
    scheduler::set_poll_hook(Some(digita_depois_do_ping));
    let relatorio = user::run_all(&["eco", "ping"], None);
    scheduler::set_poll_hook(None);
    let relatorio = relatorio.expect("run eco ping");
    assert!(saiu_com_zero(termino_de(&relatorio, "eco")));
    assert!(saiu_com_zero(termino_de(&relatorio, "ping")));
    // `eco` ficou bloqueado, sem teclado, e nem por isso o `ping` deixou de
    // rodar: as quatro linhas dele apareceram antes de qualquer tecla.
    let linhas = linhas_da_tela();
    assert!(posicao(&linhas, "ping 4") < posicao(&linhas, "voce digitou: hi"));
}

/// Gancho de `linha_parcial_pertence_a_quem_pediu_primeiro`: aperta o Enter da
/// linha que já estava pela metade, e mais tarde digita a linha do segundo.
fn termina_a_linha_parcial() {
    let chamada = CHAMADAS.fetch_add(1, Ordering::Relaxed) + 1;
    if chamada == 6 {
        empurrar("\n");
    } else if chamada == 12 {
        empurrar("cd\n");
    }
}

#[test_case]
fn linha_parcial_pertence_a_quem_pediu_primeiro() {
    reset_teclado();
    vga_buffer::clear_screen();
    CHAMADAS.store(0, Ordering::Relaxed);
    // `ab` fica digitado, sem Enter, antes de o segundo programa pedir.
    empurrar("ab");
    scheduler::set_poll_hook(Some(termina_a_linha_parcial));
    let relatorio = user::run_all(&["eco", "eco2"], None);
    scheduler::set_poll_hook(None);
    let relatorio = relatorio.expect("run eco eco2");
    assert!(saiu_com_zero(termino_de(&relatorio, "eco")));
    assert!(saiu_com_zero(termino_de(&relatorio, "eco2")));
    // O que estava pela metade era do que pediu primeiro, mesmo com o outro
    // programa pedindo depois; o segundo só recebeu a linha seguinte.
    assert_eq!(linhas_com_prefixo("voce digitou:"), ["voce digitou: ab"]);
    assert_eq!(linhas_com_prefixo("eco2: voce digitou:"), ["eco2: voce digitou: cd"]);
}

/// Conta as chamadas e, na 6ª, empurra a linha. Com `eco` bloqueado e só o laço
/// pronto, as chamadas só podem vir dos ticks.
fn digita_durante_o_laco() {
    if CHAMADAS.fetch_add(1, Ordering::Relaxed) + 1 == 6 {
        empurrar("hi\n");
    }
}

#[test_case]
fn tecla_chega_enquanto_outro_programa_computa() {
    reset_teclado();
    vga_buffer::clear_screen();
    CHAMADAS.store(0, Ordering::Relaxed);
    let laco = elf_laco(LACO_GRANDE);
    scheduler::set_poll_hook(Some(digita_durante_o_laco));
    let relatorio =
        user::run_images(&[("eco", imagem_de("eco")), ("laco", &laco[..])], None);
    scheduler::set_poll_hook(None);
    let relatorio = relatorio.expect("run eco laco");
    assert!(saiu_com_zero(termino_de(&relatorio, "eco")));
    assert!(saiu_com_zero(termino_de(&relatorio, "laco")));
    assert!(relatorio.preemptions >= 1);
    // A linha foi entregue por um tick, com o laço computando: o `eco` terminou
    // antes do laço. Se `preempt` não olhasse o teclado, o `eco` só receberia a
    // linha depois do fim do laço e a ordem se inverteria.
    assert_eq!(relatorio.finished[0].name, "eco");
    assert!(vga_buffer::screen_contains("voce digitou: hi"));
}
