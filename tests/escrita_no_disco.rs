//! Testes de integração do Marco 11 (escrita no disco ATA): o `/disco`
//! gravável pelos comandos `grava`, `anexa` e `apaga`, `mkdir` e `rmdir` (prompt
//! e syscalls 15 e 16), o programa `caderno`, o contrato versão 7 e os
//! documentos, de verdade, dentro do QEMU. Cada arquivo de `tests/` é um kernel
//! próprio, então os ajudantes de `escrita_no_ramdisk.rs` (tela, mini-assembler
//! `Asm`, ELFs montados à mão) são repetidos aqui.
//!
//! O QEMU dos testes roda com `snapshot=on`: o que se grava em `/disco` vai
//! para um arquivo temporário e a imagem original gerada pelo `build.rs` nunca
//! muda. Mesmo assim, todo teste que escreve no `/disco` o deixa como
//! encontrou: apaga o que criou e confere a listagem e os clusters livres.

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
use os_rust::ata::{AtaDisk, Bus, Drive};
use os_rust::blockdev::{BlockDevice, WritableRam};
use os_rust::fat::Fat;
use os_rust::fs::{OpenFile, Reason, VolumeId, VolumeState};
use os_rust::syscall::{
    ERR_BADF, ERR_BUSY, ERR_EXIST, ERR_FAULT, ERR_INVAL, ERR_IO, ERR_MFILE, ERR_NAMETOOLONG,
    ERR_NODEV, ERR_NOENT, ERR_NOSPC, ERR_NOTEMPTY, ERR_ROFS, ERR_TYPE, SYS_CLOSE, SYS_CREATE,
    SYS_MKDIR, SYS_OPEN, SYS_OPEN_APPEND, SYS_READ, SYS_READ_DIR, SYS_REMOVE, SYS_RMDIR,
    SYS_WRITE_FILE, SYS_YIELD,
};
use os_rust::user::{Termination, USER_REGION_START, USER_STACK_BOTTOM};
use os_rust::{fs, memory, shell, user, vga_buffer};

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

/// Limpa a tela, roda o comando e devolve as linhas não vazias da tela.
fn rodar_comando(comando: &str) -> Vec<String> {
    vga_buffer::clear_screen();
    digitar_no_prompt(comando);
    linhas_da_tela()
}

fn tem(linhas: &[String], texto: &str) -> bool {
    linhas.iter().any(|linha| linha.contains(texto))
}

fn exige(linhas: &[String], texto: &str) {
    assert!(tem(linhas, texto), "a tela nao tem {:?}: {:?}", texto, linhas);
}

/// O `/disco` como estava antes de um teste: clusters livres e listagem da
/// raiz. Diferente do `/ram`, não há como "recarregar" o disco: o teste apaga o
/// que criou e `confere` prova que tudo voltou ao que era.
struct Disco {
    livres: u32,
    raiz: Vec<String>,
}

impl Disco {
    fn limpo() -> Disco {
        assert!(fs::volume_status(VolumeId::Disco).is_ok(), "o /disco deveria estar disponivel");
        assert_eq!(fs::open_files_in_use(), 0);
        Disco { livres: fs::free_clusters(VolumeId::Disco).unwrap(), raiz: rodar_comando("ls /disco") }
    }

    fn confere(self) {
        assert_eq!(fs::free_clusters(VolumeId::Disco).unwrap(), self.livres, "clusters livres do /disco mudaram");
        assert_eq!(rodar_comando("ls /disco"), self.raiz, "a listagem do /disco mudou");
        assert_eq!(fs::open_files_in_use(), 0);
    }
}

/// O `/ram` como estava antes de um teste (os testes de diretório rodam nos
/// dois volumes): clusters livres e listagem da raiz.
struct Ram {
    livres: u32,
    raiz: Vec<String>,
}

impl Ram {
    fn limpo() -> Ram {
        fs::reset_ramdisk();
        assert_eq!(fs::open_files_in_use(), 0);
        Ram { livres: fs::free_clusters(VolumeId::Ram).unwrap(), raiz: rodar_comando("ls /ram") }
    }

    fn confere(self) {
        fs::reset_ramdisk();
        assert_eq!(fs::free_clusters(VolumeId::Ram).unwrap(), self.livres);
        assert_eq!(rodar_comando("ls /ram"), self.raiz);
        assert_eq!(fs::open_files_in_use(), 0);
    }
}

fn conteudo(path: &[u8]) -> Vec<u8> {
    fs::read_whole(path, 1 << 20).unwrap()
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



/// Roda um programa sintético sozinho e devolve como terminou.
fn rodar_sintetico(nome: &str, elf: &[u8]) -> Termination {
    let relatorio = user::run_images(&[(nome, elf)], None).expect("o pedido de run foi recusado");
    assert_eq!(relatorio.finished.len(), 1);
    relatorio.finished[0].termination
}

fn sai_bem(nome: &str, elf: &[u8]) {
    assert_eq!(rodar_sintetico(nome, elf), Termination::Exit { code: 0 }, "programa {}", nome);
    assert_eq!(fs::open_files_in_use(), 0, "o fim do programa fecha o que ficou aberto");
}

/// Roda `f` com o volume dado no lugar de `vol` e restaura o original.
fn com_volume(vol: VolumeId, estado: VolumeState, f: impl FnOnce()) {
    let original = fs::replace_volume(vol, estado);
    f();
    fs::replace_volume(vol, original);
}

/// Vaza a imagem como dispositivo gravável `'static` (um teste pode).
fn vazar_gravavel(img: Vec<u8>) -> fs::Device {
    alloc::boxed::Box::leak(alloc::boxed::Box::new(WritableRam::new(&img)))
}

/// Um resumo (FNV-1a) de todos os setores do dispositivo: dá para comparar
/// antes e depois sem guardar 2 MiB de cópia no heap.
fn resumo(dev: fs::Device) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut buf = [0u8; 512];
    for lba in 0..dev.sector_count() {
        dev.read_sector(lba, &mut buf).unwrap();
        for &byte in buf.iter() {
            hash = (hash ^ byte as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    hash
}

/// Um buffer gravável do programa: dentro da pilha dele.
const BUFFER: u64 = USER_STACK_BOTTOM + 0x1000;


// ---------------------------------------------------------------------------
// US1: gravar no disco
// ---------------------------------------------------------------------------

/// Os bytes do arquivo `caminho` lidos por um `Fat` montado do zero sobre um
/// disco recém-detectado: nada do kernel (nenhum cache) participa.
fn ler_do_dispositivo(caminho: &[&str]) -> Option<Vec<u8>> {
    let disco = AtaDisk::detect(Bus::Primary, Drive::Slave).expect("o disco de dados existe");
    let fat = Fat::mount(&disco).expect("volume valido");
    let comps: Vec<[u8; 11]> = caminho
        .iter()
        .map(|c| {
            let (base, ext) = c.split_once('.').unwrap_or((c, ""));
            let mut nome = [b' '; 11];
            for (i, b) in base.bytes().enumerate() {
                nome[i] = b.to_ascii_uppercase();
            }
            for (i, b) in ext.bytes().enumerate() {
                nome[8 + i] = b.to_ascii_uppercase();
            }
            nome
        })
        .collect();
    let node = fat.lookup(&comps).ok()?;
    let mut cursor = fat.cursor(&node).unwrap();
    let mut dados = Vec::new();
    let mut pedaco = [0u8; 100];
    loop {
        let n = fat.read(&mut cursor, &mut pedaco).unwrap();
        if n == 0 {
            return Some(dados);
        }
        dados.extend_from_slice(&pedaco[..n]);
    }
}

#[test_case]
fn sem_cache_no_kernel_criar_acrescentar_e_remover_chegam_ao_dispositivo() {
    let disco = Disco::limpo();
    let mut arquivo = OpenFile::create(b"/disco/cache.txt").unwrap();
    let primeiro: Vec<u8> = (0..700u32).map(|i| b'a' + (i % 26) as u8).collect();
    assert_eq!(arquivo.write(&primeiro).unwrap(), 700);
    let mut mais = OpenFile::open_append(b"/disco/cache.txt").unwrap();
    assert_eq!(mais.write(b"0123456789").unwrap(), 10);
    // Um disco detectado de novo e um `Fat` montado do zero enxergam tudo.
    let mut esperado = primeiro.clone();
    esperado.extend_from_slice(b"0123456789");
    assert_eq!(ler_do_dispositivo(&["cache.txt"]), Some(esperado));
    fs::remove(b"/disco/cache.txt").unwrap();
    assert_eq!(ler_do_dispositivo(&["cache.txt"]), None);
    disco.confere();
}

#[test_case]
fn grava_anexa_e_apaga_no_disco_tem_as_mensagens_do_ram() {
    let disco = Disco::limpo();
    exige(&rodar_comando("grava /disco/nota.txt gravado no disco"), "grava: criado /disco/nota.txt (17 bytes)");
    exige(&rodar_comando("cat /disco/nota.txt"), "gravado no disco");
    let ls = rodar_comando("ls /disco");
    assert!(ls.iter().any(|l| l.contains("nota.txt") && l.contains("17")), "{:?}", ls);
    exige(&rodar_comando("grava /disco/nota.txt outra"), "grava: ja existe: /disco/nota.txt");
    exige(&rodar_comando("anexa /disco/nota.txt mais"), "anexa: /disco/nota.txt agora tem 22 bytes");
    // Ampliar passando de 512 bytes (cada linha tem 100 caracteres (a linha digitada cabe em 128) mais a quebra).
    let linha: String = core::iter::repeat('x').take(100).collect();
    for _ in 0..5 {
        rodar_comando(&alloc::format!("anexa /disco/nota.txt {}", linha));
    }
    assert_eq!(conteudo(b"/disco/nota.txt").len(), 22 + 5 * 101);
    assert_eq!(ler_do_dispositivo(&["nota.txt"]).unwrap().len(), 22 + 5 * 101);
    exige(&rodar_comando("apaga /disco/nota.txt"), "apaga: removido /disco/nota.txt");
    exige(&rodar_comando("cat /disco/nota.txt"), "cat: nao encontrado: /disco/nota.txt");
    disco.confere();
}

#[test_case]
fn imagem_do_disco_protegida_por_snapshot() {
    // D2: o `cargo run` e o `cargo test` nunca alteram a imagem original.
    let cargo = include_str!("../Cargo.toml");
    let run_args = cargo.lines().find(|l| l.trim_start().starts_with("run-args")).expect("run-args");
    assert!(run_args.contains("snapshot=on"), "{}", run_args);
    let mut discos = 0;
    for linha in cargo.lines().filter(|l| l.contains("file=target/imagens/")) {
        assert!(linha.contains("snapshot=on"), "disco sem snapshot=on: {}", linha);
        discos += 1;
    }
    // Um em `run-args` e dois em `test-args`.
    assert!(discos >= 3, "esperava 3 discos de dados, achei {}", discos);
}

// ---------------------------------------------------------------------------
// US2: criar e remover diretórios (prompt)
// ---------------------------------------------------------------------------

/// A sequência da História 2 sobre o volume `vol` (`ram` ou `disco`), com o
/// arquivo existente `arquivo` na raiz e o diretório `docs` (que tem arquivos).
fn historia_2(vol: &str, arquivo: &str) {
    let raiz = alloc::format!("/{}", vol);
    let docs = alloc::format!("/{}/docs", vol);
    let notas = alloc::format!("/{}/docs/notas", vol);
    exige(&rodar_comando(&alloc::format!("mkdir {}", notas)), &alloc::format!("mkdir: criado {}", notas));
    // Aparece como diretório no pai, e dentro dele não há nada (nem `.` nem `..`).
    let ls = rodar_comando(&alloc::format!("ls {}", docs));
    assert!(ls.iter().any(|l| l.starts_with("dir") && l.ends_with("notas")), "{:?}", ls);
    let vazio = rodar_comando(&alloc::format!("ls {}", notas));
    assert!(vazio.iter().all(|l| !l.starts_with("dir") && !l.starts_with("arquivo")), "{:?}", vazio);
    // Dá para gravar dentro dele.
    let a = alloc::format!("{}/a.txt", notas);
    exige(&rodar_comando(&alloc::format!("grava {} x", a)), &alloc::format!("grava: criado {} (2 bytes)", a));
    exige(&rodar_comando(&alloc::format!("rmdir {}", notas)), &alloc::format!("rmdir: diretorio nao vazio: {}", notas));
    exige(&rodar_comando(&alloc::format!("apaga {}", a)), "apaga: removido");
    // Erros de `mkdir`.
    exige(&rodar_comando(&alloc::format!("mkdir {}", docs)), &alloc::format!("mkdir: ja existe: {}", docs));
    exige(&rodar_comando(&alloc::format!("mkdir {}", raiz)), &alloc::format!("mkdir: ja existe: {}", raiz));
    let arq = alloc::format!("/{}/{}", vol, arquivo);
    exige(&rodar_comando(&alloc::format!("mkdir {}", arq)), &alloc::format!("mkdir: ja existe: {}", arq));
    let nada = alloc::format!("/{}/nada/x", vol);
    exige(&rodar_comando(&alloc::format!("mkdir {}", nada)), &alloc::format!("mkdir: nao encontrado: {}", nada));
    let no_meio = alloc::format!("{}/x", arq);
    exige(&rodar_comando(&alloc::format!("mkdir {}", no_meio)), &alloc::format!("mkdir: nao e um diretorio: {}", no_meio));
    exige(&rodar_comando("echo vivo"), "vivo");
    // Erros de `rmdir`.
    exige(&rodar_comando(&alloc::format!("rmdir {}", docs)), &alloc::format!("rmdir: diretorio nao vazio: {}", docs));
    exige(&rodar_comando(&alloc::format!("rmdir {}", arq)), &alloc::format!("rmdir: nao e um diretorio: {}", arq));
    exige(&rodar_comando(&alloc::format!("rmdir {}", raiz)), &alloc::format!("rmdir: diretorio em uso: {}", raiz));
    let inexistente = alloc::format!("/{}/nada", vol);
    exige(&rodar_comando(&alloc::format!("rmdir {}", inexistente)), &alloc::format!("rmdir: nao encontrado: {}", inexistente));
    exige(&rodar_comando("echo vivo"), "vivo");
    // O sucesso final.
    exige(&rodar_comando(&alloc::format!("rmdir {}", notas)), &alloc::format!("rmdir: removido {}", notas));
    assert!(!tem(&rodar_comando(&alloc::format!("ls {}", docs)), "notas"));
    // O diretório não cresce: 14 arquivos cabem, o 15º não.
    let lote = alloc::format!("/{}/lote", vol);
    exige(&rodar_comando(&alloc::format!("mkdir {}", lote)), "mkdir: criado");
    for i in 0..14 {
        exige(&rodar_comando(&alloc::format!("grava {}/f{:02}.txt", lote, i)), "grava: criado");
    }
    let quinzeno = alloc::format!("{}/f14.txt", lote);
    exige(
        &rodar_comando(&alloc::format!("grava {}", quinzeno)),
        &alloc::format!("grava: sem espaco (volume ou diretorio cheio): {}", quinzeno),
    );
    exige(&rodar_comando(&alloc::format!("rmdir {}", lote)), "rmdir: diretorio nao vazio");
    for i in 0..14 {
        exige(&rodar_comando(&alloc::format!("apaga {}/f{:02}.txt", lote, i)), "apaga: removido");
    }
    exige(&rodar_comando(&alloc::format!("rmdir {}", lote)), &alloc::format!("rmdir: removido {}", lote));
}

#[test_case]
fn mkdir_e_rmdir_no_prompt_do_ram() {
    let ram = Ram::limpo();
    historia_2("ram", "ola.txt");
    ram.confere();
}

#[test_case]
fn mkdir_e_rmdir_no_prompt_do_disco() {
    let disco = Disco::limpo();
    historia_2("disco", "leiame.txt");
    disco.confere();
}

#[test_case]
fn mkdir_e_rmdir_sem_argumento_mostram_o_uso_e_os_volumes_e_help_lista_os_dois() {
    for comando in ["mkdir", "rmdir"] {
        let tela = rodar_comando(comando);
        exige(&tela, &alloc::format!("uso: {} <caminho>", comando));
        exige(&tela, "volumes: /ram (ok) /disco (ok)");
        // Argumento a mais também é uso.
        exige(&rodar_comando(&alloc::format!("{} /ram/a /ram/b", comando)), "uso:");
    }
    let ajuda = rodar_comando("help");
    exige(&ajuda, "mkdir");
    exige(&ajuda, "rmdir");
    let grava = ajuda.iter().find(|l| l.contains("grava")).expect("help lista grava");
    assert!(grava.contains("/ram") && grava.contains("/disco"), "{}", grava);
    exige(&rodar_comando("mkdir /xyz/a"), "mkdir: volume desconhecido: /xyz/a");
    exige(&rodar_comando("rmdir ram/x"), "rmdir: caminho invalido: ram/x");
    exige(&rodar_comando("echo vivo"), "vivo");
}

#[test_case]
fn rmdir_de_diretorio_aberto_e_busy_ate_fechar_e_ate_o_drop() {
    let ram = Ram::limpo();
    exige(&rodar_comando("mkdir /ram/tmp"), "mkdir: criado");
    let mut tabela = fs::FileTable::new();
    let fd = tabela.open(b"/ram/tmp").unwrap();
    assert_eq!(fs::rmdir(b"/ram/tmp").err(), Some(fs::FsError::Busy));
    exige(&rodar_comando("rmdir /ram/tmp"), "rmdir: diretorio em uso: /ram/tmp");
    tabela.close(fd).unwrap();
    fs::rmdir(b"/ram/tmp").unwrap();
    // O mesmo, soltando a tabela inteira.
    fs::mkdir(b"/ram/tmp").unwrap();
    let mut tabela = fs::FileTable::new();
    tabela.open(b"/ram/tmp").unwrap();
    assert_eq!(fs::rmdir(b"/ram/tmp").err(), Some(fs::FsError::Busy));
    drop(tabela);
    fs::rmdir(b"/ram/tmp").unwrap();
    ram.confere();
}

#[test_case]
fn fs_mkdir_e_rmdir_devolvem_cada_erro_na_ordem_do_contrato() {
    let ram = Ram::limpo();
    assert_eq!(fs::mkdir(b"/ram/docs").err(), Some(fs::FsError::AlreadyExists));
    assert_eq!(fs::mkdir(b"/ram").err(), Some(fs::FsError::AlreadyExists));
    assert_eq!(fs::mkdir(b"/ram/nada/x").err(), Some(fs::FsError::NotFound));
    assert_eq!(fs::mkdir(b"/ram/ola.txt/x").err(), Some(fs::FsError::NotADirectory));
    assert_eq!(fs::mkdir(b"/xyz/a").err(), Some(fs::FsError::UnknownVolume));
    assert_eq!(fs::mkdir(b"ram").err(), Some(fs::FsError::InvalidPath));
    assert_eq!(fs::rmdir(b"/ram").err(), Some(fs::FsError::Busy));
    assert_eq!(fs::rmdir(b"/ram/nada").err(), Some(fs::FsError::NotFound));
    assert_eq!(fs::rmdir(b"/ram/ola.txt").err(), Some(fs::FsError::NotADirectory));
    assert_eq!(fs::rmdir(b"/ram/docs").err(), Some(fs::FsError::NotEmpty));
    assert_eq!(fs::remove(b"/ram/docs").err(), Some(fs::FsError::IsADirectory));
    // Volume indisponível vem antes de tudo, inclusive da raiz.
    com_volume(VolumeId::Disco, VolumeState::Unavailable(Reason::NoDisk), || {
        assert_eq!(fs::rmdir(b"/disco").err(), Some(fs::FsError::Unavailable(Reason::NoDisk)));
        assert_eq!(fs::mkdir(b"/disco/x").err(), Some(fs::FsError::Unavailable(Reason::NoDisk)));
    });
    ram.confere();
}

// ---------------------------------------------------------------------------
// US2: SYS_MKDIR e SYS_RMDIR
// ---------------------------------------------------------------------------

#[test_case]
fn mkdir_e_rmdir_devolvem_cada_erro_do_contrato_na_ordem() {
    let ram = Ram::limpo();
    let existente = b"/ram/ola.txt";
    let diretorio = b"/ram/docs";
    let raiz = b"/ram";
    let disco_raiz = b"/disco";
    let inexistente = b"/ram/nada";
    let pai_ruim = b"/ram/nada/x";
    let no_arquivo = b"/ram/ola.txt/x";
    let sem_barra = b"ram";
    let fundo = b"/ram/a/b/c/d/e/f/g/h/i";
    let volume = b"/xyz/a";
    let novo = b"/ram/novo";
    let elf = programa(
        &[existente, diretorio, raiz, disco_raiz, inexistente, pai_ruim, no_arquivo, sem_barra, fundo, volume, novo],
        |a, p| {
            let l = |i: usize, d: &[u8]| (p[i], d.len() as u64);
            let mut codigo = 10;
            for nr in [SYS_MKDIR, SYS_RMDIR] {
                // Caminho: tamanho, ponteiro, forma.
                a.chamada(nr, [p[0], 0, 0], ERR_INVAL, codigo);
                a.chamada(nr, [p[0], 65, 0], ERR_NAMETOOLONG, codigo + 1);
                a.chamada(nr, [p[0], u64::MAX, 0], ERR_NAMETOOLONG, codigo + 2);
                a.chamada(nr, [0x1000, 5, 0], ERR_FAULT, codigo + 3);
                a.chamada(nr, [0xFFFF_FFFF_FFFF_F000, 8, 0], ERR_FAULT, codigo + 4);
                a.chamada(nr, [USER_REGION_START - 4, 8, 0], ERR_FAULT, codigo + 5);
                a.chamada(nr, [l(7, sem_barra).0, l(7, sem_barra).1, 0], ERR_INVAL, codigo + 6);
                a.chamada(nr, [l(8, fundo).0, l(8, fundo).1, 0], ERR_NAMETOOLONG, codigo + 7);
                // Volume desconhecido.
                a.chamada(nr, [l(9, volume).0, l(9, volume).1, 0], ERR_NODEV, codigo + 8);
                // Diretório pai: inexistente, e arquivo no meio.
                a.chamada(nr, [l(5, pai_ruim).0, l(5, pai_ruim).1, 0], ERR_NOENT, codigo + 9);
                a.chamada(nr, [l(6, no_arquivo).0, l(6, no_arquivo).1, 0], ERR_TYPE, codigo + 10);
                codigo += 20;
            }
            // O último componente de `SYS_MKDIR`: já existe (arquivo, diretório, raiz).
            a.chamada(SYS_MKDIR, [l(0, existente).0, l(0, existente).1, 0], ERR_EXIST, 80);
            a.chamada(SYS_MKDIR, [l(1, diretorio).0, l(1, diretorio).1, 0], ERR_EXIST, 81);
            a.chamada(SYS_MKDIR, [l(2, raiz).0, l(2, raiz).1, 0], ERR_EXIST, 82);
            a.chamada(SYS_MKDIR, [l(3, disco_raiz).0, l(3, disco_raiz).1, 0], ERR_EXIST, 83);
            // O último componente de `SYS_RMDIR`, na ordem do contrato.
            a.chamada(SYS_RMDIR, [l(4, inexistente).0, l(4, inexistente).1, 0], ERR_NOENT, 84);
            a.chamada(SYS_RMDIR, [l(0, existente).0, l(0, existente).1, 0], ERR_TYPE, 85);
            a.chamada(SYS_RMDIR, [l(2, raiz).0, l(2, raiz).1, 0], ERR_BUSY, 86);
            a.chamada(SYS_RMDIR, [l(3, disco_raiz).0, l(3, disco_raiz).1, 0], ERR_BUSY, 87);
            a.chamada(SYS_RMDIR, [l(1, diretorio).0, l(1, diretorio).1, 0], ERR_NOTEMPTY, 88);
            // `SYS_REMOVE` de diretório continua `ERR_TYPE`.
            a.chamada(SYS_REMOVE, [l(1, diretorio).0, l(1, diretorio).1, 0], ERR_TYPE, 89);
            // Os sucessos.
            a.chamada(SYS_MKDIR, [l(10, novo).0, l(10, novo).1, 0], 0, 90);
            a.chamada(SYS_MKDIR, [l(10, novo).0, l(10, novo).1, 0], ERR_EXIST, 91);
            a.chamada(SYS_RMDIR, [l(10, novo).0, l(10, novo).1, 0], 0, 92);
            a.chamada(SYS_RMDIR, [l(10, novo).0, l(10, novo).1, 0], ERR_NOENT, 93);
        },
    );
    sai_bem("mkdir_erros", &elf);
    // Nenhuma tentativa que falhou deixou rastro.
    ram.confere();
}

#[test_case]
fn mkdir_em_diretorio_cheio_e_nospc_e_nao_grava() {
    let ram = Ram::limpo();
    fs::mkdir(b"/ram/lote").unwrap();
    for i in 0..14 {
        OpenFile::create(alloc::format!("/ram/lote/f{:02}.txt", i).as_bytes()).unwrap();
    }
    let livres = fs::free_clusters(VolumeId::Ram).unwrap();
    let novo = b"/ram/lote/sub";
    let elf = programa(&[novo], |a, p| {
        a.chamada(SYS_MKDIR, [p[0], novo.len() as u64, 0], ERR_NOSPC, 10);
    });
    sai_bem("mkdir_nospc", &elf);
    assert_eq!(fs::free_clusters(VolumeId::Ram).unwrap(), livres, "o cluster nao pode ter sido tomado");
    for i in 0..14 {
        fs::remove(alloc::format!("/ram/lote/f{:02}.txt", i).as_bytes()).unwrap();
    }
    fs::rmdir(b"/ram/lote").unwrap();
    ram.confere();
}

#[test_case]
fn mkdir_com_4_arquivos_abertos_funciona_porque_nao_usa_descritor() {
    let ram = Ram::limpo();
    let ola = b"/ram/ola.txt";
    let novo = b"/ram/novo";
    let elf = programa(&[ola, novo], |a, p| {
        for fd in 0..4u64 {
            a.chamada(SYS_OPEN, [p[0], ola.len() as u64, 0], fd as i64, 10 + fd as u32);
        }
        // A tabela está cheia: criar um arquivo daria `ERR_MFILE`...
        a.chamada(SYS_CREATE, [p[1], novo.len() as u64, 0], ERR_MFILE, 20);
        // ...mas `mkdir` e `rmdir` não usam descritor.
        a.chamada(SYS_MKDIR, [p[1], novo.len() as u64, 0], 0, 21);
        a.chamada(SYS_RMDIR, [p[1], novo.len() as u64, 0], 0, 22);
    });
    sai_bem("mkdir_sem_descritor", &elf);
    ram.confere();
}

#[test_case]
fn rmdir_de_diretorio_aberto_e_busy_ate_close_exit_e_falha() {
    let ram = Ram::limpo();
    fs::mkdir(b"/ram/tmp").unwrap();
    let tmp = b"/ram/tmp";
    // Fechar libera.
    let fecha = programa(&[tmp], |a, p| {
        a.chamada(SYS_OPEN, [p[0], tmp.len() as u64, 0], 0, 10);
        a.chamada(SYS_RMDIR, [p[0], tmp.len() as u64, 0], ERR_BUSY, 11);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 12);
        a.chamada(SYS_RMDIR, [p[0], tmp.len() as u64, 0], 0, 13);
        a.chamada(SYS_RMDIR, [p[0], tmp.len() as u64, 0], ERR_NOENT, 14);
    });
    sai_bem("rmdir_busy", &fecha);
    // Sair sem fechar libera.
    fs::mkdir(b"/ram/tmp").unwrap();
    let sai = programa(&[tmp], |a, p| {
        a.chamada(SYS_OPEN, [p[0], tmp.len() as u64, 0], 0, 10);
    });
    sai_bem("rmdir_sai", &sai);
    fs::rmdir(b"/ram/tmp").unwrap();
    // Falhar (#PF) com o diretório aberto libera.
    fs::mkdir(b"/ram/tmp").unwrap();
    let cai = programa(&[tmp], |a, p| {
        a.chamada(SYS_OPEN, [p[0], tmp.len() as u64, 0], 0, 10);
        a.movabs(Rax, 0);
        a.store(Rax, Rax); // mov [0], rax: #PF
    });
    let t = rodar_sintetico("rmdir_cai", &cai);
    assert!(matches!(t, Termination::Fault { mnemonic: "#PF", .. }), "{:?}", t);
    assert_eq!(fs::open_files_in_use(), 0);
    fs::rmdir(b"/ram/tmp").unwrap();
    // Aberto por OUTRO programa em execução: `ERR_BUSY`.
    fs::mkdir(b"/ram/tmp").unwrap();
    let segura = programa(&[tmp], |a, p| {
        a.chamada(SYS_OPEN, [p[0], tmp.len() as u64, 0], 0, 10);
        for code in 11..14 {
            a.chamada(SYS_YIELD, [0, 0, 0], 0, code);
        }
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 14);
    });
    let tenta = programa(&[tmp], |a, p| {
        a.chamada(SYS_YIELD, [0, 0, 0], 0, 20);
        a.chamada(SYS_RMDIR, [p[0], tmp.len() as u64, 0], ERR_BUSY, 21);
    });
    let relatorio = user::run_images(&[("segura", &segura[..]), ("tenta", &tenta[..])], None).unwrap();
    for f in &relatorio.finished {
        assert_eq!(f.termination, Termination::Exit { code: 0 }, "{:?}", f.name);
    }
    fs::rmdir(b"/ram/tmp").unwrap();
    ram.confere();
}

#[test_case]
fn mkdir_e_rmdir_funcionam_no_disco_por_syscall() {
    let disco = Disco::limpo();
    let docs = b"/disco/docs/notas";
    let topo = b"/disco/tmpdir";
    let elf = programa(&[docs, topo], |a, p| {
        a.chamada(SYS_MKDIR, [p[0], docs.len() as u64, 0], 0, 10);
        a.chamada(SYS_MKDIR, [p[1], topo.len() as u64, 0], 0, 11);
        a.chamada(SYS_MKDIR, [p[1], topo.len() as u64, 0], ERR_EXIST, 12);
        a.chamada(SYS_RMDIR, [p[0], docs.len() as u64, 0], 0, 13);
        a.chamada(SYS_RMDIR, [p[1], topo.len() as u64, 0], 0, 14);
    });
    sai_bem("mkdir_disco", &elf);
    disco.confere();
}

#[test_case]
fn o_numero_24_continua_encerrando_so_o_programa_e_15_e_16_sao_syscalls() {
    let mut a = Asm::new();
    a.mov32(Rax, 24);
    a.syscall();
    assert_eq!(rodar_sintetico("fs_dezessete", &synth_elf(&a.0)), Termination::BadSyscall { number: 24 });
    exige(&rodar_comando("echo vivo"), "vivo");
    // `SYS_MKDIR` e `SYS_RMDIR` com ponteiro ruim devolvem `ERR_FAULT` e o programa segue.
    let elf = programa(&[], |a, _| {
        a.chamada(SYS_MKDIR, [0x1000, 5, 0], ERR_FAULT, 10);
        a.chamada(SYS_RMDIR, [0x1000, 5, 0], ERR_FAULT, 11);
    });
    sai_bem("mkdir_fault", &elf);
}

// ---------------------------------------------------------------------------
// US3: o programa `caderno`
// ---------------------------------------------------------------------------

/// `AAAA-MM-DD HH:MM:SS`: só o formato, nunca a hora exata.
fn linha_valida(linha: &str) -> bool {
    let b = linha.as_bytes();
    b.len() == 19
        && b.iter().enumerate().all(|(i, &c)| match i {
            4 | 7 => c == b'-',
            10 => c == b' ',
            13 | 16 => c == b':',
            _ => c.is_ascii_digit(),
        })
}

#[test_case]
fn caderno_de_ponta_a_ponta() {
    let disco = Disco::limpo();
    let tela = rodar_comando("run caderno");
    exige(&tela, "caderno: 1 linha em /disco/caderno.txt");
    assert_eq!(tela.iter().filter(|l| linha_valida(l)).count(), 1, "{:?}", tela);
    let ls = rodar_comando("ls /disco");
    assert!(ls.iter().any(|l| l.contains("caderno.txt") && l.contains(" 20 ")), "{:?}", ls);
    assert_eq!(conteudo(b"/disco/caderno.txt").len(), 20);
    // A segunda vez acrescenta e mostra as duas linhas.
    let tela = rodar_comando("run caderno");
    exige(&tela, "caderno: 2 linhas em /disco/caderno.txt");
    assert_eq!(tela.iter().filter(|l| linha_valida(l)).count(), 2, "{:?}", tela);
    let texto = conteudo(b"/disco/caderno.txt");
    assert_eq!(texto.len(), 40);
    assert!(texto.ends_with(b"\n"));
    for linha in core::str::from_utf8(&texto).unwrap().lines() {
        assert!(linha_valida(linha), "linha invalida: {:?}", linha);
    }
    // Chegou ao dispositivo, não só ao kernel.
    assert_eq!(ler_do_dispositivo(&["caderno.txt"]).unwrap().len(), 40);
    exige(&rodar_comando("apaga /disco/caderno.txt"), "apaga: removido");
    disco.confere();
}

#[test_case]
fn caderno_sem_disco_termina_com_codigo_1() {
    com_volume(VolumeId::Disco, VolumeState::Unavailable(Reason::NoDisk), || {
        let tela = rodar_comando("run caderno");
        exige(&tela, "caderno: nao gravou /disco/caderno.txt:");
        exige(&tela, "caderno terminou com codigo 1");
    });
    exige(&rodar_comando("echo vivo"), "vivo");
}

#[test_case]
fn caderno_aparece_unico_e_em_ordem_na_lista_de_run() {
    let nomes: Vec<&str> = user::program_names().collect();
    assert_eq!(nomes.iter().filter(|n| **n == "caderno").count(), 1);
    let mut ordenados = nomes.clone();
    ordenados.sort();
    assert_eq!(nomes, ordenados);
}

// ---------------------------------------------------------------------------
// US4: o README
// ---------------------------------------------------------------------------

#[test_case]
fn o_readme_ensina_preservar_o_disco_e_o_pen_drive() {
    for (nome, readme, frase_antiga) in [
        ("README.md", include_str!("../README.md"), "deixa de regenerar o disco"),
        ("README.en.md", include_str!("../README.en.md"), "stops regenerating the disk"),
    ] {
        assert!(readme.contains("cargo bootimage"), "{}", nome);
        // O QEMU à mão: aponta para a cópia e NÃO tem `snapshot`.
        let qemu = readme
            .lines()
            .find(|l| l.starts_with("qemu-system-x86_64 ") && l.contains("index=1") && l.contains("os-rust-disco.img"))
            .unwrap_or_else(|| panic!("{}: falta a linha do QEMU a mao", nome));
        assert!(!qemu.contains("snapshot"), "{}: {}", nome, qemu);
        assert!(qemu.contains("index=0"), "{}", nome);
        // O pen drive.
        let dd = readme
            .lines()
            .find(|l| l.starts_with("sudo dd ") && l.contains("conv=fsync"))
            .unwrap_or_else(|| panic!("{}: falta a linha do dd", nome));
        assert!(dd.contains("bs=4M") && dd.contains("status=progress"), "{}: {}", nome, dd);
        // O texto antigo, que prometia mudar o `build.rs`, não existe mais.
        assert!(!readme.contains(frase_antiga), "{}", nome);
        // O Marco 11 esta concluido (so o 12, de rede, segue planejado).
        let linha_11 = readme.lines().find(|l| l.starts_with("| 11.")).expect("linha do Marco 11");
        assert!(linha_11.ends_with("Concluído |") || linha_11.ends_with("Completed |"), "{}: {}", nome, linha_11);
    }
}

// ---------------------------------------------------------------------------
// US5: versão, contrato e o que continua igual
// ---------------------------------------------------------------------------

#[test_case]
fn a_versao_0_12_0_aparece_no_boot_e_no_sobre() {
    assert_eq!(os_rust::VERSION, "os-rust v0.12.0");
    exige(&rodar_comando("sobre"), "os-rust v0.12.0");
}

const CONTRATO: &str = include_str!("../SYSCALLS.md");

#[test_case]
fn o_contrato_concorda_com_as_constantes_da_versao_8() {
    assert!(CONTRATO.contains("**Versão do contrato**: 8"));
    assert!(CONTRATO.contains("os-rust 0.12.0 (Marco 12)"));
    for (nr, nome) in [(SYS_MKDIR, "SYS_MKDIR"), (SYS_RMDIR, "SYS_RMDIR")] {
        assert!(CONTRATO.contains(&alloc::format!("| {} | `{}` |", nr, nome)), "{}", nome);
    }
    assert_eq!((SYS_MKDIR, SYS_RMDIR), (15, 16));
    assert!(CONTRATO.contains(&alloc::format!("| `ERR_NOTEMPTY` | `{}` |", ERR_NOTEMPTY)));
    assert_eq!(ERR_NOTEMPTY, -16);
    assert!(CONTRATO.contains("**somente leitura**"));
    assert!(CONTRATO.contains("fora de 1 a 23"));
    // O inglês acompanha.
    let en = include_str!("../SYSCALLS.en.md");
    assert!(en.contains("**Contract version**: 8") && en.contains("os-rust 0.12.0"));
    assert!(en.contains("| 15 | `SYS_MKDIR` |") && en.contains("| 16 | `SYS_RMDIR` |"));
    assert!(en.contains("| `ERR_NOTEMPTY` | `-16` |"));
}

/// Um volume gravável de 2 setores por cluster: o único caso que ainda é
/// "somente leitura" para o escritor.
fn volume_de_2_setores_por_cluster() -> fs::Device {
    let mut b = fatimg::Builder::new();
    b.file("leiame.txt", b"oi").unwrap();
    b.dir("docs").unwrap();
    let mut img = b.build().unwrap();
    let total = 2 * 4100 + 67;
    img.resize(total * 512, 0);
    img[13] = 2;
    img[19..21].copy_from_slice(&0u16.to_le_bytes());
    img[32..36].copy_from_slice(&(total as u32).to_le_bytes());
    vazar_gravavel(img)
}

#[test_case]
fn volume_de_2_setores_por_cluster_continua_somente_leitura() {
    let dev = volume_de_2_setores_por_cluster();
    let geo = Fat::mount(dev).unwrap().geometry();
    assert_eq!(geo.sectors_per_cluster, 2);
    let antes = resumo(dev);
    let original = fs::replace_volume(VolumeId::Disco, VolumeState::Ready { dev, geo });
    for comando in ["grava", "anexa", "apaga", "mkdir", "rmdir"] {
        let linha = match comando {
            "mkdir" | "rmdir" => alloc::format!("{} /disco/docs", comando),
            "apaga" => String::from("apaga /disco/leiame.txt"),
            _ => alloc::format!("{} /disco/leiame.txt x", comando),
        };
        let tela = rodar_comando(&linha);
        exige(&tela, &alloc::format!("{}: volume somente leitura: /disco", comando));
    }
    let novo = b"/disco/novo.txt";
    let dir = b"/disco/docs";
    let existente = b"/disco/leiame.txt";
    let elf = programa(&[novo, dir, existente], |a, p| {
        a.chamada(SYS_CREATE, [p[0], novo.len() as u64, 0], ERR_ROFS, 10);
        a.chamada(SYS_OPEN_APPEND, [p[2], existente.len() as u64, 0], ERR_ROFS, 11);
        a.chamada(SYS_REMOVE, [p[2], existente.len() as u64, 0], ERR_ROFS, 12);
        a.chamada(SYS_MKDIR, [p[0], novo.len() as u64, 0], ERR_ROFS, 13);
        a.chamada(SYS_RMDIR, [p[1], dir.len() as u64, 0], ERR_ROFS, 14);
    });
    sai_bem("disco_2_setores", &elf);
    fs::replace_volume(VolumeId::Disco, original);
    assert_eq!(resumo(dev), antes, "o volume somente leitura mudou");
}

#[test_case]
fn disco_ausente_ou_corrompido_da_erro_de_volume_indisponivel_nas_escritas() {
    for estado in [VolumeState::Unavailable(Reason::NoDisk), VolumeState::Unavailable(Reason::BadVolume("assinatura"))] {
        com_volume(VolumeId::Disco, estado, || {
            for comando in ["grava /disco/x.txt a", "anexa /disco/leiame.txt a", "apaga /disco/leiame.txt", "mkdir /disco/x", "rmdir /disco/docs"] {
                let nome = comando.split(' ').next().unwrap();
                exige(&rodar_comando(comando), &alloc::format!("{}: volume indisponivel: /disco", nome));
            }
            let novo = b"/disco/x.txt";
            let elf = programa(&[novo], |a, p| {
                for nr in [SYS_CREATE, SYS_OPEN_APPEND, SYS_REMOVE, SYS_MKDIR, SYS_RMDIR] {
                    a.chamada(nr, [p[0], novo.len() as u64, 0], ERR_NODEV, 10 + nr as u32);
                }
            });
            sai_bem("disco_indisponivel", &elf);
        });
    }
}

#[test_case]
fn labirint_e_visita_continuam_no_disco_e_o_gravador_no_ram() {
    let ls = rodar_comando("ls /disco/bin");
    exige(&ls, "labirint");
    exige(&ls, "visita");
    exige(&rodar_comando("run /disco/bin/visita"), "visita: fui carregado do disco!");
    // `gravador` e `apagador` continuam iguais no `/ram`.
    let ram = Ram::limpo();
    exige(&rodar_comando("run gravador"), "gravador: criei /ram/gravado.txt (680 bytes)");
    exige(&rodar_comando("run gravador"), "gravador: ampliei /ram/gravado.txt (+27 bytes)");
    exige(&rodar_comando("run apagador"), "apagador: removi /ram/gravado.txt");
    ram.confere();
}

#[test_case]
fn cinquenta_ciclos_de_criar_e_apagar_no_disco_nao_vazam_clusters_frames_nem_descritores() {
    let disco = Disco::limpo();
    let ram = Ram::limpo();
    // Um programa que faz o mesmo por syscalls: cria o diretório, um arquivo
    // dentro dele, apaga os dois.
    let dir = b"/disco/ciclo";
    let arq = b"/disco/ciclo/p.txt";
    let elf = programa(&[dir, arq], |a, p| {
        a.chamada(SYS_MKDIR, [p[0], dir.len() as u64, 0], 0, 10);
        a.chamada(SYS_CREATE, [p[1], arq.len() as u64, 0], 0, 11);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 12);
        a.chamada(SYS_REMOVE, [p[1], arq.len() as u64, 0], 0, 13);
        a.chamada(SYS_RMDIR, [p[0], dir.len() as u64, 0], 0, 14);
    });
    // Uma rodada para aquecer as tabelas e a lista de frames reciclados.
    digitar_no_prompt("mkdir /disco/aquece");
    digitar_no_prompt("rmdir /disco/aquece");
    sai_bem("ciclo_aquece", &elf);
    let frames = memory::frames_outstanding();
    for ciclo in 0..50 {
        digitar_no_prompt("mkdir /disco/ciclo");
        digitar_no_prompt("grava /disco/ciclo/a.txt ola");
        digitar_no_prompt("anexa /disco/ciclo/a.txt mais");
        digitar_no_prompt("apaga /disco/ciclo/a.txt");
        digitar_no_prompt("rmdir /disco/ciclo");
        digitar_no_prompt("mkdir /ram/ciclo");
        digitar_no_prompt("grava /ram/ciclo/a.txt ola");
        digitar_no_prompt("apaga /ram/ciclo/a.txt");
        digitar_no_prompt("rmdir /ram/ciclo");
        if ciclo % 5 == 0 {
            sai_bem("ciclo_programa", &elf);
        }
    }
    assert_eq!(fs::free_clusters(VolumeId::Disco).unwrap(), disco.livres, "vazamento de clusters no /disco");
    assert_eq!(fs::free_clusters(VolumeId::Ram).unwrap(), ram.livres, "vazamento de clusters no /ram");
    assert_eq!(memory::frames_outstanding(), frames, "vazamento de frames");
    assert_eq!(fs::open_files_in_use(), 0, "vazamento de descritores");
    assert_eq!(fs::lookup(b"/disco/ciclo").err(), Some(fs::FsError::NotFound));
    ram.confere();
    disco.confere();
}

#[test_case]
fn o_guia_do_programador_contem_literalmente_o_caderno_e_os_dois_programas_novos() {
    let guia = include_str!("../GUIA_DO_PROGRAMADOR.md");
    // Os cabeçalhos de `gravador` e `apagador` mudaram com o Marco 11: o guia acompanha.
    for fonte in [
        include_str!("../programs/src/bin/caderno.rs"),
        include_str!("../programs/src/bin/gravador.rs"),
        include_str!("../programs/src/bin/apagador.rs"),
    ] {
        assert!(guia.contains(fonte.trim_end()), "o guia nao tem o codigo do programa literalmente");
    }
    assert!(guia.contains("mkdir") && guia.contains("rmdir"));
    assert!(guia.contains("Preservando o disco entre boots"));
}

// ---------------------------------------------------------------------------
// Fumaça
// ---------------------------------------------------------------------------

#[test_case]
fn o_disco_esta_disponivel() {
    assert!(fs::volume_status(VolumeId::Disco).is_ok());
    Disco::limpo().confere();
}
