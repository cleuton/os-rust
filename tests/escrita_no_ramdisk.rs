//! Testes de integração do Marco 10 (escrita no ramdisk): os comandos `grava`,
//! `anexa` e `apaga`, as quatro syscalls novas (`SYS_CREATE`, `SYS_OPEN_APPEND`,
//! `SYS_WRITE_FILE`, `SYS_REMOVE`), os programas `gravador` e `apagador` e o
//! contrato, de verdade, dentro do QEMU. Cada arquivo de `tests/` é um kernel
//! próprio.
//!
//! O que cada bloco prova:
//! - `gravador` e `apagador` ponta a ponta: tela, conteúdo exato lido do volume,
//!   tamanhos 680 e 707, código 1 do `apagador` sem arquivo, e 100 ciclos sem
//!   vazar clusters, frames nem descritores;
//! - cada erro de cada syscall nova, na ordem do contrato, com ELFs montados à
//!   mão (mini-assembler `Asm`, o mesmo de `sistema_de_arquivos.rs`: cada teste
//!   é um kernel próprio, então o ajudante é repetido);
//! - modos de descritor, o limite de 4 arquivos abertos, `ERR_BUSY` e a
//!   liberação depois de `SYS_CLOSE`, de `exit` e de falha, escrita parcial com
//!   o volume cheio e dois programas acrescentando ao mesmo arquivo;
//! - o `/disco` que não muda, volume indisponível e FAT adulterada;
//! - o contrato (`SYSCALLS.md`) e o guia (`GUIA_DO_PROGRAMADOR.md`).
//!
//! Todo teste que escreve no `/ram` o deixa como estava (`fs::reset_ramdisk()`
//! e conferência de clusters livres e listagem). Nenhum altera a imagem
//! original gerada pelo `build.rs`.

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
    SYS_OPEN_APPEND, SYS_READ, SYS_READ_DIR, SYS_REMOVE, SYS_WRITE_FILE, SYS_YIELD,
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
// US3: gravador e apagador
// ---------------------------------------------------------------------------

/// As 20 linhas que o `gravador` escreve, 34 bytes cada.
fn linhas_do_gravador() -> Vec<u8> {
    let mut dados = Vec::new();
    for n in 1..=20 {
        dados.extend_from_slice(alloc::format!("linha {:02}: gravada por um programa\n", n).as_bytes());
    }
    dados
}

const GRAVADO: &[u8] = b"/ram/gravado.txt";
const MAIS_UMA: &[u8] = b"mais uma linha do gravador\n";

#[test_case]
fn gravador_e_apagador_de_ponta_a_ponta() {
    let ram = Ram::limpo();
    let esperado = linhas_do_gravador();
    assert_eq!(esperado.len(), 680);

    // Cria: 680 bytes, 2 clusters.
    let tela = rodar_comando("run gravador");
    exige(&tela, "gravador: criei /ram/gravado.txt (680 bytes)");
    assert_eq!(conteudo(GRAVADO), esperado);
    assert_eq!(fs::free_clusters(VolumeId::Ram).unwrap(), ram.livres - 2);
    let tela = rodar_comando("ls /ram");
    exige(&tela, &alloc::format!("{:>8}  gravado.txt", 680));
    let tela = rodar_comando("cat /ram/gravado.txt");
    exige(&tela, "linha 01: gravada por um programa");
    exige(&tela, "linha 20: gravada por um programa");

    // Amplia: acrescenta ao fim, sem tocar no que já estava lá.
    let tela = rodar_comando("run gravador");
    exige(&tela, "gravador: ampliei /ram/gravado.txt (+27 bytes)");
    let mut esperado2 = esperado.clone();
    esperado2.extend_from_slice(MAIS_UMA);
    assert_eq!(esperado2.len(), 707);
    assert_eq!(conteudo(GRAVADO), esperado2);
    exige(&rodar_comando("ls /ram"), &alloc::format!("{:>8}  gravado.txt", 707));

    // Outro programa lê o que um programa gravou, fecha e apaga.
    let tela = rodar_comando("run apagador");
    exige(&tela, "mais uma linha do gravador");
    exige(&tela, "linha 20: gravada por um programa");
    exige(&tela, "apagador: removi /ram/gravado.txt");
    assert_eq!(fs::lookup(GRAVADO).err(), Some(fs::FsError::NotFound));
    assert_eq!(fs::free_clusters(VolumeId::Ram).unwrap(), ram.livres);

    // Sem o arquivo: mensagem e código 1.
    let tela = rodar_comando("run apagador");
    exige(&tela, "apagador: nao ha /ram/gravado.txt (rode gravador antes)");
    exige(&tela, "[run] apagador terminou com codigo 1");
    exige(&rodar_comando("echo oi"), "oi");
    ram.confere();
}

#[test_case]
fn cem_ciclos_de_gravador_e_apagador_nao_vazam_clusters_frames_nem_descritores() {
    let ram = Ram::limpo();
    // Uma rodada para aquecer as tabelas e a lista de frames reciclados.
    digitar_no_prompt("run gravador");
    digitar_no_prompt("run apagador");
    let frames = memory::frames_outstanding();
    for _ in 0..100 {
        digitar_no_prompt("run gravador");
        digitar_no_prompt("run apagador");
    }
    assert_eq!(fs::free_clusters(VolumeId::Ram).unwrap(), ram.livres, "vazamento de clusters");
    assert_eq!(memory::frames_outstanding(), frames, "vazamento de frames");
    assert_eq!(fs::open_files_in_use(), 0, "vazamento de descritores");
    assert_eq!(fs::lookup(GRAVADO).err(), Some(fs::FsError::NotFound));
    ram.confere();
}

#[test_case]
fn gravador_e_apagador_aparecem_unicos_e_em_ordem_na_lista_de_run() {
    let nomes: Vec<&str> = user::program_names().collect();
    assert_eq!(nomes.iter().filter(|n| **n == "gravador").count(), 1);
    assert_eq!(nomes.iter().filter(|n| **n == "apagador").count(), 1);
    let mut ordenados = nomes.clone();
    ordenados.sort();
    assert_eq!(nomes, ordenados);
}

// ---------------------------------------------------------------------------
// US3: syscalls de escrita
// ---------------------------------------------------------------------------

#[test_case]
fn create_open_append_e_remove_devolvem_cada_erro_do_contrato_na_ordem() {
    let ram = Ram::limpo();
    let existente = b"/ram/ola.txt";
    let diretorio = b"/ram/docs";
    let raiz = b"/ram";
    let inexistente = b"/ram/nada.txt";
    let pai_ruim = b"/ram/nada/x.txt";
    let no_arquivo = b"/ram/ola.txt/x";
    let sem_barra = b"ram";
    let fundo = b"/ram/a/b/c/d/e/f/g/h/i";
    let volume = b"/xyz/a";
    let disco = b"/disco/x.txt";
    let disco_fundo = b"/disco/nada/x.txt";
    let elf = programa(
        &[existente, diretorio, raiz, inexistente, pai_ruim, no_arquivo, sem_barra, fundo, volume, disco, disco_fundo],
        |a, p| {
            let tam = [12u64, 9, 4, 13, 15, 14, 3, 22, 6, 12, 17];
            let mut codigo = 10;
            for nr in [SYS_CREATE, SYS_OPEN_APPEND, SYS_REMOVE] {
                // Caminho: tamanho, ponteiro, forma.
                a.chamada(nr, [p[0], 0, 0], ERR_INVAL, codigo);
                a.chamada(nr, [p[0], 65, 0], ERR_NAMETOOLONG, codigo + 1);
                a.chamada(nr, [p[0], u64::MAX, 0], ERR_NAMETOOLONG, codigo + 2);
                a.chamada(nr, [0x1000, 5, 0], ERR_FAULT, codigo + 3);
                a.chamada(nr, [0xFFFF_FFFF_FFFF_F000, 8, 0], ERR_FAULT, codigo + 4);
                a.chamada(nr, [USER_REGION_START - 4, 8, 0], ERR_FAULT, codigo + 5);
                a.chamada(nr, [p[6], tam[6], 0], ERR_INVAL, codigo + 6);
                a.chamada(nr, [p[7], tam[7], 0], ERR_NAMETOOLONG, codigo + 7);
                // Volume desconhecido. O `/disco` grava desde o Marco 11, então
                // só o caminho dele importa: pai inexistente é `ERR_NOENT`
                // (a recusa por `ERR_ROFS` é provada em `escrita_no_disco.rs`).
                a.chamada(nr, [p[8], tam[8], 0], ERR_NODEV, codigo + 8);
                a.chamada(nr, [p[10], tam[10], 0], ERR_NOENT, codigo + 10);
                // Diretório pai.
                a.chamada(nr, [p[4], tam[4], 0], ERR_NOENT, codigo + 11);
                a.chamada(nr, [p[5], tam[5], 0], ERR_TYPE, codigo + 12);
                codigo += 20;
            }
            // O último componente, syscall a syscall.
            a.chamada(SYS_CREATE, [p[0], tam[0], 0], ERR_EXIST, 80);
            a.chamada(SYS_CREATE, [p[1], tam[1], 0], ERR_EXIST, 81);
            a.chamada(SYS_CREATE, [p[2], tam[2], 0], ERR_EXIST, 82);
            a.chamada(SYS_OPEN_APPEND, [p[3], tam[3], 0], ERR_NOENT, 83);
            a.chamada(SYS_OPEN_APPEND, [p[1], tam[1], 0], ERR_TYPE, 84);
            a.chamada(SYS_OPEN_APPEND, [p[2], tam[2], 0], ERR_TYPE, 85);
            a.chamada(SYS_REMOVE, [p[3], tam[3], 0], ERR_NOENT, 86);
            a.chamada(SYS_REMOVE, [p[1], tam[1], 0], ERR_TYPE, 87);
            a.chamada(SYS_REMOVE, [p[2], tam[2], 0], ERR_TYPE, 88);
        },
    );
    sai_bem("escrita_erros", &elf);
    // Nenhuma tentativa que falhou deixou rastro.
    ram.confere();
}

#[test_case]
fn create_em_diretorio_sem_posicao_livre_e_no_space() {
    let ram = Ram::limpo();
    // `docs` tem uma posição por entrada do setor que já possui.
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
    assert!(criados > 0);
    let novo = b"/ram/docs/zz.txt";
    let elf = programa(&[novo], |a, p| {
        a.chamada(SYS_CREATE, [p[0], novo.len() as u64, 0], ERR_NOSPC, 10);
    });
    sai_bem("escrita_nospc", &elf);
    ram.confere();
}

#[test_case]
fn descritores_de_acrescentar_leitura_e_diretorio_respeitam_o_modo() {
    let ram = Ram::limpo();
    let m = b"/ram/m.txt";
    let n = b"/ram/n.txt";
    let ola = b"/ram/ola.txt";
    let docs = b"/ram/docs";
    let carga = b"hello";
    let elf = programa(&[m, n, ola, docs, carga], |a, p| {
        let w = |a: &mut Asm, fd: u64, ptr: u64, len: u64, esperado: i64, code: u32| {
            a.chamada(SYS_WRITE_FILE, [fd, ptr, len], esperado, code)
        };
        a.chamada(SYS_CREATE, [p[0], m.len() as u64, 0], 0, 10); // fd 0: acrescentar
        // len == 0 não olha ptr nem fd; len > 4096 é inválido.
        w(a, 0, p[4], 0, 0, 11);
        w(a, 99, 0x1000, 0, 0, 12);
        w(a, 0, p[4], 4097, ERR_INVAL, 13);
        // Ponteiro ruim: nada é gravado.
        w(a, 0, 0x1000, 5, ERR_FAULT, 14);
        w(a, 0, 0xFFFF_FFFF_FFFF_F000, 8, ERR_FAULT, 15);
        w(a, 0, USER_REGION_START - 4, 8, ERR_FAULT, 16);
        a.chamada(SYS_OPEN, [p[2], ola.len() as u64, 0], 1, 17); // fd 1: leitura
        a.chamada(SYS_OPEN, [p[3], docs.len() as u64, 0], 2, 18); // fd 2: diretório
        w(a, 1, p[4], 5, ERR_BADF, 19);
        w(a, 2, p[4], 5, ERR_BADF, 20);
        w(a, 3, p[4], 5, ERR_BADF, 21); // nunca aberto
        w(a, 4, p[4], 5, ERR_BADF, 22); // fora da tabela
        w(a, u64::MAX, p[4], 5, ERR_BADF, 23);
        // Ler de um descritor de acrescentar.
        a.chamada(SYS_READ, [0, BUFFER, 8], ERR_BADF, 24);
        a.chamada(SYS_READ_DIR, [0, BUFFER, 20], ERR_TYPE, 25);
        w(a, 0, p[4], 5, 5, 26); // a escrita boa
        w(a, 0, p[4], 5, 5, 27);
        // 4 abertos (0, 1, 2 e mais um): criar não deixa arquivo.
        a.chamada(SYS_OPEN, [p[2], ola.len() as u64, 0], 3, 28);
        a.chamada(SYS_CREATE, [p[1], n.len() as u64, 0], ERR_MFILE, 29);
        a.chamada(SYS_OPEN_APPEND, [p[0], m.len() as u64, 0], ERR_MFILE, 30);
        // Fechado, o descritor deixa de valer para escrever.
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 31);
        w(a, 0, p[4], 5, ERR_BADF, 32);
    });
    sai_bem("escrita_modos", &elf);
    assert_eq!(conteudo(b"/ram/m.txt"), b"hellohello");
    assert_eq!(fs::lookup(b"/ram/n.txt").err(), Some(fs::FsError::NotFound));
    ram.confere();
}

#[test_case]
fn write_file_de_4096_bytes_atravessa_varios_clusters() {
    let ram = Ram::limpo();
    let carga: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let m = b"/ram/grande.bin";
    let elf = programa(&[m, &carga], |a, p| {
        a.chamada(SYS_CREATE, [p[0], m.len() as u64, 0], 0, 10);
        a.chamada(SYS_WRITE_FILE, [0, p[1], 4096], 4096, 11);
        a.chamada(SYS_WRITE_FILE, [0, p[1], 100], 100, 12);
    });
    sai_bem("escrita_4096", &elf);
    let mut esperado = carga.clone();
    esperado.extend_from_slice(&carga[..100]);
    assert_eq!(conteudo(m), esperado);
    assert_eq!(fs::free_clusters(VolumeId::Ram).unwrap(), ram.livres - 9);
    ram.confere();
}

#[test_case]
fn numero_24_continua_encerrando_so_o_programa() {
    // O contrato v8 vai até `SYS_TCP_TIMEOUT` (23); o 24 é o primeiro número livre.
    let mut a = Asm::new();
    a.mov32(Rax, 24);
    a.syscall();
    assert_eq!(rodar_sintetico("fs_dezessete", &synth_elf(&a.0)), Termination::BadSyscall { number: 24 });
    exige(&rodar_comando("echo vivo"), "vivo");
}

#[test_case]
fn remove_de_arquivo_aberto_e_busy_ate_fechar() {
    let ram = Ram::limpo();
    let ola = b"/ram/ola.txt";
    let vazio = b"/ram/vazio.txt";
    let elf = programa(&[ola, vazio], |a, p| {
        a.chamada(SYS_OPEN, [p[0], ola.len() as u64, 0], 0, 10);
        a.chamada(SYS_REMOVE, [p[0], ola.len() as u64, 0], ERR_BUSY, 11);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 12);
        // Aberto só para acrescentar também conta.
        a.chamada(SYS_OPEN_APPEND, [p[1], vazio.len() as u64, 0], 0, 13);
        a.chamada(SYS_REMOVE, [p[1], vazio.len() as u64, 0], ERR_BUSY, 14);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 15);
        a.chamada(SYS_REMOVE, [p[1], vazio.len() as u64, 0], 0, 16);
        a.chamada(SYS_REMOVE, [p[0], ola.len() as u64, 0], 0, 17);
        a.chamada(SYS_REMOVE, [p[0], ola.len() as u64, 0], ERR_NOENT, 18);
    });
    sai_bem("escrita_busy", &elf);
    // Nada é protegido: os dois arquivos da imagem foram apagados, e o
    // reinício do `/ram` os traz de volta.
    assert_eq!(fs::lookup(ola).err(), Some(fs::FsError::NotFound));
    ram.confere();
    assert!(fs::lookup(ola).is_ok());
}

#[test_case]
fn arquivo_aberto_por_outro_programa_em_execucao_e_busy() {
    let ram = Ram::limpo();
    let ola = b"/ram/ola.txt";
    let segura = programa(&[ola], |a, p| {
        a.chamada(SYS_OPEN, [p[0], ola.len() as u64, 0], 0, 10);
        for code in 11..14 {
            a.chamada(SYS_YIELD, [0, 0, 0], 0, code);
        }
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 14);
    });
    let tenta = programa(&[ola], |a, p| {
        a.chamada(SYS_YIELD, [0, 0, 0], 0, 20);
        a.chamada(SYS_REMOVE, [p[0], ola.len() as u64, 0], ERR_BUSY, 21);
    });
    let relatorio = user::run_images(&[("segura", &segura[..]), ("tenta", &tenta[..])], None).unwrap();
    for f in &relatorio.finished {
        assert_eq!(f.termination, Termination::Exit { code: 0 }, "{:?}", f.name);
    }
    assert_eq!(fs::open_files_in_use(), 0);
    // Os dois terminaram: agora ninguém segura o arquivo.
    fs::remove(ola).unwrap();
    ram.confere();
}

#[test_case]
fn arquivo_volta_a_ser_removivel_depois_de_exit_e_de_falha() {
    let ram = Ram::limpo();
    let ola = b"/ram/ola.txt";
    // Termina com `exit(0)` sem fechar.
    let sai = programa(&[ola], |a, p| {
        a.chamada(SYS_OPEN, [p[0], ola.len() as u64, 0], 0, 10);
        a.chamada(SYS_OPEN_APPEND, [p[0], ola.len() as u64, 0], 1, 11);
    });
    sai_bem("escrita_sai", &sai);
    fs::remove(ola).unwrap();
    fs::reset_ramdisk();
    // Termina com erro de memória (#PF), com um arquivo aberto.
    let cai = programa(&[ola], |a, p| {
        a.chamada(SYS_OPEN, [p[0], ola.len() as u64, 0], 0, 10);
        a.movabs(Rax, 0);
        a.store(Rax, Rax); // mov [0], rax: #PF
    });
    let t = rodar_sintetico("escrita_cai", &cai);
    assert!(matches!(t, Termination::Fault { mnemonic: "#PF", .. }), "{:?}", t);
    assert_eq!(fs::open_files_in_use(), 0);
    fs::remove(ola).unwrap();
    // Termina com `#UD` (instrução inválida).
    fs::reset_ramdisk();
    let mut ud = Asm::new();
    ud.chamada(SYS_OPEN, [USER_REGION_START + 5, ola.len() as u64, 0], 0, 10);
    let mut codigo = Vec::new();
    codigo.extend_from_slice(&[0xE9]);
    codigo.extend_from_slice(&(ola.len() as u32).to_le_bytes());
    codigo.extend_from_slice(ola);
    codigo.extend_from_slice(&ud.0);
    codigo.extend_from_slice(&[0x0F, 0x0B]); // ud2
    let t = rodar_sintetico("escrita_ud", &synth_elf(&codigo));
    assert!(matches!(t, Termination::Fault { mnemonic: "#UD", .. }), "{:?}", t);
    assert_eq!(fs::open_files_in_use(), 0);
    fs::remove(ola).unwrap();
    ram.confere();
}

#[test_case]
fn escrita_parcial_com_o_volume_cheio_e_erro_na_chamada_seguinte() {
    let ram = Ram::limpo();
    // Deixa 3 clusters livres.
    let mut enche = OpenFile::create(b"/ram/enche.bin").unwrap();
    let bloco = [b'x'; 512];
    while fs::free_clusters(VolumeId::Ram).unwrap() > 3 {
        enche.write(&bloco).unwrap();
    }
    let carga: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let p = b"/ram/p.bin";
    let elf = programa(&[p, &carga], |a, d| {
        a.chamada(SYS_CREATE, [d[0], p.len() as u64, 0], 0, 10);
        // Cabem 3 clusters (1536 bytes) dos 4096 pedidos.
        a.chamada(SYS_WRITE_FILE, [0, d[1], 4096], 1536, 11);
        // A próxima não consegue gravar nenhum byte: devolve o erro.
        a.chamada(SYS_WRITE_FILE, [0, d[1], 10], ERR_NOSPC, 12);
        a.chamada(SYS_WRITE_FILE, [0, d[1], 10], ERR_NOSPC, 13);
    });
    sai_bem("escrita_cheio", &elf);
    assert_eq!(conteudo(p), carga[..1536]);
    // O que já estava no volume continua legível, e apagar libera espaço.
    assert_eq!(conteudo(b"/ram/ola.txt"), include_bytes!("../discos/ram/ola.txt"));
    fs::remove(b"/ram/enche.bin").unwrap();
    assert!(fs::free_clusters(VolumeId::Ram).unwrap() > 3);
    ram.confere();
}

/// Um programa que acrescenta `letra` ao arquivo `caminho`, um byte por vez,
/// cedendo a CPU entre as escritas.
fn escritor(caminho: &'static [u8], letra: &'static [u8], vezes: usize) -> Vec<u8> {
    programa(&[caminho, letra], |a, p| {
        a.chamada(SYS_OPEN_APPEND, [p[0], caminho.len() as u64, 0], 0, 10);
        for _ in 0..vezes {
            a.chamada(SYS_WRITE_FILE, [0, p[1], 1], 1, 11);
            a.chamada(SYS_YIELD, [0, 0, 0], 0, 12);
        }
    })
}

#[test_case]
fn dois_programas_acrescentando_ao_mesmo_arquivo_nao_perdem_byte() {
    let ram = Ram::limpo();
    let arquivo = b"/ram/dois.txt";
    OpenFile::create(arquivo).unwrap();
    let a = escritor(arquivo, b"a", 40);
    let b = escritor(arquivo, b"b", 40);
    let relatorio = user::run_images(&[("esc_a", &a[..]), ("esc_b", &b[..])], None).unwrap();
    for f in &relatorio.finished {
        assert_eq!(f.termination, Termination::Exit { code: 0 }, "{:?}", f.name);
    }
    let dados = conteudo(arquivo);
    assert_eq!(dados.len(), 80);
    assert_eq!(dados.iter().filter(|&&c| c == b'a').count(), 40);
    assert_eq!(dados.iter().filter(|&&c| c == b'b').count(), 40);
    ram.confere();
}

// ---------------------------------------------------------------------------
// US4: o /disco não muda, volume indisponível, FAT adulterada
// ---------------------------------------------------------------------------

/// Tudo que se lê do `/disco`: a listagem da raiz e o conteúdo dos arquivos.
fn retrato_do_disco() -> (Vec<String>, Vec<u8>, Vec<u8>) {
    (
        rodar_comando("ls /disco"),
        conteudo(b"/disco/leiame.txt"),
        conteudo(b"/disco/docs/longo.txt"),
    )
}

#[test_case]
fn o_disco_aceita_comandos_e_syscalls_de_escrita_e_volta_ao_que_era() {
    // Desde o Marco 11 o `/disco` grava; o QEMU dos testes usa `snapshot=on` e o
    // teste apaga o que cria. A recusa por `ERR_ROFS` é provada, no
    // `tests/escrita_no_disco.rs`, com um volume de 2 setores por cluster.
    let antes = retrato_do_disco();
    let livres = fs::free_clusters(VolumeId::Disco).unwrap();
    exige(&rodar_comando("grava /disco/x.txt a"), "grava: criado /disco/x.txt (2 bytes)");
    exige(&rodar_comando("anexa /disco/x.txt b"), "anexa: /disco/x.txt agora tem 4 bytes");
    exige(&rodar_comando("apaga /disco/x.txt"), "apaga: removido /disco/x.txt");
    exige(&rodar_comando("echo oi"), "oi");
    let novo = b"/disco/x.txt";
    let existente = b"/disco/leiame.txt";
    let elf = programa(&[novo, existente], |a, p| {
        a.chamada(SYS_CREATE, [p[0], novo.len() as u64, 0], 0, 10);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 11);
        a.chamada(SYS_CREATE, [p[1], existente.len() as u64, 0], ERR_EXIST, 12);
        a.chamada(SYS_OPEN_APPEND, [p[1], existente.len() as u64, 0], 0, 13);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 14);
        a.chamada(SYS_REMOVE, [p[0], novo.len() as u64, 0], 0, 15);
        a.chamada(SYS_OPEN_APPEND, [p[0], novo.len() as u64, 0], ERR_NOENT, 16);
        a.chamada(SYS_REMOVE, [p[0], novo.len() as u64, 0], ERR_NOENT, 17);
    });
    sai_bem("disco_rw", &elf);
    assert_eq!(retrato_do_disco(), antes, "o /disco mudou");
    assert_eq!(fs::free_clusters(VolumeId::Disco).unwrap(), livres);
}

/// Roda `f` com o volume dado no lugar de `vol` e restaura o original.
fn com_volume(vol: VolumeId, estado: VolumeState, f: impl FnOnce()) {
    let original = fs::replace_volume(vol, estado);
    f();
    fs::replace_volume(vol, original);
}

#[test_case]
fn volume_indisponivel_e_reportado_antes_de_qualquer_outro_erro_de_escrita() {
    com_volume(VolumeId::Disco, VolumeState::Unavailable(Reason::NoDisk), || {
        let novo = b"/disco/x.txt";
        let elf = programa(&[novo], |a, p| {
            a.chamada(SYS_CREATE, [p[0], novo.len() as u64, 0], ERR_NODEV, 10);
            a.chamada(SYS_OPEN_APPEND, [p[0], novo.len() as u64, 0], ERR_NODEV, 11);
            a.chamada(SYS_REMOVE, [p[0], novo.len() as u64, 0], ERR_NODEV, 12);
        });
        sai_bem("disco_indisponivel", &elf);
        exige(&rodar_comando("grava /disco/x.txt a"), "grava: volume indisponivel: /disco");
    });
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

#[test_case]
fn fat_adulterada_no_ram_vira_erro_de_leitura_e_nao_escreve_nada() {
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

    // Acrescentar e apagar: erro de leitura, sem pânico nem laço infinito.
    let mut f = OpenFile::open_append(b"/ram/a.bin").unwrap();
    assert_eq!(f.write(b"abc").err(), Some(fs::FsError::Io));
    assert_eq!(fs::remove(b"/ram/a.bin").err(), Some(fs::FsError::Io));
    exige(&rodar_comando("anexa /ram/a.bin x"), "anexa: erro de leitura");
    exige(&rodar_comando("apaga /ram/a.bin"), "apaga: erro de leitura");
    let nome = b"/ram/a.bin";
    let elf = programa(&[nome, b"abc"], |a, p| {
        a.chamada(SYS_OPEN_APPEND, [p[0], nome.len() as u64, 0], 0, 10);
        a.chamada(SYS_WRITE_FILE, [0, p[1], 3], ERR_IO, 11);
        a.chamada(SYS_CLOSE, [0, 0, 0], 0, 12);
        a.chamada(SYS_REMOVE, [p[0], nome.len() as u64, 0], ERR_IO, 13);
    });
    sai_bem("fat_adulterada", &elf);
    assert_eq!(resumo(dev), antes, "o volume adulterado foi escrito");
    fs::replace_volume(VolumeId::Ram, original);
}

#[test_case]
fn mensagens_de_erro_do_prompt_e_o_prompt_segue_respondendo() {
    let ram = Ram::limpo();
    exige(&rodar_comando("grava /ram/ola.txt x"), "grava: ja existe: /ram/ola.txt");
    exige(&rodar_comando("grava /ram x"), "grava: ja existe: /ram");
    exige(&rodar_comando("grava /xyz/a x"), "grava: volume desconhecido: /xyz/a");
    exige(&rodar_comando("apaga /xyz/a"), "volumes: /ram (ok)");
    exige(&rodar_comando("anexa /ram x"), "anexa: e um diretorio: /ram");
    exige(&rodar_comando("apaga /ram"), "apaga: e um diretorio: /ram");
    exige(&rodar_comando("apaga /ram/docs"), "apaga: e um diretorio: /ram/docs");
    exige(&rodar_comando("apaga /ram/nada.txt"), "apaga: nao encontrado: /ram/nada.txt");
    exige(&rodar_comando("grava /ram/ola.txt/x y"), "grava: nao e um diretorio: /ram/ola.txt/x");
    exige(&rodar_comando("grava ram/x y"), "grava: caminho invalido: ram/x");
    // Arquivo em uso por um programa (aqui, uma tabela segurando o arquivo).
    let mut tabela = fs::FileTable::new();
    let fd = tabela.open(b"/ram/ola.txt").unwrap();
    exige(&rodar_comando("apaga /ram/ola.txt"), "apaga: arquivo em uso: /ram/ola.txt");
    exige(&rodar_comando("echo oi"), "oi");
    tabela.close(fd).unwrap();
    // Os três comandos aparecem no help, citando o /ram.
    let tela = rodar_comando("help");
    for nome in ["grava -", "anexa -", "apaga -"] {
        exige(&tela, nome);
    }
    ram.confere();
}

#[test_case]
fn a_imagem_embutida_nunca_e_alterada_e_reiniciar_restaura_o_ram() {
    // A imagem original, como o `build.rs` a gerou.
    let original: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ramdisk.img"));
    let ram = Ram::limpo();
    digitar_no_prompt("grava /ram/lixo.txt sujeira");
    digitar_no_prompt("apaga /ram/ola.txt");
    digitar_no_prompt("run gravador");
    // Ler o volume gravável agora difere da imagem...
    let sujo = fs::lookup(b"/ram/lixo.txt").is_ok() && fs::lookup(b"/ram/ola.txt").is_err();
    assert!(sujo);
    // ...e o reinício restaura o conteúdo, setor por setor.
    fs::reset_ramdisk();
    assert!(fs::lookup(b"/ram/ola.txt").is_ok());
    assert!(fs::lookup(b"/ram/lixo.txt").is_err());
    assert!(fs::lookup(GRAVADO).is_err());
    assert_eq!(conteudo(b"/ram/ola.txt"), include_bytes!("../discos/ram/ola.txt"));
    assert_eq!(original.len() / 512, 4200);
    ram.confere();
}

// ---------------------------------------------------------------------------
// Versão, contrato e guia
// ---------------------------------------------------------------------------

#[test_case]
fn a_versao_0_12_0_aparece_no_sobre() {
    assert_eq!(os_rust::VERSION, "os-rust v0.12.0");
    exige(&rodar_comando("sobre"), "os-rust v0.12.0");
}

const CONTRATO: &str = include_str!("../SYSCALLS.md");

#[test_case]
fn o_contrato_concorda_com_as_constantes() {
    assert!(CONTRATO.contains("**Versão do contrato**: 8"));
    assert!(CONTRATO.contains("os-rust 0.12.0 (Marco 12)"));
    for (nr, nome) in [(SYS_CREATE, "SYS_CREATE"), (SYS_OPEN_APPEND, "SYS_OPEN_APPEND"), (SYS_WRITE_FILE, "SYS_WRITE_FILE"), (SYS_REMOVE, "SYS_REMOVE")] {
        assert!(CONTRATO.contains(&alloc::format!("| {} | `{}` |", nr, nome)), "{}", nome);
    }
    assert_eq!((SYS_CREATE, SYS_OPEN_APPEND, SYS_WRITE_FILE, SYS_REMOVE), (11, 12, 13, 14));
    for (valor, nome) in [(ERR_EXIST, "ERR_EXIST"), (ERR_NOSPC, "ERR_NOSPC"), (ERR_ROFS, "ERR_ROFS"), (ERR_BUSY, "ERR_BUSY")] {
        assert!(CONTRATO.contains(&alloc::format!("| `{}` | `{}` |", nome, valor)), "{}", nome);
    }
    assert_eq!((ERR_EXIST, ERR_NOSPC, ERR_ROFS, ERR_BUSY), (-12, -13, -14, -15));
    assert!(CONTRATO.contains("**somente leitura**"));
    assert!(CONTRATO.contains("fora de 1 a 23"));
}

#[test_case]
fn o_guia_do_programador_contem_literalmente_os_dois_programas() {
    let guia = include_str!("../GUIA_DO_PROGRAMADOR.md");
    for fonte in [include_str!("../programs/src/bin/gravador.rs"), include_str!("../programs/src/bin/apagador.rs")] {
        assert!(guia.contains(fonte.trim_end()), "o guia nao tem o codigo do programa literalmente");
    }
}
