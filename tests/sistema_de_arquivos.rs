//! Testes de integração do Marco 8 (sistema de arquivos): comandos `ls`,
//! `cat` e `run` com caminhos, o disco ATA, as syscalls de arquivo e os
//! programas `leitor` e `listador`, de verdade, dentro do QEMU. Cada arquivo
//! de `tests/` é um kernel próprio.
//!
//! Os comandos são "digitados" no prompt pelo mesmo caminho do teclado
//! (`shell::feed`), e o resultado é conferido na tela. Nenhum teste depende de
//! tempo de relógio. Os volumes `/ram` e `/disco` vêm das imagens geradas pelo
//! `build.rs` a partir de `discos/`, e o conteúdo esperado é lido do mesmo
//! lugar (`include_bytes!`), então teste e imagem nunca divergem.
//!
//! O que cada bloco prova:
//! - `ls` e `cat` no ramdisk e no disco, com cada tipo de erro;
//! - `run` por caminho (disco e ramdisk), com as recusas e a mistura de
//!   programas embutidos e de arquivos;
//! - as syscalls de arquivo (`SYS_OPEN`, `SYS_READ`, `SYS_CLOSE`,
//!   `SYS_READ_DIR`) com ELFs montados à mão: cada erro do contrato, o limite de
//!   arquivos abertos e a liberação ao fim do programa;
//! - os programas `leitor` e `listador`, sozinhos e em dobro;
//! - disco ausente, volume inválido e FAT corrompida (cadeia curta demais, com
//!   ciclo ou fora do volume): mensagem clara, sem pânico e sem laço infinito.
//!   O estado do volume `/disco` é trocado por `fs::replace_volume` e sempre
//!   restaurado ao fim de cada teste.
//!
//! Os programas de apoio são ELFs montados à mão, com um mini-assembler de
//! alguns bytes de código de máquina (`Asm`, abaixo). Cada conferência que
//! falha faz o programa sair com um código que diz qual foi.

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
use os_rust::syscall::{
    ERR_BADF, ERR_FAULT, ERR_INVAL, ERR_MFILE, ERR_NAMETOOLONG, ERR_NODEV, ERR_NOENT, ERR_TYPE,
    SYS_CLOSE, SYS_OPEN, SYS_READ, SYS_READ_DIR,
};
use os_rust::user::{
    Termination, USER_HEAP_START, USER_REGION_START, USER_STACK_BOTTOM, USER_STACK_PAGES,
};
use os_rust::blockdev::{BlockDevice, BlockError, RamDisk};
use os_rust::fat::Fat;
use os_rust::fs::{Reason, VolumeId, VolumeState};
use os_rust::{fs, memory, shell, user, vga_buffer};
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

/// Digita uma linha no shell, byte a byte, como o teclado faria, e aperta
/// Enter. `shell::execute` é privada: este é o caminho para rodar um comando.
fn digitar_no_prompt(linha: &str) {
    for byte in linha.bytes() {
        shell::feed(byte);
    }
    shell::feed(b'\n');
}

/// Limpa a tela, roda o comando e devolve as linhas **não vazias** da tela,
/// sem os espaços do fim (e sem o prompt que volta depois do comando).
fn rodar_comando(comando: &str) -> Vec<String> {
    vga_buffer::clear_screen();
    digitar_no_prompt(comando);
    linhas_da_tela()
}

/// As linhas não vazias da tela, sem os espaços do fim.
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

/// Verdadeiro se alguma linha contém `texto`.
fn tem(linhas: &[String], texto: &str) -> bool {
    linhas.iter().any(|linha| linha.contains(texto))
}

/// `panic!` clara, mostrando a tela, se nenhuma linha contém `texto`.
fn exige(linhas: &[String], texto: &str) {
    assert!(tem(linhas, texto), "a tela nao tem {:?}: {:?}", texto, linhas);
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

/// Quantas linhas da tela contêm `texto`.
fn contar(linhas: &[String], texto: &str) -> usize {
    linhas.iter().filter(|linha| linha.contains(texto)).count()
}

/// A mensagem que `visita` escreve.
const VISITA: &str = "visita: fui carregado do disco!";

/// Como `exige`, mas procura o texto na tela inteira com as linhas coladas:
/// uma mensagem que cai no fim de uma linha cheia continua na seguinte.
fn exige_colado(linhas: &[String], texto: &str) {
    let junto: String = linhas.concat();
    assert!(junto.contains(texto), "a tela nao tem {:?}: {:?}", texto, linhas);
}

/// O conteúdo de um arquivo de `discos/`, como texto, sem o `\n` final.
fn linhas_do_arquivo(conteudo: &str) -> Vec<&str> {
    conteudo.lines().collect()
}

const OLA: &str = include_str!("../discos/ram/ola.txt");
const SOBRE: &str = include_str!("../discos/ram/docs/sobre.txt");

// ---------------------------------------------------------------------------
// US1: ls e cat no ramdisk, pelo prompt
// ---------------------------------------------------------------------------

#[test_case]
fn ls_do_ramdisk_pelo_prompt_mostra_os_arquivos_e_o_prompt_volta() {
    let tela = rodar_comando("ls /ram");
    exige(&tela, "ola.txt");
    exige(&tela, "docs");
    exige(&tela, "vazio.txt");
    exige(&tela, "arquivo");
    exige(&tela, "dir");
    // Depois do comando, o prompt responde.
    let tela = rodar_comando("echo oi");
    exige(&tela, "oi");
}

#[test_case]
fn ls_de_subdiretorio_e_cat_de_arquivo_do_ramdisk() {
    let tela = rodar_comando("ls /ram/docs");
    exige(&tela, "sobre.txt");
    let tela = rodar_comando("cat /ram/ola.txt");
    for linha in linhas_do_arquivo(OLA) {
        exige(&tela, linha);
    }
    let tela = rodar_comando("cat /ram/docs/sobre.txt");
    for linha in linhas_do_arquivo(SOBRE) {
        exige(&tela, linha);
    }
}

#[test_case]
fn erros_de_caminho_mostram_mensagem_clara_e_o_prompt_continua() {
    let tela = rodar_comando("ls /ram/nada");
    exige(&tela, "ls: nao encontrado: /ram/nada");
    let tela = rodar_comando("cat /ram/docs");
    exige(&tela, "cat: e um diretorio: /ram/docs");
    let tela = rodar_comando("ls /ram/ola.txt");
    exige(&tela, "ls: nao e um diretorio: /ram/ola.txt");
    let tela = rodar_comando("ls /xyz");
    exige(&tela, "volume desconhecido");
    exige(&tela, "volumes: /ram (ok)");
    let tela = rodar_comando("echo ainda aqui");
    exige(&tela, "ainda aqui");
}

// ---------------------------------------------------------------------------
// US2: run por caminho
// ---------------------------------------------------------------------------

#[test_case]
fn run_de_programa_que_existe_so_no_disco() {
    // `visita` não está na tabela de programas embutidos...
    assert!(user::program_names().all(|nome| nome != "visita"));
    let tela = rodar_comando("run");
    assert!(!tem(&tela, "visita"), "visita nao pode aparecer na lista de embutidos: {:?}", tela);
    // ...mas roda pelo caminho do disco, em ring 3, e o prompt volta.
    let tela = rodar_comando("run /disco/bin/visita");
    exige(&tela, VISITA);
    let tela = rodar_comando("echo depois");
    exige(&tela, "depois");
}

#[test_case]
fn run_de_arquivo_do_ramdisk_usa_o_mesmo_caminho_de_codigo() {
    let tela = rodar_comando("run /ram/bin/hello");
    exige(&tela, "Ola do ring 3!");
    // O caminho aceita qualquer caixa, como todo nome FAT.
    let tela = rodar_comando("run /DISCO/BIN/VISITA");
    exige(&tela, VISITA);
}

#[test_case]
fn run_mistura_programa_do_disco_e_embutido() {
    let tela = rodar_comando("run /disco/bin/visita hello");
    exige(&tela, VISITA);
    exige(&tela, "Ola do ring 3!");
    // O nome do arquivo e o do embutido na mesma linha, na ordem dada.
    let tela = rodar_comando("run hello /disco/bin/visita");
    exige(&tela, VISITA);
    exige(&tela, "Ola do ring 3!");
}

#[test_case]
fn run_com_duas_instancias_do_mesmo_arquivo() {
    let tela = rodar_comando("run /disco/bin/visita /disco/bin/visita");
    assert_eq!(contar(&tela, VISITA), 2, "{:?}", tela);
}

#[test_case]
fn run_de_arquivo_que_nao_e_elf_e_recusado_sem_mapear_nada() {
    let antes = memory::frames_outstanding();
    let tela = rodar_comando("run /ram/ola.txt");
    exige(&tela, "[run] erro ao carregar ola.txt");
    assert!(regiao_do_usuario_esta_livre());
    assert_eq!(memory::frames_outstanding(), antes);
    // Um arquivo do disco que também não é programa.
    let tela = rodar_comando("run /disco/leiame.txt");
    exige(&tela, "[run] erro ao carregar leiame.txt");
    // O prompt segue.
    exige(&rodar_comando("echo vivo"), "vivo");
}

#[test_case]
fn run_de_executavel_grande_demais_cita_o_limite() {
    let antes = memory::frames_outstanding();
    let tela = rodar_comando("run /disco/docs/grande.bin");
    exige(&tela, "arquivo grande demais (65537 bytes; maximo 65536)");
    assert!(regiao_do_usuario_esta_livre());
    assert_eq!(memory::frames_outstanding(), antes);
}

#[test_case]
fn run_recusa_o_comando_inteiro_se_um_alvo_e_invalido() {
    let antes = memory::frames_outstanding();
    // Caminho inexistente.
    let tela = rodar_comando("run /disco/bin/visita /disco/nada");
    exige(&tela, "run: nao encontrado: /disco/nada");
    assert!(!tem(&tela, VISITA), "nenhum programa pode ter sido iniciado: {:?}", tela);
    // Nome embutido inexistente.
    let tela = rodar_comando("run /disco/bin/visita xyz");
    exige(&tela, "programa desconhecido: xyz");
    assert!(!tem(&tela, VISITA));
    // Diretório no lugar de arquivo, e volume desconhecido.
    let tela = rodar_comando("run /disco/bin");
    exige(&tela, "run: e um diretorio: /disco/bin");
    let tela = rodar_comando("run hello /xyz/a");
    exige(&tela, "run: volume desconhecido: /xyz/a");
    assert!(!tem(&tela, "Ola do ring 3!"));
    // Mais de 4 alvos.
    let tela = rodar_comando("run /disco/bin/visita hello hello hello hello");
    exige(&tela, "no maximo 4 programas");
    assert!(!tem(&tela, VISITA));
    assert_eq!(memory::frames_outstanding(), antes);
}

#[test_case]
fn cem_execucoes_de_programa_do_disco_devolvem_todos_os_frames() {
    // Uma rodada para aquecer: cria as tabelas e a lista de frames reciclados.
    vga_buffer::clear_screen();
    digitar_no_prompt("run /disco/bin/visita");
    let antes = memory::frames_outstanding();
    for _ in 0..100 {
        digitar_no_prompt("run /disco/bin/visita");
    }
    assert_eq!(memory::frames_outstanding(), antes, "vazamento de frames");
    assert!(regiao_do_usuario_esta_livre());
}

// ---------------------------------------------------------------------------
// Programas de apoio montados à mão
// ---------------------------------------------------------------------------

/// Monta um ELF64 `ET_EXEC` x86-64 com um único segmento `PT_LOAD` `R E` em
/// `USER_REGION_START`, contendo `code`, com a entrada no primeiro byte. Campos
/// usados (ver `src/elf.rs`): cabeçalho de 64 bytes, um *program header* de 56
/// bytes logo depois, e o código em seguida. (O mesmo construtor de
/// `tests/multitarefa.rs`.)
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

/// Os registradores de uso geral que o mini-assembler usa, com o número que o
/// código de máquina usa.
#[derive(Clone, Copy)]
enum Reg {
    Rax = 0,
    Rdx = 2,
    Rbx = 3,
    Rsi = 6,
    Rdi = 7,
}

use Reg::*;

/// Um mini-assembler: só as instruções que os programas de apoio usam.
struct Asm(Vec<u8>);

impl Asm {
    fn new() -> Asm {
        Asm(Vec::new())
    }

    /// `mov r32, imm32` (zera a metade alta do registrador de 64 bits).
    fn mov32(&mut self, reg: Reg, imm: u32) {
        self.0.push(0xB8 + reg as u8);
        self.0.extend_from_slice(&imm.to_le_bytes());
    }

    /// `movabs r64, imm64`.
    fn movabs(&mut self, reg: Reg, imm: u64) {
        self.0.push(0x48); // REX.W
        self.0.push(0xB8 + reg as u8);
        self.0.extend_from_slice(&imm.to_le_bytes());
    }

    /// `mov dst, src` (64 bits, registrador para registrador).
    fn mov_rr(&mut self, dst: Reg, src: Reg) {
        self.0.push(0x48);
        self.0.push(0x89);
        self.0.push(0xC0 | ((src as u8) << 3) | dst as u8);
    }

    /// `syscall`.
    fn syscall(&mut self) {
        self.0.extend_from_slice(&[0x0F, 0x05]);
    }

    /// `cmp a, b` (calcula `a - b` e liga as flags).
    fn cmp(&mut self, a: Reg, b: Reg) {
        self.0.push(0x48);
        self.0.push(0x39);
        self.0.push(0xC0 | ((b as u8) << 3) | a as u8);
    }

    /// `mov [base], src` (64 bits; `base` não pode ser `rsp`, `rbp`, `r12`, `r13`).
    fn store(&mut self, base: Reg, src: Reg) {
        self.0.push(0x48);
        self.0.push(0x89);
        self.0.push(((src as u8) << 3) | base as u8);
    }

    /// `exit(code)`: `mov edi, code; mov eax, 2; syscall`.
    fn exit(&mut self, code: u32) {
        self.mov32(Rdi, code);
        self.mov32(Rax, 2);
        self.syscall();
    }

    /// Tamanho em bytes de `exit`: 5 + 5 + 2.
    const EXIT_LEN: u8 = 12;

    /// Chama a syscall `nr` com três argumentos e exige que o resultado seja
    /// `esperado`; senão o programa sai com `code`. O resultado vai para `rbx`
    /// (que a syscall preserva) e é comparado em `rax`.
    fn chamada(&mut self, nr: u64, args: [u64; 3], esperado: i64, code: u32) {
        self.movabs(Rax, nr);
        self.movabs(Rdi, args[0]);
        self.movabs(Rsi, args[1]);
        self.movabs(Rdx, args[2]);
        self.syscall();
        self.mov_rr(Rbx, Rax);
        self.movabs(Rax, esperado as u64);
        self.cmp(Rbx, Rax);
        self.0.extend_from_slice(&[0x74, Asm::EXIT_LEN]); // je +12: igual, pula o exit
        self.exit(code);
    }

    /// `jmp rel32` sobre `len` bytes de dados.
    fn pular_dados(&mut self, len: usize) {
        self.0.push(0xE9);
        self.0.extend_from_slice(&(len as u32).to_le_bytes());
    }
}

/// Monta um programa que carrega `dados` no começo do segmento (depois de um
/// `jmp` que os pula) e roda o código que `escreve` gera. `escreve` recebe o
/// endereço, no programa, de cada pedaço de `dados`, para usá-los como
/// ponteiros. O segmento é `R E`: os dados são legíveis e **não** graváveis.
fn programa(dados: &[&[u8]], escreve: impl FnOnce(&mut Asm, &[u64])) -> Vec<u8> {
    let total: usize = dados.iter().map(|d| d.len()).sum();
    let mut enderecos = Vec::new();
    let mut corrente = USER_REGION_START + 5; // depois do `jmp rel32`
    for d in dados {
        enderecos.push(corrente);
        corrente += d.len() as u64;
    }
    let mut a = Asm::new();
    a.pular_dados(total);
    for d in dados {
        a.0.extend_from_slice(d);
    }
    escreve(&mut a, &enderecos);
    a.exit(0);
    synth_elf(&a.0)
}

/// Um buffer gravável do programa: dentro da pilha dele.
const BUFFER: u64 = USER_STACK_BOTTOM + 0x1000;

/// Roda um programa sintético sozinho e devolve como terminou.
fn rodar_sintetico(nome: &str, elf: &[u8]) -> Termination {
    let relatorio = user::run_images(&[(nome, elf)], None).expect("o pedido de run foi recusado");
    assert_eq!(relatorio.finished.len(), 1);
    relatorio.finished[0].termination
}

// ---------------------------------------------------------------------------
// US3: syscalls de arquivo
// ---------------------------------------------------------------------------

#[test_case]
fn open_devolve_cada_erro_do_contrato() {
    let ola = b"/ram/ola.txt";
    let inexistente = b"/disco/nada";
    let volume = b"/xyz/a";
    let no_arquivo = b"/ram/ola.txt/x";
    let ponto_ponto = b"/ram/..";
    let sem_barra = b"ram";
    let elf = programa(&[ola, inexistente, volume, no_arquivo, ponto_ponto, sem_barra], |a, p| {
        let open = |a: &mut Asm, ptr: u64, len: usize, esperado: i64, code: u32| {
            a.chamada(SYS_OPEN, [ptr, len as u64, 0], esperado, code)
        };
        open(a, 0x1000, 5, ERR_FAULT, 10); // ponteiro fora da região do usuário
        open(a, 0xFFFF_FFFF_FFFF_F000, 8, ERR_FAULT, 11); // ponteiro enorme
        open(a, USER_REGION_START - 4, 8, ERR_FAULT, 12); // cruza o começo da região
        open(a, p[0], 0, ERR_INVAL, 13); // caminho vazio
        open(a, p[0], 65, ERR_NAMETOOLONG, 14); // 65 bytes
        open(a, p[0], usize::MAX, ERR_NAMETOOLONG, 15); // tamanho absurdo
        open(a, p[1], inexistente.len(), ERR_NOENT, 16);
        open(a, p[2], volume.len(), ERR_NODEV, 17);
        open(a, p[3], no_arquivo.len(), ERR_TYPE, 18);
        open(a, p[4], ponto_ponto.len(), ERR_INVAL, 19);
        open(a, p[5], sem_barra.len(), ERR_INVAL, 20);
        // E um caminho bom funciona depois de todos os erros.
        open(a, p[0], ola.len(), 0, 21);
    });
    assert_eq!(rodar_sintetico("fs_open", &elf), Termination::Exit { code: 0 });
    assert_eq!(fs::open_files_in_use(), 0);
}

#[test_case]
fn read_read_dir_e_close_validam_tudo_que_vem_do_programa() {
    let ola = b"/ram/ola.txt";
    let ram = b"/ram";
    let vazio = b"/ram/vazio.txt";
    let elf = programa(&[ola, ram, vazio], |a, p| {
        a.chamada(SYS_OPEN, [p[0], ola.len() as u64, 0], 0, 10); // fd 0: arquivo
        a.chamada(SYS_OPEN, [p[1], ram.len() as u64, 0], 1, 11); // fd 1: diretório
        // read: ponteiro ruim, página só-leitura (os dados do próprio programa),
        // tamanho zero e grande demais.
        a.chamada(SYS_READ, [0, 0x1000, 10], ERR_FAULT, 12);
        a.chamada(SYS_READ, [0, p[0], 10], ERR_FAULT, 13);
        a.chamada(SYS_READ, [0, BUFFER, 0], 0, 14);
        a.chamada(SYS_READ, [0, BUFFER, 4097], ERR_INVAL, 15);
        // read: descritores inválidos.
        a.chamada(SYS_READ, [4, BUFFER, 8], ERR_BADF, 16);
        a.chamada(SYS_READ, [u64::MAX, BUFFER, 8], ERR_BADF, 17);
        a.chamada(SYS_READ, [2, BUFFER, 8], ERR_BADF, 18); // nunca aberto
        // Tipo errado nos dois sentidos.
        a.chamada(SYS_READ, [1, BUFFER, 8], ERR_TYPE, 19);
        a.chamada(SYS_READ_DIR, [0, BUFFER, 20], ERR_TYPE, 20);
        // read_dir: buffer pequeno demais, grande demais, ponteiro ruim.
        a.chamada(SYS_READ_DIR, [1, BUFFER, 19], ERR_INVAL, 21);
        a.chamada(SYS_READ_DIR, [1, BUFFER, 4097], ERR_INVAL, 22);
        a.chamada(SYS_READ_DIR, [1, 0x1000, 20], ERR_FAULT, 23);
        a.chamada(SYS_READ_DIR, [1, p[0], 20], ERR_FAULT, 24); // página só-leitura
        a.chamada(SYS_READ_DIR, [7, BUFFER, 20], ERR_BADF, 25);
        // Uma entrada de verdade, e o que foi lido de `read` tem o tamanho certo.
        a.chamada(SYS_READ_DIR, [1, BUFFER, 20], 1, 26);
        a.chamada(SYS_READ, [0, BUFFER, 3], 3, 27);
        a.chamada(SYS_READ, [0, BUFFER, 4096], (include_str!("../discos/ram/ola.txt").len() - 3) as i64, 28);
        a.chamada(SYS_READ, [0, BUFFER, 4096], 0, 29); // fim do arquivo
        // close: válido, de novo, fora da tabela, enorme.
        a.chamada(SYS_CLOSE, [1, 0, 0], 0, 30);
        a.chamada(SYS_CLOSE, [1, 0, 0], ERR_BADF, 31);
        a.chamada(SYS_CLOSE, [4, 0, 0], ERR_BADF, 32);
        a.chamada(SYS_CLOSE, [u64::MAX, 0, 0], ERR_BADF, 33);
        // Arquivo vazio: lê zero bytes, sem erro.
        a.chamada(SYS_OPEN, [p[2], vazio.len() as u64, 0], 1, 34);
        a.chamada(SYS_READ, [1, BUFFER, 10], 0, 35);
    });
    assert_eq!(rodar_sintetico("fs_valida", &elf), Termination::Exit { code: 0 });
    assert_eq!(fs::open_files_in_use(), 0, "o fim do programa fecha o que ficou aberto");
}

#[test_case]
fn limite_de_arquivos_abertos_e_reuso_do_menor_descritor() {
    let ola = b"/ram/ola.txt";
    let elf = programa(&[ola], |a, p| {
        let n = ola.len() as u64;
        for fd in 0..4 {
            a.chamada(SYS_OPEN, [p[0], n, 0], fd, 10 + fd as u32);
        }
        a.chamada(SYS_OPEN, [p[0], n, 0], ERR_MFILE, 20);
        a.chamada(SYS_CLOSE, [2, 0, 0], 0, 21);
        a.chamada(SYS_OPEN, [p[0], n, 0], 2, 22); // o menor livre volta
        a.chamada(SYS_OPEN, [p[0], n, 0], ERR_MFILE, 23);
    });
    assert_eq!(rodar_sintetico("fs_limite", &elf), Termination::Exit { code: 0 });
    assert_eq!(fs::open_files_in_use(), 0);
}

#[test_case]
fn arquivos_abertos_sao_liberados_no_exit_e_no_erro_e_os_outros_seguem() {
    let ola = b"/ram/ola.txt";
    // Termina com `exit(0)` sem fechar nada.
    let sai = programa(&[ola], |a, p| {
        a.chamada(SYS_OPEN, [p[0], ola.len() as u64, 0], 0, 10);
        a.chamada(SYS_OPEN, [p[0], ola.len() as u64, 0], 1, 11);
    });
    assert_eq!(rodar_sintetico("fs_sai", &sai), Termination::Exit { code: 0 });
    assert_eq!(fs::open_files_in_use(), 0);
    // Termina com erro de memória, com um arquivo aberto.
    let cai = programa(&[ola], |a, p| {
        a.chamada(SYS_OPEN, [p[0], ola.len() as u64, 0], 0, 10);
        a.movabs(Rax, 0);
        a.store(Rax, Rax); // mov [0], rax: #PF
    });
    let relatorio = user::run_images(&[("fs_cai", &cai[..]), ("hello", imagem_de("hello"))], None).unwrap();
    assert_eq!(fs::open_files_in_use(), 0);
    let hello = relatorio.finished.iter().find(|f| f.name == "hello").unwrap();
    assert_eq!(hello.termination, Termination::Exit { code: 0 });
    let cai = relatorio.finished.iter().find(|f| f.name == "fs_cai").unwrap();
    assert!(matches!(cai.termination, Termination::Fault { mnemonic: "#PF", .. }));
}

#[test_case]
fn primeiro_numero_fora_do_contrato_continua_encerrando_so_o_programa() {
    // O contrato v5 vai até `SYS_TIME` (10); o 11 é o primeiro número livre.
    let mut a = Asm::new();
    a.mov32(Rax, 11);
    a.syscall();
    let elf = synth_elf(&a.0);
    assert_eq!(rodar_sintetico("fs_onze", &elf), Termination::BadSyscall { number: 11 });
}

/// A imagem ELF de um programa embutido.
fn imagem_de(nome: &str) -> &'static [u8] {
    user::builtin_image(nome).unwrap_or_else(|| panic!("programa embutido ausente: {}", nome))
}

// ---------------------------------------------------------------------------
// US3: leitor e listador
// ---------------------------------------------------------------------------

const LONGO: &str = include_str!("../discos/disco/docs/longo.txt");
const LEIAME: &str = include_str!("../discos/disco/leiame.txt");

/// Exige que o texto de `arquivo` apareça na tela, linha por linha e em ordem
/// (as linhas em branco do arquivo não aparecem em `linhas_da_tela`).
fn exige_texto_na_ordem(tela: &[String], arquivo: &str) {
    let esperadas: Vec<&str> = arquivo.lines().filter(|l| !l.is_empty()).collect();
    let inicio = tela
        .iter()
        .position(|linha| linha == esperadas[0])
        .unwrap_or_else(|| panic!("a tela nao tem a primeira linha {:?}: {:?}", esperadas[0], tela));
    for (k, esperada) in esperadas.iter().enumerate() {
        assert_eq!(tela.get(inicio + k).map(|s| s.as_str()), Some(*esperada), "linha {} do arquivo", k);
    }
}

#[test_case]
fn leitor_escreve_o_conteudo_exato_de_um_arquivo_de_dois_clusters() {
    assert!(LONGO.len() > 512, "o arquivo precisa ocupar mais de um cluster");
    let tela = rodar_comando("run leitor");
    exige_texto_na_ordem(&tela, LONGO);
    assert_eq!(fs::open_files_in_use(), 0);
}

#[test_case]
fn listador_escreve_as_entradas_do_diretorio() {
    let tela = rodar_comando("run listador");
    exige(&tela, "dir 0 bin");
    exige(&tela, "dir 0 docs");
    exige(&tela, &alloc::format!("arquivo {} leiame.txt", LEIAME.len()));
    assert_eq!(contar(&tela, "listador:"), 0, "nenhum erro: {:?}", tela);
}

/// Quantas vezes cada byte ASCII aparece em `texto`, sem contar quebras de linha.
fn histograma(texto: &str) -> [u16; 128] {
    let mut h = [0u16; 128];
    for b in texto.bytes().filter(|&b| b != b'\n') {
        h[b as usize] += 1;
    }
    h
}

#[test_case]
fn dois_leitores_ao_mesmo_tempo_recebem_o_conteudo_completo() {
    // Com dois programas, o timer pode trocar de um para o outro entre duas
    // escritas, então as linhas de um e do outro podem se misturar na tela. O
    // que não pode acontecer é um deles perder ou repetir bytes: o texto
    // somado da tela precisa ter, caractere por caractere, o conteúdo do
    // arquivo duas vezes.
    let tela = rodar_comando("run leitor leitor");
    let saida: String = tela
        .iter()
        .filter(|linha| !linha.starts_with("run ") && !linha.starts_with("os-rust>"))
        .map(|linha| linha.as_str())
        .collect();
    let mut esperado = histograma(LONGO);
    for n in esperado.iter_mut() {
        *n *= 2;
    }
    assert_eq!(histograma(&saida)[..], esperado[..], "a tela: {:?}", tela);
    assert_eq!(fs::open_files_in_use(), 0);
}

#[test_case]
fn leitor_e_listador_juntos_com_um_embutido() {
    let tela = rodar_comando("run leitor listador hello");
    exige(&tela, LONGO.lines().next().unwrap());
    exige(&tela, "dir 0 docs");
    exige(&tela, "Ola do ring 3!");
}

// ---------------------------------------------------------------------------
// US4: disco ausente, volume inválido, FAT corrompida
// ---------------------------------------------------------------------------

/// Uma imagem FAT16 com `docs/longo.txt` e `leiame.txt`, como a do disco.
fn imagem_de_disco() -> Vec<u8> {
    let mut b = fatimg::Builder::new();
    b.file("leiame.txt", LEIAME.as_bytes()).unwrap();
    b.dir("docs").unwrap();
    b.file("docs/longo.txt", LONGO.as_bytes()).unwrap();
    b.build().unwrap()
}

/// Faz da imagem um dispositivo `'static` (vazando a memória, o que um teste
/// pode) para o volume `/disco` apontar para ele.
fn vazar(img: Vec<u8>) -> fs::Device {
    let bytes: &'static [u8] = alloc::boxed::Box::leak(img.into_boxed_slice());
    alloc::boxed::Box::leak(alloc::boxed::Box::new(RamDisk(bytes)))
}

/// Roda `f` com o volume `/disco` no estado dado e restaura o original.
fn com_disco(estado: VolumeState, f: impl FnOnce()) {
    let original = fs::replace_volume(VolumeId::Disco, estado);
    f();
    fs::replace_volume(VolumeId::Disco, original);
}

fn disco_de(img: Vec<u8>) -> VolumeState {
    fs::mount_state(vazar(img))
}

/// Uma imagem em memória que não precisa ser `'static` (para só consultar).
struct MemDisk<'a>(&'a [u8]);

impl BlockDevice for MemDisk<'_> {
    fn sector_count(&self) -> u32 {
        (self.0.len() / 512) as u32
    }

    fn read_sector(&self, lba: u32, buf: &mut [u8; 512]) -> Result<(), BlockError> {
        let start = lba as usize * 512;
        buf.copy_from_slice(self.0.get(start..start + 512).ok_or(BlockError::OutOfRange)?);
        Ok(())
    }
}

/// Posição, na imagem, da entrada de diretório de `docs/longo.txt` (a terceira
/// do diretório `docs`, depois de `.` e `..`).
fn entrada_do_longo(img: &[u8]) -> usize {
    let dev = MemDisk(img);
    let fat = Fat::mount(&dev).unwrap();
    let docs = fat.lookup(&[*b"DOCS       "]).unwrap();
    (fat.geometry().data_start as usize + docs.first_cluster as usize - 2) * 512 + 2 * 32
}

fn primeiro_cluster_do_longo(img: &[u8]) -> u16 {
    let at = entrada_do_longo(img);
    u16::from_le_bytes([img[at + 26], img[at + 27]])
}

fn escreve_fat(img: &mut [u8], cluster: u16, valor: u16) {
    for copia in 0..2 {
        let at = (1 + copia * 17) * 512 + cluster as usize * 2;
        img[at..at + 2].copy_from_slice(&valor.to_le_bytes());
    }
}

#[test_case]
fn sem_disco_o_ramdisk_segue_e_o_disco_informa_indisponivel() {
    com_disco(VolumeState::Unavailable(Reason::NoDisk), || {
        exige(&rodar_comando("ls /disco"), "ls: volume indisponivel: /disco (sem disco)");
        exige(&rodar_comando("cat /disco/leiame.txt"), "cat: volume indisponivel: /disco (sem disco)");
        let tela = rodar_comando("run /disco/bin/visita");
        exige(&tela, "run: volume indisponivel: /disco (sem disco)");
        assert!(!tem(&tela, VISITA));
        let tela = rodar_comando("ls");
        exige(&tela, "/ram (ok)");
        exige(&tela, "/disco (indisponivel)");
        // O ramdisk não foi afetado, e o prompt responde.
        exige(&rodar_comando("cat /ram/ola.txt"), OLA.trim_end());
        exige(&rodar_comando("echo vivo"), "vivo");
        // O programa `leitor` recebe o erro de volume e sai com 1.
        let tela = rodar_comando("run leitor");
        exige(&tela, "leitor: nao abriu /disco/docs/longo.txt: volume indisponivel");
        exige(&tela, "[run] leitor terminou com codigo 1");
        let tela = rodar_comando("run listador");
        exige(&tela, "[run] listador terminou com codigo 1");
    });
    assert_eq!(fs::volume_status(VolumeId::Disco), Ok(()), "o estado original foi restaurado");
}

#[test_case]
fn volume_com_setor_de_boot_invalido_e_recusado_com_mensagem_clara() {
    let mut img = imagem_de_disco();
    img[510] = 0;
    let estado = disco_de(img);
    assert!(matches!(estado, VolumeState::Unavailable(Reason::BadVolume("assinatura"))));
    com_disco(estado, || {
        exige(&rodar_comando("ls /disco"), "ls: volume indisponivel: /disco (volume FAT invalido: assinatura)");
        exige(&rodar_comando("echo vivo"), "vivo");
    });
}

#[test_case]
fn erro_de_leitura_do_disco_deixa_o_volume_indisponivel() {
    com_disco(VolumeState::Unavailable(Reason::IoError), || {
        exige(&rodar_comando("ls /disco"), "ls: volume indisponivel: /disco (erro de leitura)");
    });
    com_disco(VolumeState::Unavailable(Reason::NotAta), || {
        exige(&rodar_comando("ls /disco"), "dispositivo nao e um disco ATA");
    });
}

#[test_case]
fn cadeia_mais_curta_que_o_tamanho_mostra_erro_de_leitura_e_nao_trava() {
    let mut img = imagem_de_disco();
    let at = entrada_do_longo(&img);
    img[at + 28..at + 32].copy_from_slice(&5000u32.to_le_bytes());
    com_disco(disco_de(img), || {
        // `cat` mostra o que a cadeia tem (dois clusters) e depois o erro.
        let tela = rodar_comando("cat /disco/docs/longo.txt");
        exige(&tela, "cat: erro de leitura (disco ou volume corrompido): /disco/docs/longo.txt");
        exige(&tela, LONGO.lines().next().unwrap());
        // O programa recebe os bytes lidos e depois o erro; sai com o código 2.
        let tela = rodar_comando("run leitor");
        exige_colado(&tela, "leitor: erro de leitura: erro de leitura");
        exige(&tela, "[run] leitor terminou com codigo 2");
        assert_eq!(fs::open_files_in_use(), 0);
        exige(&rodar_comando("echo vivo"), "vivo");
    });
}

#[test_case]
fn cadeia_com_ciclo_e_cadeia_fora_do_volume_terminam_em_erro_de_io() {
    // Ciclo de dois clusters e um arquivo que declara mais bytes do que o volume
    // inteiro: o limite de passos termina a leitura em erro.
    let mut img = imagem_de_disco();
    let c = primeiro_cluster_do_longo(&img);
    escreve_fat(&mut img, c, c + 1);
    escreve_fat(&mut img, c + 1, c);
    let at = entrada_do_longo(&img);
    img[at + 28..at + 32].copy_from_slice(&3_000_000u32.to_le_bytes());
    com_disco(disco_de(img), || {
        assert_eq!(fs::read_whole(b"/disco/docs/longo.txt", 8 << 20), Err(fs::FsError::Io));
        // Os outros arquivos do volume continuam legíveis.
        assert_eq!(fs::read_whole(b"/disco/leiame.txt", 4096).unwrap(), LEIAME.as_bytes());
    });
    // Um cluster que aponta para fora do volume.
    let mut img = imagem_de_disco();
    let c = primeiro_cluster_do_longo(&img);
    escreve_fat(&mut img, c, 0x2000);
    com_disco(disco_de(img), || {
        assert_eq!(fs::read_whole(b"/disco/docs/longo.txt", 8 << 20), Err(fs::FsError::Io));
        exige(&rodar_comando("cat /disco/docs/longo.txt"), "erro de leitura");
        exige(&rodar_comando("echo vivo"), "vivo");
    });
}
