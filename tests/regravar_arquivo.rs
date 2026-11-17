//! Testes de integração do Marco 13: `SYS_OPEN_WRITE` (abrir um arquivo
//! zerando o conteúdo) no `/ram` e no `/disco`. O `/disco` é o disco de teste
//! do QEMU, aberto com `snapshot=on`: as escritas somem ao fim, e a imagem
//! original gerada pelo `build.rs` nunca é alterada. Todo teste que escreve no
//! `/ram` o deixa como estava. Os programas de apoio são ELFs montados à mão
//! com um mini-assembler (`Asm`), como em `tests/escrita_no_ramdisk.rs`.

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
use os_rust::blockdev::{BlockDevice, WritableRam};
use os_rust::fat::Fat;
use os_rust::fs::{OpenFile, Reason, VolumeId, VolumeState};
use os_rust::syscall::{
    ERR_BADF, ERR_BUSY, ERR_EXIST, ERR_FAULT, ERR_INVAL, ERR_IO, ERR_MFILE, ERR_NAMETOOLONG,
    ERR_NODEV, ERR_NOENT, ERR_NOSPC, ERR_ROFS, ERR_TYPE, SYS_CLOSE, SYS_CREATE, SYS_OPEN,
    SYS_OPEN_APPEND, SYS_OPEN_WRITE, SYS_READ, SYS_READ_DIR, SYS_REMOVE, SYS_WRITE_FILE, SYS_YIELD,
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

/// O `/ram` como estava antes de um teste: clusters livres e listagem da raiz.
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

    /// Devolve o `/ram` ao original e confere que tudo voltou ao que era.
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

/// Um buffer gravável do programa: dentro da pilha dele.
const BUFFER: u64 = USER_STACK_BOTTOM + 0x1000;


// ---------------------------------------------------------------------------
// Ajudantes de volume (duplicados de `tests/escrita_no_ramdisk.rs` e
// `tests/escrita_no_disco.rs`)
// ---------------------------------------------------------------------------

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

/// Cria `path` com `n` bytes de `0xAB` (sem deixar o arquivo aberto).
fn arquivo_de(path: &[u8], n: usize) {
    let mut file = OpenFile::create(path).unwrap();
    let dados = alloc::vec![0xABu8; n];
    let mut feito = 0;
    while feito < n {
        feito += file.write(&dados[feito..]).unwrap();
    }
}

/// O tamanho que a listagem do volume mostra para `path`.
fn tamanho_de(path: &[u8]) -> u32 {
    fs::lookup(path).unwrap().1.size
}

// ---------------------------------------------------------------------------
// US4: criar, zerar e os erros do contrato
// ---------------------------------------------------------------------------

#[test_case]
fn open_write_cria_quando_nao_existe_e_as_gravacoes_acrescentam() {
    let ram = Ram::limpo();
    let novo = b"/ram/novo.txt";
    let carga = b"hello";
    let elf = programa(&[novo, carga], |a, p| {
        a.chamada(SYS_OPEN_WRITE, [p[0], novo.len() as u64, 0], 0, 10);
        a.chamada(SYS_WRITE_FILE, [0, p[1], 5], 5, 11);
        a.chamada(SYS_WRITE_FILE, [0, p[1], 5], 5, 12);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 13);
    });
    sai_bem("regrava_cria", &elf);
    assert_eq!(conteudo(novo), b"hellohello");
    ram.confere();
}

#[test_case]
fn open_write_zera_um_arquivo_de_1000_bytes_e_os_dois_clusters_voltam() {
    let ram = Ram::limpo();
    let g = b"/ram/g.bin";
    let carga = b"0123456789";
    arquivo_de(g, 1000);
    let com_arquivo = fs::free_clusters(VolumeId::Ram).unwrap();
    // Só abrir: o arquivo fica com tamanho 0 e os 2 clusters voltam a ficar livres.
    let abre = programa(&[g], |a, p| {
        a.chamada(SYS_OPEN_WRITE, [p[0], g.len() as u64, 0], 0, 10);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 11);
    });
    sai_bem("regrava_zera", &abre);
    assert_eq!(tamanho_de(g), 0);
    assert_eq!(fs::free_clusters(VolumeId::Ram).unwrap(), com_arquivo + 2);
    exige(&rodar_comando("ls /ram"), "g.bin");
    // Gravar 10 bytes deixa o arquivo com 10 bytes.
    let grava = programa(&[g, carga], |a, p| {
        a.chamada(SYS_OPEN_WRITE, [p[0], g.len() as u64, 0], 0, 10);
        a.chamada(SYS_WRITE_FILE, [0, p[1], 10], 10, 11);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 12);
    });
    sai_bem("regrava_grava", &grava);
    assert_eq!(conteudo(g), carga);
    // Regravar de novo nunca acrescenta: parte do zero outra vez.
    sai_bem("regrava_grava", &grava);
    assert_eq!(conteudo(g), carga);
    ram.confere();
}

#[test_case]
fn open_write_de_arquivo_vazio_funciona_sem_liberar_nada() {
    let ram = Ram::limpo();
    let vazio = b"/ram/vazio.txt";
    let livres = fs::free_clusters(VolumeId::Ram).unwrap();
    let elf = programa(&[vazio], |a, p| {
        a.chamada(SYS_OPEN_WRITE, [p[0], vazio.len() as u64, 0], 0, 10);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 11);
    });
    sai_bem("regrava_vazio", &elf);
    assert_eq!(fs::free_clusters(VolumeId::Ram).unwrap(), livres);
    ram.confere();
}

#[test_case]
fn arquivo_aberto_por_qualquer_um_inclusive_o_proprio_programa_e_busy_e_nao_muda() {
    let ram = Ram::limpo();
    let g = b"/ram/g.bin";
    let n = b"/ram/n.bin";
    let h = b"/ram/h.bin";
    arquivo_de(g, 1000);
    let elf = programa(&[g, n, h], |a, p| {
        // Aberto por leitura, para acrescentar, criado e aberto por `open_write`.
        a.chamada(SYS_OPEN, [p[0], g.len() as u64, 0], 0, 10);
        a.chamada(SYS_OPEN_WRITE, [p[0], g.len() as u64, 0], ERR_BUSY, 11);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 12);
        a.chamada(SYS_OPEN_APPEND, [p[0], g.len() as u64, 0], 0, 13);
        a.chamada(SYS_OPEN_WRITE, [p[0], g.len() as u64, 0], ERR_BUSY, 14);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 15);
        a.chamada(SYS_CREATE, [p[1], n.len() as u64, 0], 0, 16);
        a.chamada(SYS_OPEN_WRITE, [p[1], n.len() as u64, 0], ERR_BUSY, 17);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 18);
        a.chamada(SYS_OPEN_WRITE, [p[2], h.len() as u64, 0], 0, 19);
        a.chamada(SYS_OPEN_WRITE, [p[2], h.len() as u64, 0], ERR_BUSY, 20);
    });
    sai_bem("regrava_busy", &elf);
    assert_eq!(tamanho_de(g), 1000, "o arquivo nao mudou");
    ram.confere();
}

#[test_case]
fn arquivo_aberto_por_outro_programa_em_execucao_e_busy_e_nao_muda() {
    let ram = Ram::limpo();
    let g = b"/ram/g.bin";
    arquivo_de(g, 1000);
    let segura = programa(&[g], |a, p| {
        a.chamada(SYS_OPEN, [p[0], g.len() as u64, 0], 0, 10);
        for code in 11..14 {
            a.chamada(SYS_YIELD, [0, 0, 0], 0, code);
        }
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 14);
    });
    let tenta = programa(&[g], |a, p| {
        a.chamada(SYS_YIELD, [0, 0, 0], 0, 20);
        a.chamada(SYS_OPEN_WRITE, [p[0], g.len() as u64, 0], ERR_BUSY, 21);
    });
    let relatorio = user::run_images(&[("segura", &segura[..]), ("tenta", &tenta[..])], None).unwrap();
    for f in &relatorio.finished {
        assert_eq!(f.termination, Termination::Exit { code: 0 }, "{:?}", f.name);
    }
    assert_eq!(tamanho_de(g), 1000);
    ram.confere();
}

#[test_case]
fn diretorio_raiz_pai_inexistente_e_arquivo_no_meio_do_caminho() {
    let ram = Ram::limpo();
    let docs = b"/ram/docs";
    let raiz = b"/ram";
    let pai_ruim = b"/ram/nada/x.txt";
    let no_arquivo = b"/ram/ola.txt/x";
    let elf = programa(&[docs, raiz, pai_ruim, no_arquivo], |a, p| {
        a.chamada(SYS_OPEN_WRITE, [p[0], docs.len() as u64, 0], ERR_TYPE, 10);
        a.chamada(SYS_OPEN_WRITE, [p[1], raiz.len() as u64, 0], ERR_TYPE, 11);
        a.chamada(SYS_OPEN_WRITE, [p[2], pai_ruim.len() as u64, 0], ERR_NOENT, 12);
        a.chamada(SYS_OPEN_WRITE, [p[3], no_arquivo.len() as u64, 0], ERR_TYPE, 13);
    });
    sai_bem("regrava_tipos", &elf);
    ram.confere();
}

#[test_case]
fn cada_erro_de_caminho_na_ordem_do_contrato() {
    let ram = Ram::limpo();
    let existente = b"/ram/ola.txt";
    let sem_barra = b"ram";
    let fundo = b"/ram/a/b/c/d/e/f/g/h/i";
    let volume = b"/xyz/a";
    let disco_fundo = b"/disco/nada/x.txt";
    let elf = programa(&[existente, sem_barra, fundo, volume, disco_fundo], |a, p| {
        // Caminho: tamanho, ponteiro, forma. Nenhum dos dois primeiros olha o ponteiro.
        a.chamada(SYS_OPEN_WRITE, [p[0], 0, 0], ERR_INVAL, 10);
        a.chamada(SYS_OPEN_WRITE, [p[0], 65, 0], ERR_NAMETOOLONG, 11);
        a.chamada(SYS_OPEN_WRITE, [p[0], u64::MAX, 0], ERR_NAMETOOLONG, 12);
        a.chamada(SYS_OPEN_WRITE, [0x1000, 5, 0], ERR_FAULT, 13);
        a.chamada(SYS_OPEN_WRITE, [0xFFFF_FFFF_FFFF_F000, 8, 0], ERR_FAULT, 14);
        a.chamada(SYS_OPEN_WRITE, [USER_REGION_START - 4, 8, 0], ERR_FAULT, 15);
        a.chamada(SYS_OPEN_WRITE, [p[1], sem_barra.len() as u64, 0], ERR_INVAL, 16);
        a.chamada(SYS_OPEN_WRITE, [p[2], fundo.len() as u64, 0], ERR_NAMETOOLONG, 17);
        a.chamada(SYS_OPEN_WRITE, [p[3], volume.len() as u64, 0], ERR_NODEV, 18);
        a.chamada(SYS_OPEN_WRITE, [p[4], disco_fundo.len() as u64, 0], ERR_NOENT, 19);
    });
    sai_bem("regrava_erros", &elf);
    assert_eq!(tamanho_de(existente), 72, "ola.txt nao mudou");
    ram.confere();
}

#[test_case]
fn tabela_cheia_e_mfile_e_o_arquivo_nao_e_esvaziado() {
    let ram = Ram::limpo();
    let g = b"/ram/g.bin";
    let novo = b"/ram/novo.txt";
    let ola = b"/ram/ola.txt";
    arquivo_de(g, 1000);
    let elf = programa(&[g, novo, ola], |a, p| {
        for fd in 0..4 {
            a.chamada(SYS_OPEN, [p[2], ola.len() as u64, 0], fd, 10 + fd as u32);
        }
        a.chamada(SYS_OPEN_WRITE, [p[0], g.len() as u64, 0], ERR_MFILE, 20);
        a.chamada(SYS_OPEN_WRITE, [p[1], novo.len() as u64, 0], ERR_MFILE, 21);
    });
    sai_bem("regrava_mfile", &elf);
    assert_eq!(tamanho_de(g), 1000, "ERR_MFILE nunca esvazia o arquivo");
    assert_eq!(fs::lookup(novo).err(), Some(fs::FsError::NotFound), "nem cria");
    ram.confere();
}

#[test_case]
fn diretorio_sem_posicao_livre_e_nospc_ao_criar() {
    let ram = Ram::limpo();
    let mut criados = 0;
    loop {
        let nome = alloc::format!("/ram/docs/n{:02}.txt", criados);
        match OpenFile::create(nome.as_bytes()) {
            Ok(_) => criados += 1,
            Err(e) => {
                assert_eq!(e, fs::FsError::NoSpace);
                break;
            }
        }
    }
    let novo = b"/ram/docs/zz.txt";
    let elf = programa(&[novo], |a, p| {
        a.chamada(SYS_OPEN_WRITE, [p[0], novo.len() as u64, 0], ERR_NOSPC, 10);
    });
    sai_bem("regrava_nospc", &elf);
    ram.confere();
}

#[test_case]
fn volume_somente_leitura_e_indisponivel_sao_reportados_antes_de_escrever() {
    let existente = b"/disco/leiame.txt";
    let novo = b"/disco/novo.txt";
    let elf = programa(&[existente, novo], |a, p| {
        a.chamada(SYS_OPEN_WRITE, [p[0], existente.len() as u64, 0], ERR_ROFS, 10);
        a.chamada(SYS_OPEN_WRITE, [p[1], novo.len() as u64, 0], ERR_ROFS, 11);
    });
    let dev = volume_de_2_setores_por_cluster();
    let geo = Fat::mount(dev).unwrap().geometry();
    let antes = resumo(dev);
    com_volume(VolumeId::Disco, VolumeState::Ready { dev, geo }, || {
        sai_bem("regrava_rofs", &elf);
    });
    assert_eq!(resumo(dev), antes, "nada foi escrito");
    let indisponivel = programa(&[novo], |a, p| {
        a.chamada(SYS_OPEN_WRITE, [p[0], novo.len() as u64, 0], ERR_NODEV, 10);
    });
    com_volume(VolumeId::Disco, VolumeState::Unavailable(Reason::NoDisk), || {
        sai_bem("regrava_nodev", &indisponivel);
    });
}

#[test_case]
fn cadeia_adulterada_e_erro_de_leitura_sem_escrever_nada() {
    // Um volume com `a.bin` de 3 clusters, cuja cadeia faz um ciclo.
    let mut b = fatimg::Builder::new();
    b.file("a.bin", &[7u8; 1500]).unwrap();
    let mut img = b.build().unwrap();
    let primeiro = {
        struct Mem<'a>(&'a [u8]);
        impl BlockDevice for Mem<'_> {
            fn sector_count(&self) -> u32 {
                (self.0.len() / 512) as u32
            }
            fn read_sector(&self, lba: u32, buf: &mut [u8; 512]) -> Result<(), os_rust::blockdev::BlockError> {
                let s = lba as usize * 512;
                buf.copy_from_slice(self.0.get(s..s + 512).ok_or(os_rust::blockdev::BlockError::OutOfRange)?);
                Ok(())
            }
        }
        let dev = Mem(&img);
        Fat::mount(&dev).unwrap().lookup(&[*b"A       BIN"]).unwrap().first_cluster
    };
    for copia in 0..2 {
        let at = (1 + copia * 17) * 512 + primeiro as usize * 2;
        img[at..at + 2].copy_from_slice(&primeiro.to_le_bytes());
    }
    let dev = vazar_gravavel(img);
    let original = fs::replace_volume(VolumeId::Ram, fs::mount_state(dev));
    let antes = resumo(dev);
    let nome = b"/ram/a.bin";
    let elf = programa(&[nome], |a, p| {
        a.chamada(SYS_OPEN_WRITE, [p[0], nome.len() as u64, 0], ERR_IO, 10);
    });
    sai_bem("regrava_corrupto", &elf);
    assert_eq!(resumo(dev), antes, "o volume adulterado foi escrito");
    fs::replace_volume(VolumeId::Ram, original);
}

#[test_case]
fn no_disco_cria_zera_e_volta_ao_que_era() {
    let livres = fs::free_clusters(VolumeId::Disco).unwrap();
    let antes = rodar_comando("ls /disco");
    let novo = b"/disco/regrava.txt";
    let primeira = b"abcdef";
    let segunda = b"z";
    let elf = programa(&[novo, primeira, segunda], |a, p| {
        a.chamada(SYS_OPEN_WRITE, [p[0], novo.len() as u64, 0], 0, 10);
        a.chamada(SYS_WRITE_FILE, [0, p[1], 6], 6, 11);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 12);
        // Regravar: o conteúdo antigo some.
        a.chamada(SYS_OPEN_WRITE, [p[0], novo.len() as u64, 0], 0, 13);
        a.chamada(SYS_WRITE_FILE, [0, p[2], 1], 1, 14);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 15);
    });
    sai_bem("regrava_disco", &elf);
    assert_eq!(conteudo(novo), b"z");
    exige(&rodar_comando("apaga /disco/regrava.txt"), "apaga: removido /disco/regrava.txt");
    assert_eq!(fs::free_clusters(VolumeId::Disco).unwrap(), livres);
    assert_eq!(rodar_comando("ls /disco"), antes);
}

#[test_case]
fn remove_e_busy_para_o_descritor_novo_que_aceita_write_file_e_recusa_read() {
    let ram = Ram::limpo();
    let g = b"/ram/g.bin";
    let carga = b"hello";
    arquivo_de(g, 100);
    let elf = programa(&[g, carga], |a, p| {
        a.chamada(SYS_OPEN_WRITE, [p[0], g.len() as u64, 0], 0, 10);
        a.chamada(SYS_REMOVE, [p[0], g.len() as u64, 0], ERR_BUSY, 11);
        a.chamada(SYS_READ, [0, BUFFER, 8], ERR_BADF, 12);
        a.chamada(SYS_WRITE_FILE, [0, p[1], 5], 5, 13);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 14);
        a.chamada(SYS_REMOVE, [p[0], g.len() as u64, 0], 0, 15);
    });
    sai_bem("regrava_remove", &elf);
    assert_eq!(fs::lookup(g).err(), Some(fs::FsError::NotFound));
    ram.confere();
}

#[test_case]
fn o_numero_28_continua_encerrando_so_o_programa() {
    let mut a = Asm::new();
    a.mov32(Rax, 28);
    a.syscall();
    assert_eq!(rodar_sintetico("fs_28", &synth_elf(&a.0)), Termination::BadSyscall { number: 28 });
    exige(&rodar_comando("echo vivo"), "vivo");
}
