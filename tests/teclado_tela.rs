//! Testes de integração do Marco 13: tecla crua (`SYS_READ_KEY`), tela e cores
//! (`SYS_SCREEN`), argumentos (`SYS_ARGS`, `exec`) e o programa `teclas`, de
//! ponta a ponta dentro do QEMU. Nenhum teste depende de teclado físico: eles
//! empurram scancodes na fila do kernel (`interrupts::push_scancode`) e os
//! programas os leem pelo caminho real. Os programas de apoio são ELFs
//! montados à mão com um mini-assembler (`Asm`).

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
use core::sync::atomic::{AtomicBool, Ordering};
use os_rust::user::{self, Finished, RunReport, Termination, USER_REGION_START};
use os_rust::{interrupts, keyboard, scheduler, shell, vga_buffer};
use spin::Mutex;

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
// Ajudantes (duplicados de propósito: cada arquivo de `tests/` é um kernel
// próprio e não há módulo comum; o padrão é o de `tests/multitarefa.rs`)
// ---------------------------------------------------------------------------

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

impl Asm {
    /// Carrega `valor` em `reg` (`mov` de 32 bits se couber, `movabs` senão).
    fn set(&mut self, reg: Reg, valor: u64) {
        if valor <= u32::MAX as u64 {
            self.mov32(reg, valor as u32);
        } else {
            self.movabs(reg, valor);
        }
    }

    /// Chama a syscall `nr` com até três argumentos (`rdi`, `rsi`, `rdx`).
    fn chama(&mut self, nr: u64, a1: u64, a2: u64, a3: u64) {
        self.set(Rdi, a1);
        self.set(Rsi, a2);
        self.set(Rdx, a3);
        self.set(Rax, nr);
        self.syscall();
    }

    /// `SYS_WRITE` de `texto`, que viaja dentro do próprio código: um `jmp`
    /// por cima dos bytes e o endereço deles calculado a partir do começo da
    /// região do usuário (o código é carregado lá, com a entrada no primeiro
    /// byte).
    fn escreve(&mut self, texto: &[u8]) {
        self.0.push(0xE9); // jmp rel32
        self.0.extend_from_slice(&(texto.len() as u32).to_le_bytes());
        let em = self.pos();
        self.0.extend_from_slice(texto);
        self.movabs(Rdi, USER_REGION_START + em as u64);
        self.mov32(Rsi, texto.len() as u32);
        self.mov32(Rax, abi::SYS_WRITE as u32);
        self.syscall();
    }

    /// Exige que o resultado da última syscall (`rax`) seja `esperado`; senão o
    /// programa sai com `code`. Usa `rbx` e `rax` de rascunho.
    fn resultado_igual(&mut self, esperado: u64, code: u32) {
        self.0.extend_from_slice(&[0x48, 0x89, 0xC3]); // mov rbx, rax
        self.confere(Rbx, esperado, code);
    }

    /// `ud2`: instrução inválida (falha de CPU, #UD).
    fn ud2(&mut self) {
        self.0.extend_from_slice(&[0x0F, 0x0B]);
    }

    /// Escreve num endereço não mapeado (falha de memória, #PF).
    fn escreve_no_nada(&mut self) {
        self.movabs(Rax, 0xdead_beef);
        self.store(Rax, Rax);
    }

    /// Guarda `rax` (o resultado da última syscall) em `[rbx]`, onde `rbx`
    /// aponta para uma página de heap (ver `aloca_dados`), e avança `rbx` 8.
    fn guarda_rax(&mut self) {
        self.store(Rbx, Rax);
        // add rbx, 8
        self.0.extend_from_slice(&[0x48, 0x83, 0xC3, 0x08]);
    }

    /// Pede uma página de heap (`SYS_ALLOC`), deixa o endereço em `rbx` (para
    /// `guarda_rax`) e em `r15` (o começo, para o programa conferir).
    fn aloca_dados(&mut self) {
        self.chama(abi::SYS_ALLOC, 4096, 0, 0);
        // mov rbx, rax ; mov r15, rax
        self.0.extend_from_slice(&[0x48, 0x89, 0xC3, 0x49, 0x89, 0xC7]);
    }
}

/// Restaura a tela: cor padrão e a tela em branco com o cursor na última linha.
fn restaurar_tela() {
    vga_buffer::restore_prompt_line();
    vga_buffer::clear_screen();
}

/// Esvazia a fila de scancodes, solta Shift e Ctrl e zera o decodificador: a
/// fila e esses estados são globais, então cada teste começa do zero.
fn reset_teclado() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        while interrupts::next_scancode().is_some() {}
    });
    keyboard::translate(0xAA); // Shift esquerdo solto
    keyboard::translate(0xB6); // Shift direito solto
    keyboard::reset_decoder();
}

/// Empurra scancodes crus na fila (que tem 16 posições).
fn empurrar(scancodes: &[u8]) {
    assert!(scancodes.len() <= 16, "a fila tem 16 posicoes");
    for &scancode in scancodes {
        interrupts::push_scancode(scancode);
    }
}

/// Digita uma linha no shell, byte a byte, como o teclado faria, e aperta Enter.
fn digitar_no_prompt(linha: &str) {
    for byte in linha.bytes() {
        shell::feed(byte);
    }
    shell::feed(b'\n');
}

/// As linhas não vazias da tela, sem os espaços do fim.
fn linhas_da_tela() -> Vec<String> {
    let mut linhas = Vec::new();
    for row in 0..25 {
        let texto: String = vga_buffer::screen_row_bytes(row)
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

/// O texto de uma linha da tela, sem os espaços do fim.
fn linha(row: usize) -> String {
    let texto: String = vga_buffer::screen_row_bytes(row)
        .iter()
        .map(|&b| if (0x20..=0x7e).contains(&b) { b as char } else { '?' })
        .collect();
    String::from(texto.trim_end())
}

/// Limpa a tela, roda o comando no prompt e devolve as linhas não vazias.
fn rodar_comando(comando: &str) -> Vec<String> {
    vga_buffer::clear_screen();
    digitar_no_prompt(comando);
    linhas_da_tela()
}

fn tem(linhas: &[String], texto: &str) -> bool {
    linhas.iter().any(|l| l.contains(texto))
}

fn exige(linhas: &[String], texto: &str) {
    assert!(tem(linhas, texto), "a tela nao tem {:?}: {:?}", texto, linhas);
}

/// Grava `elf` em `caminho` (um arquivo do `/ram`, na raiz), pela API pública
/// do kernel, para `run`/`exec` poderem iniciá-lo pelo prompt. Apaga antes o
/// que houver lá.
fn instalar_elf(caminho: &str, elf: &[u8]) {
    let _ = os_rust::fs::remove(caminho.as_bytes());
    let mut arquivo = os_rust::fs::OpenFile::create(caminho.as_bytes()).expect("cria o ELF no /ram");
    for pedaco in elf.chunks(512) {
        let mut feito = 0;
        while feito < pedaco.len() {
            feito += arquivo.write(&pedaco[feito..]).expect("grava o ELF no /ram");
        }
    }
}

/// Programa que só chama `exit(code)`.
fn elf_sai(code: u8) -> Vec<u8> {
    let mut a = Asm::new();
    a.exit(code as u32);
    a.into_elf()
}

// ---------------------------------------------------------------------------
// Rodar programas com teclas "digitadas" no meio da execução
// ---------------------------------------------------------------------------

/// Os scancodes que o gancho ainda tem de empurrar, e onde está.
struct Roteiro {
    scancodes: Vec<u8>,
    proximo: usize,
}

static ROTEIRO: Mutex<Roteiro> = Mutex::new(Roteiro { scancodes: Vec::new(), proximo: 0 });

/// Enquanto for `false`, o gancho não empurra nada (os testes que precisam
/// que uma tecla só chegue depois de um certo ponto).
static LIBERA: AtomicBool = AtomicBool::new(true);

/// O gancho do escalonador: empurra até 4 scancodes do roteiro cada vez que o
/// kernel olha o teclado, como se a pessoa digitasse durante a execução.
fn tick() {
    if !LIBERA.load(Ordering::SeqCst) {
        return;
    }
    let mut roteiro = ROTEIRO.lock();
    let mut empurrados = 0;
    while roteiro.proximo < roteiro.scancodes.len() && empurrados < 4 {
        interrupts::push_scancode(roteiro.scancodes[roteiro.proximo]);
        roteiro.proximo += 1;
        empurrados += 1;
    }
}

/// Roda os programas juntos, com `antecipadas` já na fila antes de começar
/// (digitação antecipada) e `durante` empurrado pelo gancho enquanto rodam.
fn rodar(programas: &[(&str, &Vec<u8>)], antecipadas: &[u8], durante: &[u8], on_end: Option<fn(&Finished)>) -> RunReport {
    reset_teclado();
    empurrar(antecipadas);
    {
        let mut roteiro = ROTEIRO.lock();
        roteiro.scancodes = durante.to_vec();
        roteiro.proximo = 0;
    }
    LIBERA.store(true, Ordering::SeqCst);
    scheduler::set_poll_hook(Some(tick));
    let imagens: Vec<(&str, &[u8])> = programas.iter().map(|(nome, elf)| (*nome, elf.as_slice())).collect();
    let relatorio = user::run_images(&imagens, on_end).expect("o pedido de run foi recusado");
    scheduler::set_poll_hook(None);
    relatorio
}

/// Roda um programa só, com as teclas empurradas durante a execução, e exige
/// que ele saia com `0`.
fn sai_bem(nome: &str, elf: &Vec<u8>, durante: &[u8]) {
    let relatorio = rodar(&[(nome, elf)], &[], durante, None);
    assert_eq!(relatorio.finished.len(), 1);
    assert_eq!(relatorio.finished[0].termination, Termination::Exit { code: 0 }, "programa {}", nome);
}

/// Programa que lê uma tecla para cada valor de `esperados` e exige que o
/// código recebido seja esse; sai com `10 + n` se a leitura `n` veio errada.
fn elf_le_teclas(esperados: &[u64]) -> Vec<u8> {
    let mut a = Asm::new();
    for (n, &esperado) in esperados.iter().enumerate() {
        a.chama(abi::SYS_READ_KEY, 0, 0, 0);
        a.resultado_igual(esperado, 10 + n as u32);
    }
    a.exit(0);
    a.into_elf()
}

const CTRL: u64 = abi::KEY_CTRL as u64;

fn ch(c: char) -> u64 {
    c as u64
}

// ---------------------------------------------------------------------------
// US2: SYS_READ_KEY
// ---------------------------------------------------------------------------

#[test_case]
fn soltar_nao_gera_evento_e_nada_aparece_na_tela() {
    restaurar_tela();
    // `a` pressionado e solto, depois `b`: se o solto gerasse evento, a
    // segunda leitura seria 0x9E em vez de `b`.
    sai_bem("le", &elf_le_teclas(&[ch('a'), ch('b')]), &[0x1E, 0x9E, 0x30, 0xB0]);
    assert_eq!(linhas_da_tela(), Vec::<String>::new(), "SYS_READ_KEY nao ecoa");
    reset_teclado();
}

#[test_case]
fn shift_da_maiuscula_e_shift_sozinho_nao_gera_evento() {
    restaurar_tela();
    sai_bem("le", &elf_le_teclas(&[ch('A'), ch('a')]), &[0x2A, 0x1E, 0x9E, 0xAA, 0x1E]);
    reset_teclado();
}

#[test_case]
fn as_dez_teclas_especiais_chegam_com_o_codigo_da_tabela() {
    restaurar_tela();
    let tabela: [(u8, u32); 10] = [
        (0x48, abi::KEY_UP),
        (0x50, abi::KEY_DOWN),
        (0x4B, abi::KEY_LEFT),
        (0x4D, abi::KEY_RIGHT),
        (0x47, abi::KEY_HOME),
        (0x4F, abi::KEY_END),
        (0x49, abi::KEY_PAGE_UP),
        (0x51, abi::KEY_PAGE_DOWN),
        (0x53, abi::KEY_DELETE),
        (0x52, abi::KEY_INSERT),
    ];
    let mut scancodes = Vec::new();
    let mut esperados = Vec::new();
    for (make, codigo) in tabela {
        scancodes.extend_from_slice(&[0xE0, make, 0xE0, make | 0x80]);
        esperados.push(codigo as u64);
    }
    sai_bem("le", &elf_le_teclas(&esperados), &scancodes);
    reset_teclado();
}

#[test_case]
fn ctrl_soma_o_bit_a_letra_vem_minuscula_e_solto_volta_ao_normal() {
    restaurar_tela();
    let esperados = [
        CTRL | ch('q'), // Ctrl + q
        CTRL | ch('q'), // Ctrl + Shift + q: continua minuscula
        ch('q'),        // Ctrl solto
        CTRL | ch('q'), // Ctrl direito
        ch('q'),        // Ctrl direito solto
        CTRL | abi::KEY_LEFT as u64,
    ];
    let scancodes = [
        0x1D, 0x10, 0x90, // Ctrl esquerdo, q
        0x2A, 0x10, 0x90, 0xAA, // Shift + q (o Ctrl continua apertado)
        0x9D, 0x10, 0x90, // Ctrl solto, q
        0xE0, 0x1D, 0x10, 0x90, 0xE0, 0x9D, // Ctrl direito, q, solto
        0x10, 0x90, // q
        0xE0, 0x1D, 0xE0, 0x4B, 0xE0, 0xCB, // Ctrl direito + seta esquerda
    ];
    sai_bem("le", &elf_le_teclas(&esperados), &scancodes);
    reset_teclado();
}

#[test_case]
fn enter_backspace_tab_esc_e_as_duas_teclas_do_teclado_numerico() {
    restaurar_tela();
    let esperados = [0x0A, 0x08, 0x09, 0x1B, 0x0A, 0x2F];
    let scancodes = [0x1C, 0x0E, 0x0F, 0x01, 0xE0, 0x1C, 0xE0, 0x35];
    sai_bem("le", &elf_le_teclas(&esperados), &scancodes);
    reset_teclado();
}

#[test_case]
fn falso_shift_nao_liga_o_shift_e_nao_gera_evento() {
    restaurar_tela();
    let scancodes = [0xE0, 0x2A, 0xE0, 0x48, 0xE0, 0xC8, 0xE0, 0xAA, 0x1E];
    sai_bem("le", &elf_le_teclas(&[abi::KEY_UP as u64, ch('a')]), &scancodes);
    reset_teclado();
}

#[test_case]
fn pause_printscreen_f1_alt_capslock_e_windows_nao_geram_evento() {
    restaurar_tela();
    let scancodes = [
        0xE1, 0x1D, 0x45, 0xE1, 0x9D, 0xC5, // Pause
        0xE0, 0x2A, 0xE0, 0x37, 0xE0, 0xB7, 0xE0, 0xAA, // PrintScreen
        0x3B, 0xBB, 0x58, 0xD8, // F1, F12
        0x38, 0xB8, 0x3A, 0xBA, // Alt, Caps Lock
        0xE0, 0x5B, 0xE0, 0xDB, // Windows
        0x1E, // a
    ];
    sai_bem("le", &elf_le_teclas(&[ch('a')]), &scancodes);
    reset_teclado();
}

#[test_case]
fn digitacao_antecipada_chega_na_ordem_em_que_foi_digitada() {
    restaurar_tela();
    let elf = elf_le_teclas(&[ch('a'), ch('b'), ch('c')]);
    let relatorio = rodar(&[("le", &elf)], &[0x1E, 0x30, 0x2E], &[], None);
    assert_eq!(relatorio.finished[0].termination, Termination::Exit { code: 0 });
    reset_teclado();
}

/// Programa que só computa um laço e sai.
fn elf_laco(iteracoes: u32) -> Vec<u8> {
    let mut a = Asm::new();
    a.mov32(Rcx, iteracoes);
    // laco: dec ecx; jnz laco
    a.0.extend_from_slice(&[0xFF, 0xC9, 0x75, 0xFC]);
    a.exit(0);
    a.into_elf()
}

/// O que `on_end` registra: quando o programa de computação terminou.
static CALCULO_TERMINOU: AtomicBool = AtomicBool::new(false);

fn marca_calculo(finished: &Finished) {
    if &*finished.name == "calculo" {
        CALCULO_TERMINOU.store(true, Ordering::SeqCst);
        // Só agora a tecla pode chegar.
        LIBERA.store(true, Ordering::SeqCst);
    }
}

#[test_case]
fn quem_espera_tecla_nao_atrasa_quem_computa() {
    restaurar_tela();
    CALCULO_TERMINOU.store(false, Ordering::SeqCst);
    let leitor = elf_le_teclas(&[ch('a')]);
    let calculo = elf_laco(50_000_000);
    reset_teclado();
    {
        let mut roteiro = ROTEIRO.lock();
        roteiro.scancodes = alloc::vec![0x1E];
        roteiro.proximo = 0;
    }
    // A tecla só é liberada quando o cálculo termina.
    LIBERA.store(false, Ordering::SeqCst);
    scheduler::set_poll_hook(Some(tick));
    let imagens: [(&str, &[u8]); 2] = [("leitor", &leitor), ("calculo", &calculo)];
    let relatorio = user::run_images(&imagens, Some(marca_calculo)).expect("run recusado");
    scheduler::set_poll_hook(None);
    LIBERA.store(true, Ordering::SeqCst);
    assert!(CALCULO_TERMINOU.load(Ordering::SeqCst));
    // O cálculo terminou primeiro: o leitor, esperando, não o atrasou.
    assert_eq!(&*relatorio.finished[0].name, "calculo");
    assert_eq!(&*relatorio.finished[1].name, "leitor");
    assert_eq!(relatorio.finished[1].termination, Termination::Exit { code: 0 });
    reset_teclado();
}

/// Conta os polls do teclado e só libera a tecla depois de `ESPERA` deles.
static POLLS: Mutex<u32> = Mutex::new(0);

fn tick_atrasado() {
    let mut polls = POLLS.lock();
    *polls += 1;
    if *polls == 6 {
        interrupts::push_scancode(0x1E);
    }
}

#[test_case]
fn todos_esperando_tecla_faz_o_kernel_dormir_em_vez_de_girar() {
    restaurar_tela();
    reset_teclado();
    *POLLS.lock() = 0;
    scheduler::set_poll_hook(Some(tick_atrasado));
    let elf = elf_le_teclas(&[ch('a')]);
    let relatorio = user::run_images(&[("le", &elf)], None).expect("run recusado");
    scheduler::set_poll_hook(None);
    assert_eq!(relatorio.finished[0].termination, Termination::Exit { code: 0 });
    // Ninguém estava pronto e alguém esperava o teclado: o kernel dormiu
    // (`sti; hlt`) algumas vezes até a tecla chegar, sem laço ocupado.
    assert!(relatorio.idle_loops >= 1, "o kernel nao dormiu: {}", relatorio.idle_loops);
    reset_teclado();
}

/// Programa que lê uma linha (`SYS_READ_LINE`) e sai com o tamanho recebido.
fn elf_le_linha() -> Vec<u8> {
    let mut a = Asm::new();
    a.aloca_dados();
    // O buffer é a página de heap, cujo endereço `aloca_dados` deixou em `r15`.
    a.0.extend_from_slice(&[0x4C, 0x89, 0xFF]); // mov rdi, r15
    a.mov32(Rsi, 16);
    a.mov32(Rax, abi::SYS_READ_LINE as u32);
    a.syscall();
    a.0.extend_from_slice(&[0x48, 0x89, 0xC7]); // mov rdi, rax
    a.mov32(Rax, abi::SYS_EXIT as u32);
    a.syscall();
    a.into_elf()
}

#[test_case]
fn linha_e_tecla_esperando_juntas_as_teclas_vao_a_quem_pediu_primeiro() {
    restaurar_tela();
    let linha_elf = elf_le_linha();
    let tecla_elf = elf_le_teclas(&[ch('x')]);
    // `h`, `i`, Enter completam a linha de quem pediu primeiro; o `x` que vem
    // depois é do segundo.
    let relatorio = rodar(&[("linha", &linha_elf), ("tecla", &tecla_elf)], &[], &[0x23, 0x17, 0x1C, 0x2D], None);
    let linha = relatorio.finished.iter().find(|f| &*f.name == "linha").unwrap();
    let tecla = relatorio.finished.iter().find(|f| &*f.name == "tecla").unwrap();
    assert_eq!(linha.termination, Termination::Exit { code: 3 }, "hi + Enter = 3 bytes");
    assert_eq!(tecla.termination, Termination::Exit { code: 0 });
    restaurar_tela();
    reset_teclado();
}

#[test_case]
fn o_que_ninguem_pediu_continua_na_fila_e_o_prompt_recebe() {
    restaurar_tela();
    let elf = elf_le_teclas(&[ch('a')]);
    // O programa lê só o `a`; o `b` e o `c` ficam na fila.
    let relatorio = rodar(&[("le", &elf)], &[0x1E, 0x30, 0x2E], &[], None);
    assert_eq!(relatorio.finished[0].termination, Termination::Exit { code: 0 });
    vga_buffer::clear_screen();
    shell::poll_keyboard();
    assert!(vga_buffer::screen_contains("bc"), "o prompt nao recebeu o que sobrou: {:?}", linhas_da_tela());
    // Limpa a linha do prompt.
    shell::feed(0x08);
    shell::feed(0x08);
    restaurar_tela();
    reset_teclado();
}

#[test_case]
fn ctrl_apertado_ou_solto_pelo_prompt_vale_para_o_programa_que_vem_depois() {
    restaurar_tela();
    // Ctrl apertado e **solto** pelo prompt: o programa recebe `q` sem o bit.
    reset_teclado();
    empurrar(&[0x1D]);
    shell::poll_keyboard();
    empurrar(&[0x9D]);
    shell::poll_keyboard();
    let elf = elf_le_teclas(&[ch('q')]);
    let relatorio = rodar_sem_reset(&[("le", &elf)], &[0x10]);
    assert_eq!(relatorio.finished[0].termination, Termination::Exit { code: 0 });
    // Ctrl apertado pelo prompt e ainda apertado: o programa o vê.
    reset_teclado();
    empurrar(&[0x1D]);
    shell::poll_keyboard();
    let elf = elf_le_teclas(&[CTRL | ch('q')]);
    let relatorio = rodar_sem_reset(&[("le", &elf)], &[0x10]);
    assert_eq!(relatorio.finished[0].termination, Termination::Exit { code: 0 });
    reset_teclado();
    restaurar_tela();
}

/// Como `rodar`, sem esvaziar a fila nem o decodificador antes.
fn rodar_sem_reset(programas: &[(&str, &Vec<u8>)], durante: &[u8]) -> RunReport {
    {
        let mut roteiro = ROTEIRO.lock();
        roteiro.scancodes = durante.to_vec();
        roteiro.proximo = 0;
    }
    LIBERA.store(true, Ordering::SeqCst);
    scheduler::set_poll_hook(Some(tick));
    let imagens: Vec<(&str, &[u8])> = programas.iter().map(|(nome, elf)| (*nome, elf.as_slice())).collect();
    let relatorio = user::run_images(&imagens, None).expect("o pedido de run foi recusado");
    scheduler::set_poll_hook(None);
    relatorio
}

#[test_case]
fn o_prompt_ignora_tab_esc_e_as_teclas_estendidas_como_sempre() {
    restaurar_tela();
    reset_teclado();
    vga_buffer::clear_screen();
    empurrar(&[0x0F, 0x01, 0xE0, 0x48, 0xE0, 0xC8, 0xE0, 0x4B, 0xE0, 0xCB, 0x1E]);
    shell::poll_keyboard();
    // Só o `a` foi aceito pelo prompt.
    assert!(vga_buffer::screen_contains("a"));
    shell::feed(0x08);
    restaurar_tela();
    reset_teclado();
}

// ---------------------------------------------------------------------------
// US3: SYS_SCREEN (limpar, mover, escrever a partir do cursor)
// ---------------------------------------------------------------------------

/// Monta um programa: `corpo` gera o código e o programa termina com `exit(0)`.
fn programa(corpo: impl FnOnce(&mut Asm)) -> Vec<u8> {
    let mut a = Asm::new();
    corpo(&mut a);
    a.exit(0);
    a.into_elf()
}

const OK: u64 = 0;
const INVAL: u64 = abi::ERR_INVAL as u64;

impl Asm {
    /// `SYS_SCREEN(op, a, b)`, exigindo o resultado `esperado`.
    fn tela(&mut self, op: u64, a: u64, b: u64, esperado: u64, code: u32) {
        self.chama(abi::SYS_SCREEN, op, a, b);
        self.resultado_igual(esperado, code);
    }
}

/// O caractere guardado na célula `(linha, coluna)`.
fn celula(linha: usize, coluna: usize) -> u8 {
    vga_buffer::screen_row_bytes(linha)[coluna]
}

/// Roda `elf` sozinho, por `run_images` (sem o fim de execução do prompt), e
/// exige `exit(0)`.
fn roda_e_sai_bem(elf: &Vec<u8>) {
    let relatorio = rodar(&[("tela", elf)], &[], &[], None);
    assert_eq!(relatorio.finished[0].termination, Termination::Exit { code: 0 });
}

#[test_case]
fn limpar_deixa_25_linhas_em_branco_e_o_proximo_write_comeca_em_0_0() {
    restaurar_tela();
    os_rust::print!("lixo no pe da tela");
    let elf = programa(|a| {
        a.tela(abi::SCREEN_CLEAR, 0, 0, OK, 10);
        a.escreve(b"Z");
    });
    roda_e_sai_bem(&elf);
    assert_eq!(celula(0, 0), b'Z');
    assert_eq!(linha(0), "Z");
    for row in 1..25 {
        assert_eq!(linha(row), "", "linha {} deveria estar em branco", row);
    }
    assert_eq!(vga_buffer::screen_cursor(), (0, 1));
    restaurar_tela();
}

#[test_case]
fn mover_leva_o_cursor_na_hora_e_o_write_comeca_nele() {
    restaurar_tela();
    // Só mover: o cursor vai para lá sem que nada seja escrito.
    let so_mover = programa(|a| {
        a.tela(abi::SCREEN_CLEAR, 0, 0, OK, 10);
        a.tela(abi::SCREEN_MOVE, 5, 10, OK, 11);
    });
    roda_e_sai_bem(&so_mover);
    assert_eq!(vga_buffer::screen_cursor(), (5, 10));
    assert_eq!(celula(5, 10), b' ');
    // Mover e escrever.
    let move_e_escreve = programa(|a| {
        a.tela(abi::SCREEN_CLEAR, 0, 0, OK, 10);
        a.tela(abi::SCREEN_MOVE, 5, 10, OK, 11);
        a.escreve(b"@");
    });
    roda_e_sai_bem(&move_e_escreve);
    assert_eq!(celula(5, 10), b'@');
    assert_eq!(vga_buffer::screen_cursor(), (5, 11));
    restaurar_tela();
}

#[test_case]
fn posicao_ou_operacao_fora_dos_limites_e_inval_e_o_cursor_fica_onde_estava() {
    restaurar_tela();
    let elf = programa(|a| {
        a.tela(abi::SCREEN_CLEAR, 0, 0, OK, 10);
        a.tela(abi::SCREEN_MOVE, 5, 10, OK, 11);
        a.tela(abi::SCREEN_MOVE, 25, 0, INVAL, 12);
        a.tela(abi::SCREEN_MOVE, 0, 80, INVAL, 13);
        a.tela(abi::SCREEN_MOVE, u64::MAX, 0, INVAL, 14);
        a.tela(abi::SCREEN_MOVE, 0, u64::MAX, INVAL, 15);
        a.tela(0, 0, 0, INVAL, 16);
        a.tela(4, 0, 0, INVAL, 17);
        a.tela(u64::MAX, 0, 0, INVAL, 18);
        // Nada mudou: o `@` cai onde o cursor já estava.
        a.escreve(b"@");
    });
    roda_e_sai_bem(&elf);
    assert_eq!(celula(5, 10), b'@');
    restaurar_tela();
}

#[test_case]
fn quebra_de_linha_acima_da_ultima_so_desce_o_cursor_sem_rolar() {
    restaurar_tela();
    let elf = programa(|a| {
        a.tela(abi::SCREEN_CLEAR, 0, 0, OK, 10);
        a.tela(abi::SCREEN_MOVE, 5, 10, OK, 11);
        a.escreve(b"ab\ncd");
    });
    roda_e_sai_bem(&elf);
    assert_eq!((celula(5, 10), celula(5, 11)), (b'a', b'b'));
    assert_eq!((celula(6, 0), celula(6, 1)), (b'c', b'd'));
    assert_eq!(linha(0), "", "nada rolou");
    restaurar_tela();
}

#[test_case]
fn passar_da_coluna_79_vai_para_a_linha_seguinte_e_na_ultima_linha_rola() {
    restaurar_tela();
    let acima = programa(|a| {
        a.tela(abi::SCREEN_CLEAR, 0, 0, OK, 10);
        a.tela(abi::SCREEN_MOVE, 3, 79, OK, 11);
        a.escreve(b"xy");
    });
    roda_e_sai_bem(&acima);
    assert_eq!(celula(3, 79), b'x');
    assert_eq!(celula(4, 0), b'y');
    assert_eq!(linha(0), "");
    let na_ultima = programa(|a| {
        a.tela(abi::SCREEN_CLEAR, 0, 0, OK, 10);
        a.tela(abi::SCREEN_MOVE, 24, 79, OK, 11);
        a.escreve(b"xy");
    });
    roda_e_sai_bem(&na_ultima);
    assert_eq!(celula(23, 79), b'x', "o x subiu com a rolagem");
    assert_eq!(celula(24, 0), b'y');
    restaurar_tela();
}

#[test_case]
fn oitenta_caracteres_a_partir_de_24_0_nao_rolam() {
    restaurar_tela();
    let elf = programa(|a| {
        a.tela(abi::SCREEN_CLEAR, 0, 0, OK, 10);
        a.tela(abi::SCREEN_MOVE, 24, 0, OK, 11);
        a.escreve(&[b'z'; 80]);
    });
    roda_e_sai_bem(&elf);
    assert_eq!(celula(24, 0), b'z');
    assert_eq!(celula(24, 79), b'z');
    assert_eq!(linha(23), "", "a tela nao rolou");
    assert_eq!(vga_buffer::screen_cursor(), (24, 79), "o cursor fica na coluna 79");
    restaurar_tela();
}

#[test_case]
fn sem_syscall_de_tela_a_tela_e_a_de_sempre() {
    restaurar_tela();
    let imagem = user::builtin_image("hello").expect("hello embutido");
    let relatorio = user::run_images(&[("hello", imagem)], None).expect("run");
    assert_eq!(relatorio.finished[0].termination, Termination::Exit { code: 0 });
    assert!(vga_buffer::screen_contains("Ola do ring 3!"));
    // O cursor continua na última linha e tudo em amarelo sobre preto.
    assert_eq!(vga_buffer::screen_cursor().0, 24);
    for row in 22..25 {
        assert_eq!(vga_buffer::screen_cell_color(row, 0), 0x0E);
    }
    restaurar_tela();
}

/// Os três jeitos de um programa terminar depois de ter usado a tela.
#[derive(Clone, Copy)]
enum Fim {
    Exit,
    FalhaDeCpu,
    FalhaDeMemoria,
}

/// Um programa que desenha (`topo` na linha 0 e `rodape` na linha 24, com a
/// cor dada) e termina como `fim`. Deixa o cursor na linha 1, para a mensagem
/// de falha, se houver, não rolar a tela.
fn elf_desenha(cor: Option<(u64, u64)>, fim: Fim) -> Vec<u8> {
    programa(|a| {
        a.tela(abi::SCREEN_CLEAR, 0, 0, OK, 10);
        if let Some((texto, fundo)) = cor {
            a.tela(abi::SCREEN_COLOR, texto, fundo, OK, 11);
        }
        a.tela(abi::SCREEN_MOVE, 0, 0, OK, 12);
        a.escreve(b"topo");
        a.tela(abi::SCREEN_MOVE, 24, 0, OK, 13);
        a.escreve(b"rodape");
        a.tela(abi::SCREEN_MOVE, 1, 0, OK, 14);
        match fim {
            Fim::Exit => {}
            Fim::FalhaDeCpu => a.ud2(),
            Fim::FalhaDeMemoria => a.escreve_no_nada(),
        }
    })
}

/// Roda `elf` pelo prompt (`run /ram/p`), que é quem restaura a tela.
fn roda_pelo_prompt(elf: &Vec<u8>) -> Vec<String> {
    instalar_elf("/ram/p", elf);
    let linhas = rodar_comando("run /ram/p");
    let _ = os_rust::fs::remove(b"/ram/p");
    linhas
}

#[test_case]
fn a_volta_ao_prompt_apaga_so_a_linha_24_depois_de_exit_de_falha_de_cpu_e_de_falha_de_memoria() {
    for fim in [Fim::Exit, Fim::FalhaDeCpu, Fim::FalhaDeMemoria] {
        restaurar_tela();
        reset_teclado();
        let _ = roda_pelo_prompt(&elf_desenha(None, fim));
        assert!(linha(0).starts_with("topo"), "as linhas de cima ficam: {:?}", linha(0));
        assert!(!linha(24).contains("rodape"), "a linha 24 foi apagada: {:?}", linha(24));
        assert!(linha(24).starts_with("os-rust>"), "o prompt voltou na linha 24: {:?}", linha(24));
        assert_eq!(vga_buffer::screen_cursor(), (24, 9));
        // O prompt responde.
        exige(&rodar_comando("echo vivo"), "vivo");
    }
    restaurar_tela();
}

#[test_case]
fn o_comando_clear_do_prompt_faz_o_que_fazia() {
    restaurar_tela();
    os_rust::print!("uma sujeira qualquer");
    let linhas = rodar_comando("clear");
    // A tela em branco e o prompt no pé.
    assert_eq!(linhas.len(), 1, "{:?}", linhas);
    assert!(linhas[0].starts_with("os-rust>"));
    restaurar_tela();
}

// ---------------------------------------------------------------------------
// US5: cores e vídeo reverso
// ---------------------------------------------------------------------------

#[test_case]
fn as_16_cores_de_texto_com_as_8_de_fundo_viram_o_byte_certo() {
    restaurar_tela();
    let elf = programa(|a| {
        a.tela(abi::SCREEN_CLEAR, 0, 0, OK, 10);
        for fundo in 0..8u64 {
            for texto in 0..16u64 {
                a.tela(abi::SCREEN_COLOR, texto, fundo, OK, 11);
                a.tela(abi::SCREEN_MOVE, fundo, texto, OK, 12);
                a.escreve(b"x");
            }
        }
    });
    roda_e_sai_bem(&elf);
    for fundo in 0..8usize {
        for texto in 0..16usize {
            assert_eq!(celula(fundo, texto), b'x');
            assert_eq!(vga_buffer::screen_cell_color(fundo, texto), ((fundo << 4) | texto) as u8);
        }
    }
    // Os exemplos do contrato.
    assert_eq!(vga_buffer::screen_cell_color(0, 14), 0x0E);
    assert_eq!(vga_buffer::screen_cell_color(7, 15), 0x7F);
    assert_eq!(vga_buffer::screen_cell_color(7, 0), 0x70);
    restaurar_tela();
}

#[test_case]
fn cor_fora_dos_limites_e_inval_e_a_cor_atual_nao_muda() {
    restaurar_tela();
    let elf = programa(|a| {
        a.tela(abi::SCREEN_CLEAR, 0, 0, OK, 10);
        a.tela(abi::SCREEN_COLOR, 16, 0, INVAL, 11);
        a.tela(abi::SCREEN_COLOR, 0, 8, INVAL, 12);
        a.tela(abi::SCREEN_COLOR, u64::MAX, 0, INVAL, 13);
        a.tela(abi::SCREEN_COLOR, 0, u64::MAX, INVAL, 14);
        a.tela(abi::SCREEN_MOVE, 2, 2, OK, 15);
        a.escreve(b"x");
    });
    roda_e_sai_bem(&elf);
    assert_eq!(vga_buffer::screen_cell_color(2, 2), 0x0E, "a cor atual continua amarelo sobre preto");
    restaurar_tela();
}

#[test_case]
fn a_cor_nao_muda_o_que_ja_estava_e_mover_nao_muda_a_cor() {
    restaurar_tela();
    let elf = programa(|a| {
        a.tela(abi::SCREEN_CLEAR, 0, 0, OK, 10);
        a.tela(abi::SCREEN_MOVE, 1, 0, OK, 11);
        a.escreve(b"p");
        a.tela(abi::SCREEN_COLOR, 15, 7, OK, 12);
        a.tela(abi::SCREEN_MOVE, 2, 0, OK, 13);
        a.escreve(b"m");
    });
    roda_e_sai_bem(&elf);
    assert_eq!(vga_buffer::screen_cell_color(1, 0), 0x0E, "o que ja estava nao mudou de cor");
    assert_eq!(vga_buffer::screen_cell_color(2, 0), 0x7F, "mover nao mudou a cor");
    restaurar_tela();
}

#[test_case]
fn limpar_pinta_as_25_linhas_com_a_cor_atual_e_nao_a_muda() {
    restaurar_tela();
    let elf = programa(|a| {
        a.tela(abi::SCREEN_COLOR, 15, 7, OK, 10);
        a.tela(abi::SCREEN_CLEAR, 0, 0, OK, 11);
        a.escreve(b"c");
    });
    roda_e_sai_bem(&elf);
    assert_eq!(vga_buffer::screen_cell_color(0, 0), 0x7F);
    assert_eq!(vga_buffer::screen_cell_color(12, 40), 0x7F);
    assert_eq!(vga_buffer::screen_cell_color(24, 79), 0x7F);
    restaurar_tela();
}

#[test_case]
fn a_rolagem_da_ultima_linha_abre_a_linha_com_a_cor_atual() {
    restaurar_tela();
    let elf = programa(|a| {
        a.tela(abi::SCREEN_COLOR, 0, 7, OK, 10);
        a.tela(abi::SCREEN_MOVE, 24, 0, OK, 11);
        a.escreve(b"\n");
    });
    roda_e_sai_bem(&elf);
    assert_eq!(vga_buffer::screen_cell_color(24, 5), 0x70);
    restaurar_tela();
}

#[test_case]
fn a_volta_ao_prompt_restaura_a_cor_padrao_e_as_linhas_de_cima_ficam_coloridas() {
    for fim in [Fim::Exit, Fim::FalhaDeCpu, Fim::FalhaDeMemoria] {
        restaurar_tela();
        reset_teclado();
        let _ = roda_pelo_prompt(&elf_desenha(Some((0, 7)), fim));
        // As linhas de cima mantêm a cor que o programa deixou...
        assert_eq!(vga_buffer::screen_cell_color(0, 0), 0x70);
        // ... a linha 24 foi apagada e escrita de novo na cor padrão...
        for col in [0usize, 5, 20, 79] {
            assert_eq!(vga_buffer::screen_cell_color(24, col), 0x0E, "coluna {}", col);
        }
        // ... e o prompt, o ls e os outros comandos escrevem em amarelo sobre preto.
        let _ = rodar_comando("echo cor");
        assert_eq!(vga_buffer::screen_cell_color(24, 0), 0x0E);
    }
    restaurar_tela();
}

#[test_case]
fn a_volta_ao_prompt_restaura_a_cor_tambem_quando_so_se_chamou_screen_color() {
    restaurar_tela();
    reset_teclado();
    let elf = programa(|a| {
        a.tela(abi::SCREEN_COLOR, 0, 7, OK, 10);
        a.escreve(b"zzz");
    });
    let _ = roda_pelo_prompt(&elf);
    assert!(!linha(24).contains("zzz"));
    for col in [0usize, 5, 79] {
        assert_eq!(vga_buffer::screen_cell_color(24, col), 0x0E, "coluna {}", col);
    }
    restaurar_tela();
}

#[test_case]
fn sem_syscall_de_tela_tudo_continua_amarelo_sobre_preto() {
    restaurar_tela();
    let linhas = rodar_comando("run hello");
    exige(&linhas, "Ola do ring 3!");
    for row in 0..25 {
        for col in [0usize, 10, 40] {
            assert_eq!(vga_buffer::screen_cell_color(row, col), 0x0E);
        }
    }
    restaurar_tela();
}

// ---------------------------------------------------------------------------
// US6: SYS_ARGS e o comando exec
// ---------------------------------------------------------------------------

/// Programa que pede uma página de heap, chama `SYS_ARGS(heap, buf_len)` e
/// exige `total` como resultado e `primeiros` (os 8 primeiros bytes do buffer,
/// little-endian) como o que foi copiado. Sai com `10` ou `11` se falhar.
fn elf_args(buf_len: u64, total: u64, primeiros: u64) -> Vec<u8> {
    programa(|a| {
        a.aloca_dados();
        a.0.extend_from_slice(&[0x4C, 0x89, 0xFF]); // mov rdi, r15
        a.set(Rsi, buf_len);
        a.mov32(Rax, abi::SYS_ARGS as u32);
        a.syscall();
        a.resultado_igual(total, 10);
        a.load(Rax, R15);
        a.movabs(Rcx, primeiros);
        a.cmp(Rax, Rcx);
        a.exige_igual_ou_sai(11);
    })
}

/// Os bytes de `texto` (no máximo 8) como um `u64` little-endian.
fn le(texto: &[u8]) -> u64 {
    let mut bytes = [0u8; 8];
    bytes[..texto.len().min(8)].copy_from_slice(&texto[..texto.len().min(8)]);
    u64::from_le_bytes(bytes)
}

/// `exec /ram/p <args>` com `elf` instalado em `/ram/p`; devolve a tela.
fn exec_de(elf: &Vec<u8>, args: &str) -> Vec<String> {
    reset_teclado();
    instalar_elf("/ram/p", elf);
    let linhas = rodar_comando(&alloc::format!("exec /ram/p {}", args));
    let _ = os_rust::fs::remove(b"/ram/p");
    linhas
}

/// A tela não tem a mensagem de término com falha.
fn sem_falha(linhas: &[String]) {
    assert!(!tem(linhas, "terminou com codigo"), "o programa falhou: {:?}", linhas);
}

#[test_case]
fn run_entrega_argumentos_vazios_e_nao_escreve_no_buffer() {
    restaurar_tela();
    // Pelo carregador, sem argumentos: o tamanho é 0 e o buffer fica zerado.
    let elf = elf_args(16, 0, 0);
    roda_e_sai_bem(&elf);
    // Pelo `run` do prompt, com um programa e com vários.
    instalar_elf("/ram/p", &elf);
    sem_falha(&rodar_comando("run /ram/p"));
    sem_falha(&rodar_comando("run /ram/p /ram/p"));
    let _ = os_rust::fs::remove(b"/ram/p");
    restaurar_tela();
}

#[test_case]
fn exec_entrega_o_texto_depois_do_nome_sem_os_espacos_das_pontas() {
    restaurar_tela();
    sem_falha(&exec_de(&elf_args(16, 3, le(b"a b")), "a b"));
    // Espaços do começo e do fim saem; os do meio ficam: "a   b" tem 5 bytes.
    sem_falha(&exec_de(&elf_args(16, 5, le(b"a   b")), "  a   b  "));
    // Sem texto: devolve 0 e não escreve no buffer.
    sem_falha(&exec_de(&elf_args(16, 0, 0), ""));
    // Aspas chegam como estão.
    sem_falha(&exec_de(&elf_args(16, 5, le(b"\"a b\"")), "\"a b\""));
    restaurar_tela();
}

#[test_case]
fn texto_de_116_bytes_pelo_prompt_chega_inteiro() {
    restaurar_tela();
    let texto = "x".repeat(116);
    // `exec /ram/p ` tem 12 bytes: com 116 a linha tem os 128 do limite.
    sem_falha(&exec_de(&elf_args(128, 116, le(b"xxxxxxxx")), &texto));
    restaurar_tela();
}

#[test_case]
fn o_limite_de_128_bytes_chega_inteiro_pelo_carregador() {
    restaurar_tela();
    let elf = elf_args(128, 128, le(b"yyyyyyyy"));
    let texto = "y".repeat(128);
    let relatorio = user::run_images_with_args(&[("p", &elf)], &texto, None).expect("run");
    assert_eq!(relatorio.finished[0].termination, Termination::Exit { code: 0 });
    restaurar_tela();
}

#[test_case]
fn buffer_menor_corta_a_copia_e_devolve_o_tamanho_total() {
    restaurar_tela();
    // 4 bytes de buffer para um texto de 8: copia `abcd` e devolve 8.
    sem_falha(&exec_de(&elf_args(4, 8, le(b"abcd")), "abcdefgh"));
    restaurar_tela();
}

#[test_case]
fn buf_len_zero_devolve_o_tamanho_sem_olhar_o_ponteiro() {
    restaurar_tela();
    let elf = programa(|a| {
        a.chama(abi::SYS_ARGS, 0x1000, 0, 0);
        a.resultado_igual(3, 10);
    });
    sem_falha(&exec_de(&elf, "a b"));
    restaurar_tela();
}

#[test_case]
fn ponteiro_ruim_ou_len_enorme_e_fault_sem_copiar_nada_e_o_kernel_segue() {
    restaurar_tela();
    let fault = abi::ERR_FAULT as u64;
    // Ponteiros que nunca valem: fora da região, não mapeado, não gravável (o
    // código do programa é `R E`).
    for ptr in [0x1000u64, 0xFFFF_FFFF_FFFF_F000, USER_REGION_START - 4, USER_REGION_START + 0x10_0000, USER_REGION_START] {
        let elf = programa(|a| {
            a.chama(abi::SYS_ARGS, ptr, 8, 0);
            a.resultado_igual(fault, 10);
        });
        sem_falha(&exec_de(&elf, "abc"));
    }
    // `buf_len = u64::MAX`, com ponteiro inválido e com ponteiro válido (o
    // heap), e um `buf_len` que passa do fim da região com ponteiro válido:
    // `ERR_FAULT`, sem pânico e sem copiar nada para o buffer.
    for len in [u64::MAX, 0x4000_0000u64, 0x1_0000_0000u64] {
        let elf = programa(|a| {
            a.aloca_dados();
            a.0.extend_from_slice(&[0x4C, 0x89, 0xFF]); // mov rdi, r15
            a.set(Rsi, len);
            a.mov32(Rax, abi::SYS_ARGS as u32);
            a.syscall();
            a.resultado_igual(fault, 10);
            a.load(Rax, R15);
            a.cmp_zero(Rax);
            a.exige_igual_ou_sai(11);
        });
        sem_falha(&exec_de(&elf, "abc"));
    }
    let elf = programa(|a| {
        a.chama(abi::SYS_ARGS, 0x1000, u64::MAX, 0);
        a.resultado_igual(fault, 10);
    });
    sem_falha(&exec_de(&elf, "abc"));
    // O kernel segue vivo.
    exige(&rodar_comando("echo vivo"), "vivo");
    restaurar_tela();
}

#[test_case]
fn dois_exec_seguidos_nao_vazam_o_texto_de_um_para_o_outro() {
    restaurar_tela();
    sem_falha(&exec_de(&elf_args(16, 3, le(b"abc")), "abc"));
    sem_falha(&exec_de(&elf_args(16, 0, 0), ""));
    restaurar_tela();
}

#[test_case]
fn exec_mostra_as_mesmas_mensagens_do_run_trocando_o_nome_do_comando() {
    restaurar_tela();
    reset_teclado();
    exige(&rodar_comando("exec"), "exec: informe o programa.");
    exige(&rodar_comando("exec xpto"), "programa desconhecido: xpto");
    let linhas = rodar_comando("exec /ram/nada.bin a");
    exige(&linhas, "exec: ");
    // Um diretório e um arquivo que não é ELF.
    exige(&rodar_comando("exec /ram/docs"), "exec: ");
    instalar_elf("/ram/lixo", b"isto nao e um ELF");
    exige(&rodar_comando("exec /ram/lixo"), "[run] erro ao carregar lixo");
    let _ = os_rust::fs::remove(b"/ram/lixo");
    // Um programa que termina com falha.
    let falha = elf_sai(7);
    exige(&exec_de(&falha, "x"), "[run] p terminou com codigo 7");
    // O mesmo programa pelo `run` diz o mesmo.
    instalar_elf("/ram/p", &falha);
    exige(&rodar_comando("run /ram/p"), "[run] p terminou com codigo 7");
    let _ = os_rust::fs::remove(b"/ram/p");
    restaurar_tela();
}

// ---------------------------------------------------------------------------
// US1: o programa `teclas`, de ponta a ponta
// ---------------------------------------------------------------------------

/// Um passo do roteiro: os scancodes de uma tecla e o texto que a linha 2 da
/// tela deve mostrar **depois** de o programa processá-la.
struct Passo {
    scancodes: Vec<u8>,
    linha_2: &'static str,
    linha_3: String,
}

/// O roteiro por passos: só avança quando o programa está esperando tecla (já
/// terminou de desenhar), e confere a tela antes de empurrar a seguinte.
struct Passos {
    passos: Vec<Passo>,
    proximo: usize,
    pendente: Option<(&'static str, String)>,
    erros: Vec<String>,
}

static PASSOS: Mutex<Passos> = Mutex::new(Passos { passos: Vec::new(), proximo: 0, pendente: None, erros: Vec::new() });

/// O gancho dos testes de `teclas`: se o programa espera tecla, confere o
/// resultado do passo anterior e empurra os scancodes do próximo.
fn tick_passos() {
    if scheduler::keyboard_waiters() == 0 {
        return;
    }
    let mut roteiro = PASSOS.lock();
    if let Some((linha_2, linha_3)) = roteiro.pendente.take() {
        let (visto_2, visto_3) = (linha(2), linha(3));
        if !visto_2.contains(linha_2) || visto_3 != linha_3 {
            roteiro.erros.push(alloc::format!("esperava {:?}/{:?}, a tela tem {:?}/{:?}", linha_2, linha_3, visto_2, visto_3));
        }
    }
    if roteiro.proximo < roteiro.passos.len() {
        let k = roteiro.proximo;
        for &scancode in roteiro.passos[k].scancodes.clone().iter() {
            interrupts::push_scancode(scancode);
        }
        roteiro.pendente = Some((roteiro.passos[k].linha_2, roteiro.passos[k].linha_3.clone()));
        roteiro.proximo += 1;
    }
}

/// Os scancodes de pressionar e soltar uma tecla estendida.
fn estendida(make: u8) -> Vec<u8> {
    alloc::vec![0xE0, make, 0xE0, make | 0x80]
}

fn passo(scancodes: Vec<u8>, linha_2: &'static str, lidas: u32) -> Passo {
    Passo { scancodes, linha_2, linha_3: alloc::format!("teclas lidas: {}", lidas) }
}

/// O passo final: `Ctrl+Q`, que também conta.
fn passo_ctrl_q(lidas: u32) -> Passo {
    passo(alloc::vec![0x1D, 0x10, 0x90, 0x9D], "Ctrl+Q", lidas)
}

/// Roda `comando` no prompt com os passos de `roteiro`, devolve a tela e os
/// erros que o gancho achou pelo caminho (que o teste deve conferir vazios).
fn roda_com_passos(comando: &str, roteiro: Vec<Passo>) -> (Vec<String>, Vec<String>) {
    reset_teclado();
    {
        let mut r = PASSOS.lock();
        r.passos = roteiro;
        r.proximo = 0;
        r.pendente = None;
        r.erros = Vec::new();
    }
    scheduler::set_poll_hook(Some(tick_passos));
    let tela = rodar_comando(comando);
    scheduler::set_poll_hook(None);
    let erros = PASSOS.lock().erros.clone();
    (tela, erros)
}

fn conteudo_de(path: &[u8]) -> Vec<u8> {
    os_rust::fs::read_whole(path, 1 << 20).expect("le o arquivo")
}

fn tamanho_de(path: &[u8]) -> u32 {
    os_rust::fs::lookup(path).expect("o arquivo existe").1.size
}

/// A sequência da História 1: `a`, seta direita duas vezes, seta abaixo, Enter
/// e `Ctrl+Q` (6 eventos).
fn historia_1() -> Vec<Passo> {
    alloc::vec![
        passo(alloc::vec![0x1E, 0x9E], "caractere 'a'", 1),
        passo(estendida(0x4D), "seta direita", 2),
        passo(estendida(0x4D), "seta direita", 3),
        passo(estendida(0x50), "seta abaixo", 4),
        passo(alloc::vec![0x1C, 0x9C], "Enter", 5),
        passo_ctrl_q(6),
    ]
}

#[test_case]
fn teclas_a_historia_1_de_ponta_a_ponta() {
    restaurar_tela();
    let _ = os_rust::fs::remove(b"/ram/teclas.txt");
    let (_, erros) = roda_com_passos("run teclas", historia_1());
    assert_eq!(erros, Vec::<String>::new(), "a linha 2 e a 3 de cada passo");
    // Barra de título, em vídeo reverso nas 80 colunas.
    assert_eq!(linha(0), "teclas: setas movem o @, Ctrl+Q sai");
    for col in 0..80 {
        assert_eq!(vga_buffer::screen_cell_color(0, col), 0x70, "coluna {}", col);
    }
    // O `@` andou de (12, 40) para (13, 42) e a posição antiga ficou em branco.
    assert_eq!(celula(13, 42), b'@');
    assert_eq!(vga_buffer::screen_cell_color(13, 42), 0x0A);
    assert_eq!(celula(12, 40), b' ');
    // Status e resultado.
    assert_eq!(linha(2), "ultima tecla: Ctrl+Q");
    assert_eq!(linha(3), "teclas lidas: 6");
    assert_eq!(vga_buffer::screen_cell_color(2, 0), 0x0E);
    assert_eq!(vga_buffer::screen_cell_color(3, 0), 0x0E);
    assert_eq!(linha(20), "teclas: 6 teclas lidas");
    assert_eq!(linha(21), "teclas: gravado em /ram/teclas.txt (2 bytes)");
    // O programa não limpou a tela: o prompt voltou limpo no pé.
    assert!(linha(24).starts_with("os-rust>"));
    assert_eq!(conteudo_de(b"/ram/teclas.txt"), b"6\n");
    exige(&rodar_comando("cat /ram/teclas.txt"), "6");
    let _ = os_rust::fs::remove(b"/ram/teclas.txt");
    restaurar_tela();
}

/// Uma execução com `n` letras `a` e depois `Ctrl+Q`.
fn passos_de_letras(n: u32) -> Vec<Passo> {
    let mut passos = Vec::new();
    for k in 1..=n {
        passos.push(passo(alloc::vec![0x1E, 0x9E], "caractere 'a'", k));
    }
    passos.push(passo_ctrl_q(n + 1));
    passos
}

#[test_case]
fn teclas_regrava_nunca_acrescenta_6_1_11_2() {
    restaurar_tela();
    let _ = os_rust::fs::remove(b"/ram/teclas.txt");
    // 6 (a historia), 1 (so Ctrl+Q), 11 (10 teclas + Ctrl+Q), 2.
    let casos: [(Vec<Passo>, &[u8], u32); 4] = [
        (historia_1(), b"6\n", 2),
        (passos_de_letras(0), b"1\n", 2),
        (passos_de_letras(10), b"11\n", 3),
        (passos_de_letras(1), b"2\n", 2),
    ];
    for (passos, esperado, bytes) in casos {
        let (_, erros) = roda_com_passos("run teclas", passos);
        assert_eq!(erros, Vec::<String>::new());
        assert_eq!(conteudo_de(b"/ram/teclas.txt"), esperado);
        assert_eq!(tamanho_de(b"/ram/teclas.txt"), bytes);
    }
    let _ = os_rust::fs::remove(b"/ram/teclas.txt");
    restaurar_tela();
}

#[test_case]
fn teclas_o_arroba_para_nas_bordas_e_o_contador_sobe() {
    restaurar_tela();
    let _ = os_rust::fs::remove(b"/ram/teclas.txt");
    // Cima x8 (sobe 7 e para na linha 5) e esquerda x41 (anda 40 e para na
    // coluna 0): termina em (5, 0) com 49 teclas + Ctrl+Q.
    let mut passos = Vec::new();
    let mut lidas = 0;
    for _ in 0..8 {
        lidas += 1;
        passos.push(passo(estendida(0x48), "seta acima", lidas));
    }
    for _ in 0..41 {
        lidas += 1;
        passos.push(passo(estendida(0x4B), "seta esquerda", lidas));
    }
    passos.push(passo_ctrl_q(lidas + 1));
    let (_, erros) = roda_com_passos("run teclas", passos);
    assert_eq!(erros, Vec::<String>::new());
    assert_eq!(celula(5, 0), b'@');
    assert_eq!(linha(20), "teclas: 50 teclas lidas");
    // Baixo x9 (desce 6 e para na linha 18) e direita x41 (a partir da coluna
    // 40, anda 39 e para na 79): termina em (18, 79).
    let mut passos = Vec::new();
    let mut lidas = 0;
    for _ in 0..9 {
        lidas += 1;
        passos.push(passo(estendida(0x50), "seta abaixo", lidas));
    }
    for _ in 0..41 {
        lidas += 1;
        passos.push(passo(estendida(0x4D), "seta direita", lidas));
    }
    passos.push(passo_ctrl_q(lidas + 1));
    let (_, erros) = roda_com_passos("run teclas", passos);
    assert_eq!(erros, Vec::<String>::new());
    assert_eq!(celula(18, 79), b'@');
    assert_eq!(linha(20), "teclas: 51 teclas lidas");
    let _ = os_rust::fs::remove(b"/ram/teclas.txt");
    restaurar_tela();
}

#[test_case]
fn teclas_mostra_o_nome_das_outras_teclas_e_o_arroba_nao_se_move() {
    restaurar_tela();
    let _ = os_rust::fs::remove(b"/ram/teclas.txt");
    let mut passos = alloc::vec![
        passo(estendida(0x47), "Home", 1),
        passo(estendida(0x4F), "End", 2),
        passo(estendida(0x49), "Page Up", 3),
        passo(estendida(0x51), "Page Down", 4),
        passo(estendida(0x53), "Delete", 5),
        passo(estendida(0x52), "Insert", 6),
        passo(alloc::vec![0x0F, 0x8F], "Tab", 7),
        passo(alloc::vec![0x01, 0x81], "Esc", 8),
        passo(alloc::vec![0x0E, 0x8E], "Backspace", 9),
        // Com Ctrl: `Ctrl+` e a letra maiuscula.
        passo(alloc::vec![0x1D, 0x2D, 0xAD, 0x9D], "Ctrl+X", 10),
    ];
    passos.push(passo_ctrl_q(11));
    let (_, erros) = roda_com_passos("run teclas", passos);
    assert_eq!(erros, Vec::<String>::new());
    assert_eq!(celula(12, 40), b'@', "nenhuma delas move o @");
    let _ = os_rust::fs::remove(b"/ram/teclas.txt");
    restaurar_tela();
}

#[test_case]
fn teclas_com_argumento_grava_nesse_arquivo_e_nao_toca_o_padrao() {
    restaurar_tela();
    let _ = os_rust::fs::remove(b"/ram/contagem.txt");
    let _ = os_rust::fs::remove(b"/ram/teclas.txt");
    {
        // Um /ram/teclas.txt que ja existia: nao pode mudar.
        let mut antigo = os_rust::fs::OpenFile::create(b"/ram/teclas.txt").unwrap();
        antigo.write(b"antigo\n").unwrap();
    }
    let (_, erros) = roda_com_passos("exec teclas /ram/contagem.txt", historia_1());
    assert_eq!(erros, Vec::<String>::new());
    assert_eq!(linha(21), "teclas: gravado em /ram/contagem.txt (2 bytes)");
    assert_eq!(conteudo_de(b"/ram/contagem.txt"), b"6\n");
    assert_eq!(conteudo_de(b"/ram/teclas.txt"), b"antigo\n");
    // `exec teclas` sem argumento usa o padrão.
    let (_, erros) = roda_com_passos("exec teclas", passos_de_letras(0));
    assert_eq!(erros, Vec::<String>::new());
    assert_eq!(conteudo_de(b"/ram/teclas.txt"), b"1\n");
    let _ = os_rust::fs::remove(b"/ram/contagem.txt");
    let _ = os_rust::fs::remove(b"/ram/teclas.txt");
    restaurar_tela();
}

#[test_case]
fn teclas_recusa_mais_de_uma_palavra_e_caminho_longo_sem_mexer_na_tela() {
    restaurar_tela();
    reset_teclado();
    let longo = alloc::format!("/ram/{}", "a".repeat(36));
    assert_eq!(longo.len(), 41);
    for (comando, mensagem) in [
        (String::from("exec teclas a b"), "teclas: uso: teclas [arquivo]"),
        (alloc::format!("exec teclas {}", longo), "teclas: caminho muito longo (maximo 40 bytes)"),
    ] {
        vga_buffer::clear_screen();
        os_rust::print!("marcador");
        digitar_no_prompt(&comando);
        let tela = linhas_da_tela();
        exige(&tela, mensagem);
        exige(&tela, "[run] teclas terminou com codigo 1");
        // Nada de barra de título, nem de cor fora do padrão, nem tela limpa.
        assert!(!tem(&tela, "setas movem"));
        exige(&tela, "marcador");
        for row in 0..25 {
            for col in [0usize, 20, 79] {
                assert_eq!(vga_buffer::screen_cell_color(row, col), 0x0E);
            }
        }
    }
    restaurar_tela();
}

#[test_case]
fn teclas_com_o_ram_cheio_mostra_o_erro_e_sai_com_codigo_1() {
    restaurar_tela();
    os_rust::fs::reset_ramdisk();
    let _ = os_rust::fs::remove(b"/ram/teclas.txt");
    // Enche o /ram até não caber mais nada.
    {
        let mut enche = os_rust::fs::OpenFile::create(b"/ram/enche.bin").unwrap();
        let bloco = [0xEEu8; 4096];
        loop {
            match enche.write(&bloco) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    }
    let (tela, erros) = roda_com_passos("run teclas", passos_de_letras(0));
    assert_eq!(erros, Vec::<String>::new());
    assert_eq!(linha(20), "teclas: 1 teclas lidas");
    assert!(linha(21).starts_with("teclas: nao gravou /ram/teclas.txt: "), "{:?}", linha(21));
    exige(&tela, "[run] teclas terminou com codigo 1");
    exige(&rodar_comando("echo vivo"), "vivo");
    os_rust::fs::reset_ramdisk();
    restaurar_tela();
}

#[test_case]
fn teclas_aparece_uma_vez_e_em_ordem_alfabetica_na_lista_do_run() {
    let nomes: Vec<&str> = user::program_names().collect();
    assert_eq!(nomes.iter().filter(|n| **n == "teclas").count(), 1);
    let mut ordenados = nomes.clone();
    ordenados.sort();
    assert_eq!(nomes, ordenados);
}

// ---------------------------------------------------------------------------
// US7: o contrato, o guia e o README
// ---------------------------------------------------------------------------

const CONTRATO: &str = include_str!("../SYSCALLS.md");
const CONTRATO_EN: &str = include_str!("../SYSCALLS.en.md");

#[test_case]
fn o_contrato_concorda_com_as_constantes_do_marco_13() {
    use abi::*;
    assert!(CONTRATO.contains("**Versão do contrato**: 9"));
    assert!(CONTRATO.contains("os-rust 0.13.0 (Marco 13)"));
    assert!(CONTRATO.contains("fora de 1 a 27"));
    assert!(CONTRATO_EN.contains("**Contract version**: 9"));
    assert!(CONTRATO_EN.contains("os-rust 0.13.0"));
    assert!(CONTRATO_EN.contains("outside 1 to 27"));
    for (valor, nome, esperado) in [
        (SYS_READ_KEY, "SYS_READ_KEY", 24),
        (SYS_SCREEN, "SYS_SCREEN", 25),
        (SYS_OPEN_WRITE, "SYS_OPEN_WRITE", 26),
        (SYS_ARGS, "SYS_ARGS", 27),
    ] {
        assert_eq!(valor, esperado, "{}", nome);
        let linha = alloc::format!("| {} | `{}` |", valor, nome);
        assert!(CONTRATO.contains(&linha) && CONTRATO_EN.contains(&linha), "{}", nome);
        let titulo = alloc::format!("### `{}` ({})", nome, valor);
        assert!(CONTRATO.contains(&titulo) && CONTRATO_EN.contains(&titulo), "{}", nome);
    }
    // O último erro continua o mesmo: o marco não cria erro novo.
    assert_eq!(ERR_NOBUFS, -21);
    for (nome, valor) in [
        ("KEY_UP", KEY_UP),
        ("KEY_DOWN", KEY_DOWN),
        ("KEY_LEFT", KEY_LEFT),
        ("KEY_RIGHT", KEY_RIGHT),
        ("KEY_HOME", KEY_HOME),
        ("KEY_END", KEY_END),
        ("KEY_PAGE_UP", KEY_PAGE_UP),
        ("KEY_PAGE_DOWN", KEY_PAGE_DOWN),
        ("KEY_DELETE", KEY_DELETE),
        ("KEY_INSERT", KEY_INSERT),
    ] {
        let trecho = alloc::format!("`{}` `0x{:X}`", nome, valor);
        assert!(CONTRATO.contains(&trecho) && CONTRATO_EN.contains(&trecho), "{}", nome);
    }
    assert_eq!(KEY_CTRL, 0x1_0000);
    for texto in [CONTRATO, CONTRATO_EN] {
        assert!(texto.contains("`KEY_CTRL`") && texto.contains("`0x1_0000`"));
    }
    for (nome, valor) in [("SCREEN_CLEAR", SCREEN_CLEAR), ("SCREEN_MOVE", SCREEN_MOVE), ("SCREEN_COLOR", SCREEN_COLOR)] {
        let linha = alloc::format!("| `{}` | {} |", nome, valor);
        assert!(CONTRATO.contains(&linha) && CONTRATO_EN.contains(&linha), "{}", nome);
    }
    assert_eq!((SCREEN_ROWS, SCREEN_COLS), (25, 80));
    let cores: [(&str, u8); 16] = [
        ("COLOR_BLACK", COLOR_BLACK),
        ("COLOR_BLUE", COLOR_BLUE),
        ("COLOR_GREEN", COLOR_GREEN),
        ("COLOR_CYAN", COLOR_CYAN),
        ("COLOR_RED", COLOR_RED),
        ("COLOR_MAGENTA", COLOR_MAGENTA),
        ("COLOR_BROWN", COLOR_BROWN),
        ("COLOR_LIGHT_GRAY", COLOR_LIGHT_GRAY),
        ("COLOR_DARK_GRAY", COLOR_DARK_GRAY),
        ("COLOR_LIGHT_BLUE", COLOR_LIGHT_BLUE),
        ("COLOR_LIGHT_GREEN", COLOR_LIGHT_GREEN),
        ("COLOR_LIGHT_CYAN", COLOR_LIGHT_CYAN),
        ("COLOR_LIGHT_RED", COLOR_LIGHT_RED),
        ("COLOR_PINK", COLOR_PINK),
        ("COLOR_YELLOW", COLOR_YELLOW),
        ("COLOR_WHITE", COLOR_WHITE),
    ];
    for (k, (nome, valor)) in cores.iter().enumerate() {
        assert_eq!(*valor as usize, k, "{}", nome);
        let crua = alloc::format!("`{}`", nome);
        assert!(CONTRATO.contains(&crua) && CONTRATO_EN.contains(&crua), "{}", nome);
    }
    assert_eq!((COLOR_DEFAULT_TEXT, COLOR_DEFAULT_BACKGROUND), (14, 0));
    assert!(CONTRATO.contains("`COLOR_DEFAULT_TEXT` (14") && CONTRATO_EN.contains("`COLOR_DEFAULT_TEXT` (14"));
    assert_eq!(MAX_ARGS_LEN, 128);
    assert!(CONTRATO.contains("`MAX_ARGS_LEN` (128") && CONTRATO_EN.contains("`MAX_ARGS_LEN` (128"));
}

#[test_case]
fn o_guia_contem_literalmente_o_teclas_e_os_titulos() {
    let guia = include_str!("../GUIA_DO_PROGRAMADOR.md");
    let fonte = include_str!("../programs/src/bin/teclas.rs");
    assert!(guia.contains(fonte.trim_end()), "o guia nao tem o codigo do teclas literalmente");
    assert!(guia.contains("Programa de exemplo 16: teclas"));
    assert!(guia.contains("## Teclado e tela") && guia.contains("## Argumentos de linha de comando"));
    let en = include_str!("../PROGRAMMERS_GUIDE.md");
    assert!(en.contains("Example program 16: teclas"));
    assert!(en.contains("## Keyboard and screen") && en.contains("## Command-line arguments"));
    // O guia continua trazendo o ftpd (Programa de exemplo 15).
    assert!(guia.contains("Programa de exemplo 15: ftpd"));
}

#[test_case]
fn o_readme_traz_o_marco_13_nos_dois_idiomas() {
    for (nome, readme, fim) in [
        ("README.md", include_str!("../README.md"), "Concluído |"),
        ("README.en.md", include_str!("../README.en.md"), "Completed |"),
    ] {
        let linha_13 = readme.lines().find(|l| l.starts_with("| 13.")).expect("linha do Marco 13");
        assert!(linha_13.ends_with(fim), "{}: {}", nome, linha_13);
        assert!(readme.contains("0.13.0"), "{}", nome);
        assert!(readme.contains("`teclas`") && readme.contains("`exec"), "{}", nome);
    }
    assert!(include_str!("../CHANGELOG.md").contains("[0.13.0]") && include_str!("../CHANGELOG.en.md").contains("[0.13.0]"));
    assert!(include_str!("../WALKTHROUGH.md").contains("Marco 13: teclado cru, tela e arquivo regravável"));
    assert!(include_str!("../WALKTHROUGH.en.md").contains("Milestone 13: raw keyboard, screen and a file that can be rewritten"));
}

#[test_case]
fn a_versao_0_13_0_aparece_no_boot_e_no_sobre() {
    assert_eq!(os_rust::VERSION, "os-rust v0.13.2");
    exige(&rodar_comando("sobre"), "os-rust v0.13.2");
}

// ---------------------------------------------------------------------------
// Não regressão
// ---------------------------------------------------------------------------

#[test_case]
fn um_comando_de_cada_marco_anterior_segue_funcionando() {
    restaurar_tela();
    reset_teclado();
    exige(&rodar_comando("echo oi"), "oi");
    exige(&rodar_comando("ls /ram"), "ola.txt");
    exige(&rodar_comando("cat /ram/ola.txt"), "Ola do ramdisk!");
    let linhas = rodar_comando("data");
    assert!(vga_buffer::screen_count_clock_lines() >= 1 || tem(&linhas, "data:"), "{:?}", linhas);
    exige(&rodar_comando("mkdir /ram/novodir"), "criado");
    exige(&rodar_comando("rmdir /ram/novodir"), "removido");
    exige(&rodar_comando("run hello"), "Ola do ring 3!");
    exige(&rodar_comando("rede"), "RTL8139");
    // Dois programas ao mesmo tempo: cada um escreve as suas linhas.
    let linhas = rodar_comando("run ping pong");
    exige(&linhas, "ping 1");
    exige(&linhas, "pong 1");
    // `help` lista `exec` e a primeira linha continua visível.
    let linhas = rodar_comando("help");
    exige(&linhas, "exec - executa um programa com argumentos");
    restaurar_tela();
}

#[test_case]
fn o_binario_sobe() {
    reset_teclado();
    assert!(!elf_sai(0).is_empty());
}
