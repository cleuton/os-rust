//! Testes do editor `mano` (versão 0.13.1), de ponta a ponta dentro do QEMU. O
//! `mano` roda **de verdade**, iniciado pelo prompt (`run mano`, `exec mano ...`),
//! e os testes "digitam" empurrando scancodes na fila do kernel, uma tecla por
//! vez, só quando o programa está esperando tecla (já terminou de desenhar). As
//! conferências de tela rodam dentro do gancho do escalonador, antes da tecla
//! seguinte, porque o `mano` limpa a tela ao sair.
//!
//! O padrão é o de `tests/teclado_tela.rs`. Os ajudantes são duplicados de
//! propósito: cada arquivo de `tests/` é um kernel próprio e não há módulo comum.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os_rust::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use os_rust::fs::VolumeId;
use os_rust::user::{self, Termination};
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
// Tela, teclado e arquivos
// ---------------------------------------------------------------------------

/// Devolve o `Writer` ao prompt de sempre (cursor na última linha, cor padrão).
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

/// O texto de uma linha da tela, sem os espaços do fim.
fn linha(row: usize) -> String {
    let texto: String = vga_buffer::screen_row_bytes(row)
        .iter()
        .map(|&b| if (0x20..=0x7e).contains(&b) { b as char } else { '?' })
        .collect();
    String::from(texto.trim_end())
}

/// O texto de uma linha da tela **inteira** (as 80 colunas, com os espaços).
fn linha_inteira(row: usize) -> String {
    vga_buffer::screen_row_bytes(row)
        .iter()
        .map(|&b| if (0x20..=0x7e).contains(&b) { b as char } else { '?' })
        .collect()
}

/// O caractere guardado na célula `(linha, coluna)`.
fn celula(row: usize, col: usize) -> u8 {
    vga_buffer::screen_row_bytes(row)[col]
}

/// O byte de cor da célula `(linha, coluna)`.
fn cor(row: usize, col: usize) -> u8 {
    vga_buffer::screen_cell_color(row, col)
}

/// A posição do cursor de hardware `(linha, coluna)`.
fn cursor() -> (usize, usize) {
    vga_buffer::screen_cursor()
}

/// As linhas não vazias da tela, sem os espaços do fim.
fn linhas_da_tela() -> Vec<String> {
    (0..25).map(linha).filter(|l| !l.is_empty()).collect()
}

/// Digita uma linha no shell, byte a byte, como o teclado faria, e aperta Enter.
fn digitar_no_prompt(texto: &str) {
    for byte in texto.bytes() {
        shell::feed(byte);
    }
    shell::feed(b'\n');
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

/// Cria `caminho` (no `/ram`) com `bytes`, apagando antes o que houver lá.
fn cria(caminho: &str, bytes: &[u8]) {
    let _ = os_rust::fs::remove(caminho.as_bytes());
    let mut arquivo = os_rust::fs::OpenFile::create(caminho.as_bytes()).expect("cria o arquivo no /ram");
    for pedaco in bytes.chunks(512) {
        let mut feito = 0;
        while feito < pedaco.len() {
            feito += arquivo.write(&pedaco[feito..]).expect("grava no /ram");
        }
    }
}

/// Apaga `caminho`, se existir.
fn apaga(caminho: &str) {
    let _ = os_rust::fs::remove(caminho.as_bytes());
}

/// O conteúdo de um arquivo.
fn arquivo(caminho: &str) -> Vec<u8> {
    os_rust::fs::read_whole(caminho.as_bytes(), 1 << 20).expect("le o arquivo")
}

/// O arquivo existe?
fn existe(caminho: &str) -> bool {
    os_rust::fs::lookup(caminho.as_bytes()).is_ok()
}

/// Clusters livres do `/ram` (para provar que os arquivos grandes foram apagados).
fn livres() -> u32 {
    os_rust::fs::free_clusters(VolumeId::Ram).expect("clusters livres")
}

// ---------------------------------------------------------------------------
// O roteiro: uma tecla por vez, só quando o `mano` está esperando
// ---------------------------------------------------------------------------

/// Um passo do roteiro: os scancodes de **uma** tecla (no máximo 6; a fila tem
/// 16) e uma conferência que roda **antes** dela, com o programa ocioso.
struct Passo {
    scancodes: Vec<u8>,
    confere: Option<Box<dyn Fn() + Send>>,
}

struct Roteiro {
    passos: Vec<Passo>,
    proximo: usize,
}

static ROTEIRO: Mutex<Roteiro> = Mutex::new(Roteiro { passos: Vec::new(), proximo: 0 });

/// O gancho do escalonador: se o programa espera tecla (já desenhou), roda a
/// conferência do próximo passo e empurra os scancodes dele. Uma conferência que
/// falha dá `panic`, e o teste falha na hora.
fn tick_mano() {
    if scheduler::keyboard_waiters() == 0 {
        return;
    }
    let mut roteiro = ROTEIRO.lock();
    let k = roteiro.proximo;
    if k >= roteiro.passos.len() {
        return;
    }
    roteiro.proximo += 1;
    if let Some(confere) = &roteiro.passos[k].confere {
        confere();
    }
    for &scancode in roteiro.passos[k].scancodes.iter() {
        interrupts::push_scancode(scancode);
    }
}

/// Roda `comando` no prompt com o roteiro dado; devolve a tela no fim (o prompt).
/// Exige que todo o roteiro tenha sido consumido.
fn roda_mano(comando: &str, passos: Vec<Passo>) -> Vec<String> {
    reset_teclado();
    let total = passos.len();
    {
        let mut roteiro = ROTEIRO.lock();
        roteiro.passos = passos;
        roteiro.proximo = 0;
    }
    scheduler::set_poll_hook(Some(tick_mano));
    let tela = rodar_comando(comando);
    scheduler::set_poll_hook(None);
    let consumidos = ROTEIRO.lock().proximo;
    assert_eq!(consumidos, total, "o programa terminou antes do fim do roteiro");
    reset_teclado();
    tela
}

/// O scancode de um caractere e se precisa de Shift (layout US, Scan Code Set 1).
fn codigo(c: char) -> (u8, bool) {
    match c {
        'a'..='z' => (letra(c), false),
        'A'..='Z' => (letra(c.to_ascii_lowercase()), true),
        '1'..='9' => (0x02 + (c as u8 - b'1'), false),
        '0' => (0x0B, false),
        ' ' => (0x39, false),
        '-' => (0x0C, false),
        '_' => (0x0C, true),
        '=' => (0x0D, false),
        '+' => (0x0D, true),
        '[' => (0x1A, false),
        '{' => (0x1A, true),
        ']' => (0x1B, false),
        '}' => (0x1B, true),
        ';' => (0x27, false),
        ':' => (0x27, true),
        '\'' => (0x28, false),
        '"' => (0x28, true),
        '`' => (0x29, false),
        '~' => (0x29, true),
        '\\' => (0x2B, false),
        '|' => (0x2B, true),
        ',' => (0x33, false),
        '<' => (0x33, true),
        '.' => (0x34, false),
        '>' => (0x34, true),
        '/' => (0x35, false),
        '?' => (0x35, true),
        '!' => (0x02, true),
        '@' => (0x03, true),
        '#' => (0x04, true),
        '$' => (0x05, true),
        '%' => (0x06, true),
        '^' => (0x07, true),
        '&' => (0x08, true),
        '*' => (0x09, true),
        '(' => (0x0A, true),
        ')' => (0x0B, true),
        _ => panic!("sem scancode para {:?}", c),
    }
}

fn letra(c: char) -> u8 {
    const FILEIRAS: [(&str, u8); 3] = [("qwertyuiop", 0x10), ("asdfghjkl", 0x1E), ("zxcvbnm", 0x2C)];
    for (letras, base) in FILEIRAS {
        if let Some(posicao) = letras.find(c) {
            return base + posicao as u8;
        }
    }
    panic!("sem scancode para a letra {:?}", c)
}

const SHIFT: u8 = 0x2A;
const CTRL: u8 = 0x1D;

/// Os scancodes de pressionar e soltar `make`, com Shift e Ctrl se pedidos.
fn teclas(make: u8, shift: bool, ctrl: bool) -> Vec<u8> {
    let mut v = Vec::new();
    if ctrl {
        v.push(CTRL);
    }
    if shift {
        v.push(SHIFT);
    }
    v.push(make);
    v.push(make | 0x80);
    if shift {
        v.push(SHIFT | 0x80);
    }
    if ctrl {
        v.push(CTRL | 0x80);
    }
    v
}

fn passo(scancodes: Vec<u8>) -> Passo {
    Passo { scancodes, confere: None }
}

/// Uma tecla de texto (com Shift se precisar).
fn tecla(c: char) -> Passo {
    let (make, shift) = codigo(c);
    passo(teclas(make, shift, false))
}

/// `Ctrl` mais uma tecla.
fn ctrl(c: char) -> Passo {
    let (make, shift) = codigo(c);
    passo(teclas(make, shift, true))
}

/// Um passo por caractere.
fn texto(t: &str) -> Vec<Passo> {
    t.chars().map(tecla).collect()
}

fn enter() -> Passo {
    passo(teclas(0x1C, false, false))
}

fn esc() -> Passo {
    passo(teclas(0x01, false, false))
}

fn backspace() -> Passo {
    passo(teclas(0x0E, false, false))
}

fn tab() -> Passo {
    passo(teclas(0x0F, false, false))
}

/// Uma tecla estendida (setas, Home, End, ...): o prefixo `0xE0` e o código.
fn estendida(make: u8) -> Passo {
    passo(vec![0xE0, make, 0xE0, make | 0x80])
}

fn cima() -> Passo {
    estendida(0x48)
}

fn baixo() -> Passo {
    estendida(0x50)
}

fn esquerda() -> Passo {
    estendida(0x4B)
}

fn direita() -> Passo {
    estendida(0x4D)
}

fn home() -> Passo {
    estendida(0x47)
}

fn end() -> Passo {
    estendida(0x4F)
}

fn page_up() -> Passo {
    estendida(0x49)
}

fn page_down() -> Passo {
    estendida(0x51)
}

fn delete() -> Passo {
    estendida(0x53)
}

fn insert() -> Passo {
    estendida(0x52)
}

/// Um passo que só confere (não digita nada): a conferência roda com o `mano`
/// ocioso e o roteiro segue.
fn veja(confere: fn()) -> Passo {
    Passo { scancodes: Vec::new(), confere: Some(Box::new(confere)) }
}

/// Um passo que confere com uma função qualquer (pode guardar valores).
fn veja_com(confere: impl Fn() + Send + 'static) -> Passo {
    Passo { scancodes: Vec::new(), confere: Some(Box::new(confere)) }
}

/// O cursor de hardware está em `(linha, coluna)`.
fn veja_cursor(row: usize, col: usize) -> Passo {
    veja_com(move || assert_eq!(cursor(), (row, col)))
}

/// A linha da tela, sem os espaços do fim, é exatamente `esperado`.
fn veja_linha(row: usize, esperado: &str) -> Passo {
    let esperado = String::from(esperado);
    veja_com(move || assert_eq!(linha(row), esperado, "linha {}", row))
}

/// A linha da tela contém `trecho`.
fn veja_contem(row: usize, trecho: &str) -> Passo {
    let trecho = String::from(trecho);
    veja_com(move || assert!(linha(row).contains(trecho.as_str()), "linha {}: {:?} nao tem {:?}", row, linha(row), trecho))
}

/// A barra de título mostra (ou não) `Modificado`.
fn veja_modificado(sim: bool) -> Passo {
    veja_com(move || assert_eq!(linha(0).contains("Modificado"), sim, "barra: {:?}", linha(0)))
}

/// Os passos de gravar um texto sem nome: `Ctrl+O`, o nome e `Enter`.
fn grava_como(nome: &str) -> Vec<Passo> {
    let mut passos = vec![ctrl('o')];
    passos.extend(texto(nome));
    passos.push(enter());
    passos
}

/// Concatena pedaços de roteiro.
fn roteiro(pedacos: Vec<Vec<Passo>>) -> Vec<Passo> {
    pedacos.into_iter().flatten().collect()
}

/// `n` vezes o mesmo passo (gerado por `f`).
fn vezes(n: usize, f: fn() -> Passo) -> Vec<Passo> {
    (0..n).map(|_| f()).collect()
}

// ---------------------------------------------------------------------------
// Base: o decodificador entrega as teclas de que o editor depende (T004)
// ---------------------------------------------------------------------------

#[test_case]
fn ctrl_com_underscore_barra_e_barra_invertida_chegam_ao_programa() {
    reset_teclado();
    // Ctrl+_ é Shift mais hífen, com Ctrl.
    let mut obtido = Vec::new();
    for &s in &[CTRL, SHIFT, 0x0C, 0x8C, SHIFT | 0x80, CTRL | 0x80] {
        if let Some(c) = keyboard::decode(s) {
            obtido.push(c);
        }
    }
    assert_eq!(obtido, [abi::KEY_CTRL | b'_' as u32]);
    let mut obtido = Vec::new();
    for &s in &[CTRL, 0x35, 0xB5, CTRL | 0x80] {
        if let Some(c) = keyboard::decode(s) {
            obtido.push(c);
        }
    }
    assert_eq!(obtido, [abi::KEY_CTRL | b'/' as u32]);
    let mut obtido = Vec::new();
    for &s in &[CTRL, 0x2B, 0xAB, CTRL | 0x80] {
        if let Some(c) = keyboard::decode(s) {
            obtido.push(c);
        }
    }
    assert_eq!(obtido, [abi::KEY_CTRL | b'\\' as u32]);
    reset_teclado();
}

// ---------------------------------------------------------------------------
// Conferências de tela que se repetem
// ---------------------------------------------------------------------------

const ATALHOS_1: [(&str, &str); 6] =
    [("^G", "Ajuda"), ("^O", "Gravar"), ("^W", "Buscar"), ("^K", "Cortar"), ("^C", "Posicao"), ("^X", "Sair")];
const ATALHOS_2: [(&str, &str); 6] =
    [("^R", "Ler"), ("^\\", "Subst."), ("^U", "Colar"), ("^_", "Linha"), ("^Y", "Pag ant"), ("^V", "Pag seg")];

/// O que um quadro de atalhos escreve: 13 colunas por atalho, completado até 80.
fn quadro(atalhos: &[(&str, &str)]) -> String {
    let mut texto = String::new();
    for (tecla, rotulo) in atalhos {
        let mut atalho = format!("{} {}", tecla, rotulo);
        while atalho.len() < 13 {
            atalho.push(' ');
        }
        texto.push_str(&atalho);
    }
    while texto.len() < 80 {
        texto.push(' ');
    }
    texto
}

/// Confere as cores de um quadro: a tecla em vídeo reverso (`0x70`) e o resto no corpo.
fn confere_cores_do_quadro(row: usize, atalhos: &[(&str, &str)]) {
    for (i, (tecla, _)) in atalhos.iter().enumerate() {
        for col in i * 13..i * 13 + 13 {
            let esperado = if col < i * 13 + tecla.len() { 0x70 } else { 0x07 };
            assert_eq!(cor(row, col), esperado, "linha {} coluna {}", row, col);
        }
    }
}

/// Os dois quadros de atalhos de sempre (linhas 23 e 24), com as cores.
fn confere_quadros_normais() {
    assert_eq!(linha_inteira(23), quadro(&ATALHOS_1));
    assert_eq!(linha_inteira(24), quadro(&ATALHOS_2));
    confere_cores_do_quadro(23, &ATALHOS_1);
    confere_cores_do_quadro(24, &ATALHOS_2);
}

/// Toda linha de texto (1 a 21) em branco, na cor do corpo.
fn confere_texto_em_branco() {
    for row in 1..=21 {
        assert_eq!(linha(row), "", "linha {}", row);
        for col in 0..80 {
            assert_eq!(cor(row, col), 0x07);
        }
    }
}

/// Toda linha da tela tem 80 células (invariante da tela) e a 24 não rolou nada.
fn confere_que_nada_rolou() {
    for row in 0..25 {
        assert_eq!(vga_buffer::screen_row_bytes(row).len(), 80);
    }
}

// ---------------------------------------------------------------------------
// Fumaça: `run mano` e `Ctrl+X`
// ---------------------------------------------------------------------------

#[test_case]
fn fumaca_run_mano_abre_e_ctrl_x_volta_ao_prompt() {
    restaurar_tela();
    let tela = roda_mano("run mano", vec![ctrl('x')]);
    assert!(linha(24).starts_with("os-rust>"), "{:?}", tela);
    restaurar_tela();
}

// ---------------------------------------------------------------------------
// História 1: criar um texto novo e gravá-lo
// ---------------------------------------------------------------------------

fn confere_abertura_sem_argumento() {
    assert_eq!(linha(0), " mano  Novo buffer");
    for col in 0..80 {
        assert_eq!(cor(0, col), 0x70, "barra de titulo, coluna {}", col);
    }
    confere_texto_em_branco();
    assert_eq!(linha(22), "");
    for col in 0..80 {
        assert_eq!(cor(22, col), 0x0E);
    }
    confere_quadros_normais();
    assert_eq!(cursor(), (1, 0));
}

fn confere_modificado_na_barra() {
    assert_eq!(&linha_inteira(0)[69..79], "Modificado");
    assert_eq!(cursor(), (2, 5));
}

fn confere_pergunta_do_nome_vazia() {
    assert_eq!(linha(22), "Nome do arquivo para gravar:");
    assert_eq!(cursor(), (22, 29));
}

fn confere_gravou_duas_linhas() {
    assert_eq!(linha(22), "Gravadas 2 linhas");
    assert!(linha(0).contains("/ram/a.txt"), "{:?}", linha(0));
    assert!(!linha(0).contains("Modificado"));
}

#[test_case]
fn historia_1_criar_digitar_gravar_e_sair() {
    restaurar_tela();
    apaga("/ram/a.txt");
    let passos = roteiro(vec![
        vec![veja(confere_abertura_sem_argumento)],
        texto("ola"),
        vec![enter()],
        texto("mundo"),
        vec![veja(confere_modificado_na_barra), ctrl('o'), veja(confere_pergunta_do_nome_vazia)],
        texto("/ram/a.txt"),
        vec![enter(), veja(confere_gravou_duas_linhas), ctrl('x')],
    ]);
    roda_mano("run mano", passos);
    assert_eq!(arquivo("/ram/a.txt"), b"ola\nmundo\n");
    exige(&rodar_comando("cat /ram/a.txt"), "mundo");
    apaga("/ram/a.txt");
    restaurar_tela();
}

fn confere_gravou_de_novo() {
    assert_eq!(linha(22), "Gravadas 2 linhas");
}

fn confere_sem_mensagem() {
    assert_eq!(linha(22), "");
}

#[test_case]
fn ctrl_s_com_nome_grava_direto_e_sem_nome_pergunta() {
    restaurar_tela();
    apaga("/ram/s.txt");
    // Sem nome: `Ctrl+S` age como `Ctrl+O`; depois, com nome, grava direto.
    let passos = roteiro(vec![
        texto("a"),
        vec![enter()],
        texto("b"),
        vec![ctrl('s'), veja(confere_pergunta_do_nome_vazia_s)],
        texto("/ram/s.txt"),
        vec![enter(), veja(confere_gravou_duas_linhas_s), ctrl('s'), veja(confere_gravou_de_novo), ctrl('x')],
    ]);
    roda_mano("run mano", passos);
    assert_eq!(arquivo("/ram/s.txt"), b"a\nb\n");
    // Enter com o campo vazio cancela sem mensagem.
    let passos = roteiro(vec![texto("x"), vec![ctrl('o'), enter(), veja(confere_sem_mensagem), ctrl('x'), tecla('n')]]);
    roda_mano("run mano", passos);
    apaga("/ram/s.txt");
    restaurar_tela();
}

fn confere_pergunta_do_nome_vazia_s() {
    assert_eq!(linha(22), "Nome do arquivo para gravar:");
}

fn confere_gravou_duas_linhas_s() {
    assert_eq!(linha(22), "Gravadas 2 linhas");
}

// -- Ctrl+X: as respostas (D15 e D20) ----------------------------------------

fn confere_pergunta_de_gravar() {
    assert_eq!(linha(22), "Gravar o texto alterado? (S sim, N nao, Ctrl+C cancela)");
    // Com uma pergunta aberta, o quadro de cima mostra só o cancelar.
    assert_eq!(linha_inteira(23), quadro(&[("^C", "Cancelar")]));
    assert_eq!(linha(24), "");
}

fn confere_voltou_a_editar_alterado() {
    assert!(linha(0).contains("Modificado"));
    assert_eq!(linha(22), "");
}

#[test_case]
fn ctrl_x_alterado_sem_nome_s_pergunta_o_nome_e_ctrl_c_no_campo_volta_a_editar() {
    restaurar_tela();
    apaga("/ram/x1.txt");
    let passos = roteiro(vec![
        texto("a"),
        vec![ctrl('x'), veja(confere_pergunta_de_gravar), tecla('s'), veja(confere_pergunta_do_nome_vazia_s)],
        // Ctrl+C no campo do nome: volta a editar, sem sair.
        vec![ctrl('c'), veja(confere_voltou_a_editar_alterado)],
        // Agora de novo, e grava.
        vec![ctrl('x'), tecla('S')],
        texto("/ram/x1.txt"),
        vec![enter()],
    ]);
    roda_mano("run mano", passos);
    assert_eq!(arquivo("/ram/x1.txt"), b"a\n");
    apaga("/ram/x1.txt");
    restaurar_tela();
}

#[test_case]
fn ctrl_x_n_sai_sem_gravar_e_ctrl_c_ou_esc_continuam_editando() {
    restaurar_tela();
    apaga("/ram/x2.txt");
    // `n` sai sem gravar: o arquivo nunca e criado.
    let passos = roteiro(vec![texto("a"), vec![ctrl('o')], texto("/ram/x2.txt"), vec![enter()], texto("b"), vec![ctrl('x'), tecla('n')]]);
    roda_mano("run mano", passos);
    // O primeiro `Ctrl+O` gravou `a`; o `b` nao foi gravado.
    assert_eq!(arquivo("/ram/x2.txt"), b"a\n");
    // Ctrl+C e Esc cancelam; outra tecla e ignorada e a pergunta continua.
    let passos = roteiro(vec![
        texto("z"),
        vec![ctrl('x'), tecla('q'), veja(confere_pergunta_de_gravar), ctrl('c'), veja(confere_voltou_a_editar_alterado)],
        vec![ctrl('x'), esc(), veja(confere_voltou_a_editar_alterado)],
        vec![ctrl('x'), tecla('N')],
    ]);
    roda_mano("run mano", passos);
    apaga("/ram/x2.txt");
    restaurar_tela();
}

#[test_case]
fn ctrl_x_com_nome_s_grava_direto_e_se_falha_volta_a_editar() {
    restaurar_tela();
    apaga("/ram/x3.txt");
    // Com nome atual, `S` grava direto, sem perguntar o nome.
    cria("/ram/x3.txt", b"um\n");
    let passos = roteiro(vec![vec![end()], texto("!"), vec![ctrl('x'), tecla('s')]]);
    roda_mano("exec mano /ram/x3.txt", passos);
    assert_eq!(arquivo("/ram/x3.txt"), b"um!\n");
    apaga("/ram/x3.txt");
    // Gravar falha (o diretorio nao existe): mostra o erro e volta a editar.
    let passos = roteiro(vec![
        texto("a"),
        vec![ctrl('x'), tecla('s'), veja(confere_erro_de_gravar_e_modificado), ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/nada/a.txt", passos);
    assert!(!existe("/ram/nada/a.txt"));
    restaurar_tela();
}

fn confere_erro_de_gravar_e_modificado() {
    assert!(linha(22).starts_with("Erro ao gravar: "), "{:?}", linha(22));
    assert!(linha(0).contains("Modificado"));
}

// -- A saída deixa o prompt limpo (cenário 1.7) --------------------------------

fn confere_prompt_limpo_no_pe() {
    for row in 0..24 {
        assert_eq!(linha(row), "", "linha {}", row);
    }
    assert!(linha(24).starts_with("os-rust>"), "{:?}", linha(24));
    assert_eq!(cor(24, 0), 0x0E);
}

#[test_case]
fn a_saida_limpa_a_tela_restaura_a_cor_e_o_prompt_volta_no_pe() {
    restaurar_tela();
    apaga("/ram/p1.txt");
    // Sem alteracoes.
    roda_mano("run mano", vec![ctrl('x')]);
    confere_prompt_limpo_no_pe();
    restaurar_tela();
    // Com `N`.
    roda_mano("run mano", roteiro(vec![texto("a"), vec![ctrl('x'), tecla('n')]]));
    confere_prompt_limpo_no_pe();
    restaurar_tela();
    // Com `S` (grava e sai).
    roda_mano("run mano", roteiro(vec![texto("a"), vec![ctrl('x'), tecla('s')], texto("/ram/p1.txt"), vec![enter()]]));
    confere_prompt_limpo_no_pe();
    // A tela esta como depois de `clear`.
    restaurar_tela();
    rodar_comando("clear");
    for row in 0..24 {
        assert_eq!(linha(row), "");
    }
    apaga("/ram/p1.txt");
    restaurar_tela();
}

// -- Campos de prompt (D19) -------------------------------------------------------

fn confere_campo_de_40() {
    let esperado = format!("Nome do arquivo para gravar: {}", "a".repeat(40));
    assert_eq!(linha(22), esperado);
    assert_eq!(cursor(), (22, 69));
    // Com um campo aberto, os quadros mostram `^C Cancelar` e `Enter Aceitar`.
    assert_eq!(linha_inteira(23), quadro(&[("^C", "Cancelar")]));
    assert_eq!(linha_inteira(24), quadro(&[("Enter", "Aceitar")]));
}

fn confere_campo_ab() {
    assert_eq!(linha(22), "Nome do arquivo para gravar: ab");
}

fn confere_campo_cancelado() {
    assert_eq!(linha(22), "");
    confere_quadros_normais();
    assert!(!linha(0).contains("Modificado"));
}

#[test_case]
fn campos_de_prompt_aceitam_40_caracteres_backspace_e_esc_cancela() {
    restaurar_tela();
    let mut passos = vec![ctrl('o')];
    passos.extend(texto(&"a".repeat(45)));
    passos.push(veja(confere_campo_de_40));
    // `Backspace` apaga o ultimo; recomeca com `ab`.
    passos.extend(vezes(40, backspace));
    passos.extend(texto("abc"));
    passos.push(backspace());
    passos.push(veja(confere_campo_ab));
    passos.push(esc());
    passos.push(veja(confere_campo_cancelado));
    passos.push(ctrl('x'));
    roda_mano("run mano", passos);
    restaurar_tela();
}

// ---------------------------------------------------------------------------
// História 3: mover o cursor e rolar
// ---------------------------------------------------------------------------

#[test_case]
fn setas_home_end_e_os_apelidos_de_ctrl_movem_o_cursor() {
    restaurar_tela();
    cria("/ram/mv.txt", b"abc\nde\n");
    let passos = roteiro(vec![
        vec![veja_cursor(1, 0), direita(), veja_cursor(1, 1), direita(), direita(), veja_cursor(1, 3)],
        // Direita no fim da linha vai ao começo da seguinte; esquerda no começo volta ao fim.
        vec![direita(), veja_cursor(2, 0), esquerda(), veja_cursor(1, 3)],
        // Home, End e os apelidos.
        vec![home(), veja_cursor(1, 0), end(), veja_cursor(1, 3), ctrl('a'), veja_cursor(1, 0), ctrl('e'), veja_cursor(1, 3)],
        vec![ctrl('b'), veja_cursor(1, 2), ctrl('f'), veja_cursor(1, 3)],
        // Ctrl+N e Ctrl+P fazem o mesmo que Down e Up (a coluna 3 é cortada em 2 na linha curta).
        vec![ctrl('n'), veja_cursor(2, 2), ctrl('p'), veja_cursor(1, 3)],
        // Nos extremos nada acontece.
        vec![cima(), veja_cursor(1, 3), home(), esquerda(), veja_cursor(1, 0)],
        vec![baixo(), baixo(), veja_cursor(3, 0), baixo(), direita(), veja_cursor(3, 0)],
        vec![ctrl('x')],
    ]);
    roda_mano("exec mano /ram/mv.txt", passos);
    apaga("/ram/mv.txt");
    restaurar_tela();
}

#[test_case]
fn a_coluna_desejada_e_mantida_nas_linhas_curtas() {
    restaurar_tela();
    cria("/ram/dz.txt", b"01234567890123456789\nabc\nABCDEFGHIJKLMNOPQRST\n");
    let passos = roteiro(vec![
        vezes(10, direita),
        vec![veja_cursor(1, 10)],
        // Cinco `Left` fixam a coluna 5; `Down` passa pela linha curta e chega à terceira na coluna 5.
        vezes(5, esquerda),
        vec![baixo(), veja_cursor(2, 3), baixo(), veja_cursor(3, 5)],
        // `Ctrl+C` não altera a coluna desejada.
        vec![cima(), cima(), veja_cursor(1, 5), baixo(), ctrl('c'), baixo(), veja_cursor(3, 5)],
        // Depois de `End` na primeira linha: coluna 3 na curta e 20 na terceira.
        vec![cima(), cima(), end(), baixo(), veja_cursor(2, 3), baixo(), veja_cursor(3, 20)],
        vec![ctrl('x')],
    ]);
    roda_mano("exec mano /ram/dz.txt", passos);
    apaga("/ram/dz.txt");
    restaurar_tela();
}

/// Um arquivo de `n` linhas `L001`, `L002`, ... terminado em `\n` (então com `n + 1` linhas no editor).
fn arquivo_de_linhas(n: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for i in 1..=n {
        bytes.extend_from_slice(format!("L{:03}\n", i).as_bytes());
    }
    bytes
}

#[test_case]
fn a_janela_anda_uma_linha_por_vez_e_as_paginas_andam_vinte() {
    restaurar_tela();
    cria("/ram/cem.txt", &arquivo_de_linhas(100));
    let passos = roteiro(vec![
        // Vinte `Down`: ainda cabe na janela (linha do texto 21 da tela).
        vezes(20, baixo),
        vec![veja_cursor(21, 0), veja_linha(1, "L001")],
        // O 21º anda a janela uma linha; o cursor fica na linha 21 da janela.
        vec![baixo(), veja_cursor(21, 0), veja_linha(1, "L002"), veja_linha(21, "L022"), veja_contem(0, "/ram/cem.txt")],
        vec![baixo(), veja_cursor(21, 0), veja_linha(1, "L003")],
        // `Ctrl+Home` não existe; voltar ao topo com páginas.
        vezes(2, page_up),
        vec![veja_cursor(1, 0), veja_linha(1, "L001")],
        // Páginas para baixo: 20 linhas por vez, até a última (a linha vazia depois do último `\n`).
        vec![page_down(), veja_cursor(21, 0), page_down(), veja_linha(1, "L021"), page_down(), page_down(), page_down()],
        vec![veja_cursor(21, 0), veja_linha(1, "L081"), veja_linha(21, "")],
        // `Ctrl+Y` sobe 20 linhas sem mudar a janela enquanto o cursor cabe nela.
        vec![ctrl('y'), veja_cursor(1, 0), veja_linha(1, "L081"), ctrl('y'), veja_linha(1, "L061"), ctrl('v'), veja_linha(1, "L061")],
        vec![ctrl('x')],
    ]);
    roda_mano("exec mano /ram/cem.txt", passos);
    apaga("/ram/cem.txt");
    restaurar_tela();
}

/// Confere que a linha 24 e as demais não rolaram: os quadros continuam onde estavam.
fn confere_tela_inteira_intacta() {
    confere_que_nada_rolou();
    assert_eq!(linha_inteira(24), quadro(&ATALHOS_2));
    assert_eq!(linha_inteira(23), quadro(&ATALHOS_1));
}

#[test_case]
fn a_janela_horizontal_anda_o_minimo_e_a_tela_nunca_passa_de_80_bytes() {
    restaurar_tela();
    let linha_longa: String = "0123456789".repeat(20);
    let mut bytes = linha_longa.clone().into_bytes();
    bytes.extend_from_slice(b"\nfim\n");
    cria("/ram/lg.txt", &bytes);
    let l = linha_longa.clone();
    let l2 = linha_longa.clone();
    let l3 = linha_longa.clone();
    let passos = roteiro(vec![
        vec![veja_com(confere_tela_inteira_intacta), veja_linha(1, &linha_longa[..80])],
        // Até a coluna 79 a janela não anda.
        vezes(79, direita),
        vec![veja_cursor(1, 79), veja_com(confere_tela_inteira_intacta), veja_linha(1, &linha_longa[..80])],
        // Mais uma coluna: a janela anda uma coluna.
        vec![direita(), veja_cursor(1, 79), veja_com(move || assert_eq!(linha(1), l[1..81])), veja_com(confere_tela_inteira_intacta)],
        // No fim da linha de 200 colunas.
        vec![end(), veja_cursor(1, 79), veja_com(move || assert_eq!(linha(1), l2[121..200])), veja_com(confere_tela_inteira_intacta)],
        // As outras linhas andam junto (a linha 2, `fim`, fica fora da janela).
        vec![veja_linha(2, "")],
        // De volta ao começo da linha: a janela volta ao começo.
        vec![home(), veja_cursor(1, 0), veja_com(move || assert_eq!(linha(1), l3[..80])), veja_com(confere_tela_inteira_intacta)],
        vec![ctrl('x')],
    ]);
    roda_mano("exec mano /ram/lg.txt", passos);
    apaga("/ram/lg.txt");
    restaurar_tela();
}

#[test_case]
fn ctrl_c_mostra_a_posicao_e_ir_para_a_linha_valida_o_numero() {
    restaurar_tela();
    cria("/ram/pos.txt", b"abc\nde\n");
    let passos = roteiro(vec![
        vec![ctrl('c'), veja_linha(22, "linha 1/3, coluna 1/4")],
        // A mensagem some na tecla seguinte, e a tecla age.
        vec![direita(), veja_linha(22, ""), direita(), ctrl('c'), veja_linha(22, "linha 1/3, coluna 3/4")],
        // Ir para a linha 3: coluna 1.
        vec![ctrl('_'), veja_linha(22, "Ir para a linha:"), tecla('3'), enter(), veja_cursor(3, 0), ctrl('c'), veja_linha(22, "linha 3/3, coluna 1/1")],
        // `Ctrl+/` também; número fora do intervalo e texto não numérico não movem o cursor.
        vec![ctrl('/'), tecla('2'), enter(), veja_cursor(2, 0)],
        vec![ctrl('/'), tecla('9'), enter(), veja_linha(22, "Linha fora do intervalo (1 a 3)"), veja_cursor(2, 0)],
        vec![ctrl('_'), tecla('x'), enter(), veja_linha(22, "Linha fora do intervalo (1 a 3)"), veja_cursor(2, 0)],
        vec![ctrl('_'), tecla('0'), enter(), veja_linha(22, "Linha fora do intervalo (1 a 3)")],
        vec![ctrl('x')],
    ]);
    roda_mano("exec mano /ram/pos.txt", passos);
    // Num texto vazio.
    roda_mano("run mano", vec![ctrl('c'), veja_linha(22, "linha 1/1, coluna 1/1"), ctrl('x')]);
    apaga("/ram/pos.txt");
    restaurar_tela();
}

// ---------------------------------------------------------------------------
// História 4: editar o texto
// ---------------------------------------------------------------------------

#[test_case]
fn enter_parte_backspace_e_delete_juntam_e_ctrl_h_ctrl_d_fazem_o_mesmo() {
    restaurar_tela();
    apaga("/ram/ed.txt");
    let passos = roteiro(vec![
        // No começo do arquivo e no fim, Backspace e Delete não fazem nada (nem marcam Modificado).
        vec![backspace(), delete(), ctrl('h'), ctrl('d'), veja_modificado(false)],
        // Caractere entra na posição do cursor; `Enter` parte a linha sem recuo.
        texto("abcd"),
        vec![esquerda(), esquerda(), enter(), veja_linha(1, "ab"), veja_linha(2, "cd"), veja_cursor(2, 0), veja_modificado(true)],
        // Backspace no começo da linha junta com a anterior, cursor no ponto da junção.
        vec![backspace(), veja_linha(1, "abcd"), veja_linha(2, ""), veja_cursor(1, 2)],
        // Delete apaga sob o cursor; Ctrl+H e Ctrl+D fazem o mesmo que Backspace e Delete.
        vec![delete(), veja_linha(1, "abd"), ctrl('h'), veja_linha(1, "ad"), veja_cursor(1, 1), ctrl('d'), veja_linha(1, "a")],
        // Delete no fim da linha junta com a seguinte.
        vec![enter()],
        texto("z"),
        vec![cima(), veja_cursor(1, 1), delete(), veja_linha(1, "az"), veja_linha(2, ""), veja_cursor(1, 1)],
        grava_como("/ram/ed.txt"),
        vec![ctrl('x')],
    ]);
    roda_mano("run mano", passos);
    assert_eq!(arquivo("/ram/ed.txt"), b"az\n");
    apaga("/ram/ed.txt");
    restaurar_tela();
}

#[test_case]
fn tab_insere_espacos_ate_a_proxima_coluna_multipla_de_4_e_nunca_o_byte_09() {
    restaurar_tela();
    apaga("/ram/tb.txt");
    let passos = roteiro(vec![
        texto("12345"),
        vec![tab(), veja_linha(1, "12345"), veja_cursor(1, 8)],
        vec![enter(), tab(), veja_cursor(2, 4)],
        texto("x"),
        vec![tab(), veja_cursor(2, 8)],
        grava_como("/ram/tb.txt"),
        vec![ctrl('x')],
    ]);
    roda_mano("run mano", passos);
    assert_eq!(arquivo("/ram/tb.txt"), b"12345   \n    x   \n");
    apaga("/ram/tb.txt");
    restaurar_tela();
}

#[test_case]
fn teclas_sem_funcao_nao_mudam_o_texto_nem_marcam_modificado() {
    restaurar_tela();
    cria("/ram/ig.txt", b"x\n");
    let passos = roteiro(vec![
        vec![insert(), esc(), ctrl('i'), ctrl('j'), ctrl('m'), ctrl('z'), ctrl('l'), ctrl('q')],
        vec![veja_modificado(false), veja_cursor(1, 0), veja_linha(1, "x"), veja_linha(22, "")],
        // Com Ctrl, uma seta que não está na tabela também não faz nada de novo; e sair não pergunta nada.
        vec![ctrl('x')],
    ]);
    roda_mano("exec mano /ram/ig.txt", passos);
    assert_eq!(arquivo("/ram/ig.txt"), b"x\n");
    apaga("/ram/ig.txt");
    restaurar_tela();
}

/// Um arquivo de 4096 linhas de 15 bytes, a última sem `\n` (65535 bytes; seria gravado com 65536).
fn arquivo_no_limite_de_bytes() -> Vec<u8> {
    let mut bytes = Vec::new();
    for i in 0..4096 {
        if i > 0 {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(format!("{:04}-abcdefghij", i).as_bytes());
    }
    bytes
}

#[test_case]
fn no_limite_de_bytes_ou_de_linhas_a_digitacao_e_o_enter_sao_recusados_e_nada_muda() {
    restaurar_tela();
    let antes = livres();
    let grande = arquivo_no_limite_de_bytes();
    assert_eq!(grande.len(), 65535);
    cria("/ram/lim1.txt", &grande);
    let mut curtas = Vec::new();
    for _ in 0..4095 {
        curtas.extend_from_slice(b"a\n");
    }
    curtas.push(b'a');
    cria("/ram/lim2.txt", &curtas);
    // 4096 linhas de 15 bytes: um byte a mais passa de 65536, e uma linha a mais passa de 4096.
    let passos = roteiro(vec![
        vec![ctrl('_')],
        texto("4096"),
        vec![enter(), end(), veja_cursor(21, 15)],
        texto("x"),
        vec![veja_linha(22, "Limite de 65536 bytes atingido"), veja_modificado(false), veja_cursor(21, 15)],
        vec![tab(), veja_linha(22, "Limite de 65536 bytes atingido")],
        vec![enter(), veja_linha(22, "Limite de 4096 linhas atingido"), veja_modificado(false), veja_cursor(21, 15)],
        vec![ctrl('x')],
    ]);
    roda_mano("exec mano /ram/lim1.txt", passos);
    // 4096 linhas curtas: o `Enter` passa do limite de linhas; digitar ainda cabe.
    let passos = roteiro(vec![
        vec![enter(), veja_linha(22, "Limite de 4096 linhas atingido"), veja_modificado(false)],
        texto("b"),
        vec![veja_modificado(true), veja_linha(1, "ba")],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/lim2.txt", passos);
    assert_eq!(arquivo("/ram/lim1.txt"), grande);
    apaga("/ram/lim1.txt");
    apaga("/ram/lim2.txt");
    assert_eq!(livres(), antes, "os arquivos grandes foram apagados");
    restaurar_tela();
}

// ---------------------------------------------------------------------------
// História 2: abrir um arquivo, editar e regravar
// ---------------------------------------------------------------------------

#[test_case]
fn abrir_mostra_o_texto_sem_linha_a_mais_e_gravar_regrava_com_o_que_mudou() {
    restaurar_tela();
    cria("/ram/nota.txt", b"um\ndois\ntres\n");
    let passos = roteiro(vec![
        vec![
            veja_contem(0, "/ram/nota.txt"),
            veja_linha(1, "um"),
            veja_linha(2, "dois"),
            veja_linha(3, "tres"),
            veja_linha(4, ""),
            veja_cursor(1, 0),
            veja_modificado(false),
            ctrl('c'),
            veja_linha(22, "linha 1/4, coluna 1/3"),
        ],
        // Cursor no fim de `dois`, digita `!`.
        vec![baixo(), end(), tecla('!'), veja_linha(2, "dois!"), veja_modificado(true), ctrl('s'), veja_modificado(false)],
        vec![ctrl('x')],
    ]);
    roda_mano("exec mano /ram/nota.txt", passos);
    assert_eq!(arquivo("/ram/nota.txt"), b"um\ndois!\ntres\n");
    apaga("/ram/nota.txt");
    restaurar_tela();
}

fn confere_arquivo_novo() {
    assert_eq!(linha(22), "Novo arquivo");
    assert!(linha(0).contains("/ram/novo.txt"));
    assert!(!existe("/ram/novo.txt"));
    assert_eq!(linha(1), "");
}

#[test_case]
fn arquivo_inexistente_abre_vazio_com_o_nome_e_so_passa_a_existir_ao_gravar() {
    restaurar_tela();
    apaga("/ram/novo.txt");
    roda_mano("exec mano /ram/novo.txt", vec![veja(confere_arquivo_novo), ctrl('x')]);
    assert!(!existe("/ram/novo.txt"), "sair sem alterar nao cria o arquivo");
    // Gravar cria o arquivo (Ctrl+S, o nome já é o atual).
    let passos = roteiro(vec![texto("oi"), vec![ctrl('s'), veja_linha(22, "Gravada 1 linha"), ctrl('x')]]);
    roda_mano("exec mano /ram/novo.txt", passos);
    assert_eq!(arquivo("/ram/novo.txt"), b"oi\n");
    apaga("/ram/novo.txt");
    restaurar_tela();
}

#[test_case]
fn os_bytes_do_arquivo_sao_preservados_ao_abrir_e_gravar_sem_editar() {
    restaurar_tela();
    // (conteúdo, quantos `\n` serão gravados) — todos voltam idênticos, menos o que não termina em `\n`.
    let casos: [(&[u8], &[u8]); 9] = [
        (b"", b""),
        (b"\n", b"\n"),
        (b"\n\n", b"\n\n"),
        (b"a\n", b"a\n"),
        (b"a", b"a\n"),
        (b"a\nb", b"a\nb\n"),
        (b"x\ty\r\n", b"x\ty\r\n"),
        (b"\xc3\xa9\0z\n", b"\xc3\xa9\0z\n"),
        (b"\n\n\nfim\n", b"\n\n\nfim\n"),
    ];
    for (entrada, saida) in casos {
        cria("/ram/bt.txt", entrada);
        roda_mano("exec mano /ram/bt.txt", vec![ctrl('s'), ctrl('x')]);
        assert_eq!(arquivo("/ram/bt.txt"), saida, "entrada {:?}", entrada);
    }
    // O arquivo de 0 bytes continua com 0 bytes e a mensagem diz 0 linhas.
    cria("/ram/bt.txt", b"");
    roda_mano("exec mano /ram/bt.txt", vec![ctrl('s'), veja_linha(22, "Gravadas 0 linhas"), ctrl('x')]);
    assert_eq!(arquivo("/ram/bt.txt"), b"");
    apaga("/ram/bt.txt");
    restaurar_tela();
}

fn confere_bytes_especiais_na_tela() {
    // `x \t y \r 0xC3 0xA9 \0 z`: cada byte que o kernel não imprime ocupa uma coluna e vira 0xfe.
    let esperado = [b'x', 0xfe, b'y', 0xfe, 0xfe, 0xfe, 0xfe, b'z'];
    assert_eq!(&vga_buffer::screen_row_bytes(1)[..8], &esperado[..]);
}

#[test_case]
fn bytes_que_o_kernel_nao_imprime_aparecem_como_0xfe_uma_coluna_cada() {
    restaurar_tela();
    cria("/ram/es.txt", b"x\ty\r\xc3\xa9\0z\n");
    roda_mano("exec mano /ram/es.txt", vec![veja(confere_bytes_especiais_na_tela), ctrl('x')]);
    apaga("/ram/es.txt");
    restaurar_tela();
}

fn confere_pergunta_sobrescrever() {
    assert_eq!(linha(22), "Arquivo existe. Sobrescrever? (S sim, N nao)");
    assert_eq!(linha_inteira(23), quadro(&[("^C", "Cancelar")]));
}

#[test_case]
fn gravar_num_nome_que_ja_existe_pergunta_e_so_grava_com_s() {
    restaurar_tela();
    cria("/ram/ex2.txt", b"outro\n");
    let passos = roteiro(vec![
        texto("novo"),
        // `Esc`, `Ctrl+C` e `N` voltam a editar sem gravar; o arquivo fica intacto.
        grava_como("/ram/ex2.txt"),
        vec![veja(confere_pergunta_sobrescrever), esc(), veja_modificado(true), veja_linha(1, "novo")],
        grava_como("/ram/ex2.txt"),
        vec![ctrl('c'), veja_modificado(true)],
        grava_como("/ram/ex2.txt"),
        vec![tecla('n'), veja_modificado(true), veja_com(|| assert_eq!(arquivo("/ram/ex2.txt"), b"outro\n"))],
        // `S` grava.
        grava_como("/ram/ex2.txt"),
        vec![tecla('S'), veja_linha(22, "Gravada 1 linha"), veja_modificado(false), ctrl('x')],
    ]);
    roda_mano("run mano", passos);
    assert_eq!(arquivo("/ram/ex2.txt"), b"novo\n");
    // Um nome de diretório que não existe: erro, o texto fica na tela e `Modificado` continua.
    let passos = roteiro(vec![
        texto("a"),
        grava_como("/ram/nada/a.txt"),
        vec![veja_contem(22, "Erro ao gravar: nao encontrado"), veja_modificado(true), veja_linha(1, "a")],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("run mano", passos);
    apaga("/ram/ex2.txt");
    restaurar_tela();
}

// ---------------------------------------------------------------------------
// História 5: cortar e colar linhas, inserir arquivo
// ---------------------------------------------------------------------------

#[test_case]
fn ctrl_k_corta_a_linha_ctrl_u_cola_acima_e_a_area_nao_esvazia() {
    restaurar_tela();
    cria("/ram/ct.txt", b"um\ndois\ntres\n");
    let passos = roteiro(vec![
        // Cortar `dois`: o cursor vai ao começo de `tres`, que subiu.
        vec![baixo(), ctrl('k')],
        vec![veja_linha(1, "um"), veja_linha(2, "tres"), veja_linha(3, ""), veja_cursor(2, 0), veja_modificado(true)],
        // Colar com o cursor na linha 1: `dois` entra acima de `um`, e o cursor vai ao começo de `um`.
        vec![cima(), ctrl('u')],
        vec![veja_linha(1, "dois"), veja_linha(2, "um"), veja_linha(3, "tres"), veja_cursor(2, 0)],
        // A área não esvazia: colar de novo cola de novo.
        vec![ctrl('u'), veja_linha(1, "dois"), veja_linha(2, "dois"), veja_linha(3, "um"), veja_cursor(3, 0)],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/ct.txt", passos);
    apaga("/ram/ct.txt");
    restaurar_tela();
}

#[test_case]
fn cortes_seguidos_acumulam_e_outra_tecla_comeca_uma_area_nova() {
    restaurar_tela();
    cria("/ram/ac.txt", b"a\nb\nc\nd\n");
    // Três `Ctrl+K` seguidos acumulam as três linhas, na ordem; `Ctrl+U` as cola juntas.
    let passos = roteiro(vec![
        vezes(3, || ctrl('k')),
        vec![veja_linha(1, "d"), veja_cursor(1, 0)],
        vec![ctrl('u'), veja_linha(1, "a"), veja_linha(2, "b"), veja_linha(3, "c"), veja_linha(4, "d"), veja_cursor(4, 0)],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/ac.txt", passos);
    // `Ctrl+K`, uma seta, `Ctrl+K`: a segunda sequência substitui a área.
    cria("/ram/ac.txt", b"a\nb\nc\n");
    let passos = roteiro(vec![
        vec![ctrl('k'), direita(), ctrl('k'), ctrl('u')],
        vec![veja_linha(1, "b"), veja_linha(2, "c"), veja_linha(3, "")],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/ac.txt", passos);
    apaga("/ram/ac.txt");
    restaurar_tela();
}

#[test_case]
fn cortar_a_ultima_linha_uma_linha_so_e_uma_linha_vazia_unica() {
    restaurar_tela();
    // Última linha de um texto de várias linhas: o cursor vai ao começo da nova última.
    cria("/ram/ul.txt", b"a\nb\n");
    let passos = roteiro(vec![
        vec![ctrl('_'), tecla('3'), enter(), ctrl('k')],
        vec![veja_cursor(2, 0), veja_linha(1, "a"), veja_linha(2, "b"), veja_linha(3, ""), veja_modificado(true)],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/ul.txt", passos);
    // Uma linha só, não vazia: o corte a deixa vazia e o texto fica `Modificado`.
    let passos = roteiro(vec![
        texto("abc"),
        vec![ctrl('k'), veja_linha(1, ""), veja_modificado(true), veja_cursor(1, 0)],
        // Numa única linha vazia `Ctrl+K` é ignorado: a área e a sequência de cortes ficam como estavam.
        vec![ctrl('k'), ctrl('u'), veja_linha(1, "abc"), veja_linha(2, "")],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("run mano", passos);
    // Num texto novo, sem nada cortado, `Ctrl+K` não muda nada nem marca `Modificado`; `Ctrl+U` avisa.
    let passos = roteiro(vec![
        vec![ctrl('k'), veja_modificado(false), ctrl('u'), veja_linha(22, "Nada para colar"), veja_modificado(false)],
        vec![ctrl('x')],
    ]);
    roda_mano("run mano", passos);
    apaga("/ram/ul.txt");
    restaurar_tela();
}

#[test_case]
fn o_ctrl_k_ignorado_numa_linha_vazia_nao_encerra_a_sequencia_de_cortes() {
    restaurar_tela();
    // `b`, `a`: dois cortes seguidos esvaziam o texto; um terceiro `Ctrl+K` é ignorado e a área segue com as duas.
    let passos = roteiro(vec![
        texto("a"),
        vec![enter()],
        texto("b"),
        vec![ctrl('k'), ctrl('k'), ctrl('k'), veja_linha(1, ""), ctrl('u')],
        vec![veja_linha(1, "b"), veja_linha(2, "a"), veja_linha(3, "")],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("run mano", passos);
    restaurar_tela();
}

#[test_case]
fn colar_alem_dos_limites_e_recusado_com_a_mensagem_e_nada_muda() {
    restaurar_tela();
    let antes = livres();
    cria("/ram/cl.txt", &arquivo_no_limite_de_bytes());
    let passos = roteiro(vec![
        // Corta uma linha (4095 linhas), cola (volta a 4096, no limite exato) e cola de novo (4097: recusado).
        vec![ctrl('k'), ctrl('u'), veja_com(|| assert_eq!(linha(22), "")), ctrl('u')],
        vec![veja_linha(22, "Limite de 4096 linhas atingido")],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/cl.txt", passos);
    apaga("/ram/cl.txt");
    assert_eq!(livres(), antes);
    restaurar_tela();
}

#[test_case]
fn ctrl_r_insere_o_arquivo_no_cursor_e_o_cursor_fica_no_fim_do_texto_inserido() {
    restaurar_tela();
    cria("/ram/in1.txt", b"X\nY");
    cria("/ram/in2.txt", b"Q\n");
    cria("/ram/in3.txt", b"");
    cria("/ram/in0.txt", b"ab\n");
    let passos = roteiro(vec![
        // `ab` com o cursor depois do `a`; o arquivo `X\nY` (sem `\n` no fim) entra no meio.
        vec![direita(), ctrl('r'), veja_linha(22, "Arquivo a inserir:")],
        texto("/ram/in1.txt"),
        vec![enter(), veja_linha(1, "aX"), veja_linha(2, "Yb"), veja_cursor(2, 1), veja_modificado(true)],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/in0.txt", passos);
    // Arquivo que termina em `\n`: o cursor fica no começo da linha seguinte.
    let passos = roteiro(vec![
        vec![direita(), ctrl('r')],
        texto("/ram/in2.txt"),
        vec![enter(), veja_linha(1, "aQ"), veja_linha(2, "b"), veja_cursor(2, 0)],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/in0.txt", passos);
    // Arquivo de 0 bytes: nada muda, nem `Modificado`.
    let passos = roteiro(vec![vec![ctrl('r')], texto("/ram/in3.txt"), vec![enter(), veja_modificado(false), veja_linha(1, "ab"), ctrl('x')]]);
    roda_mano("exec mano /ram/in0.txt", passos);
    for nome in ["/ram/in0.txt", "/ram/in1.txt", "/ram/in2.txt", "/ram/in3.txt"] {
        apaga(nome);
    }
    restaurar_tela();
}

#[test_case]
fn ctrl_r_com_erro_deixa_o_texto_intacto() {
    restaurar_tela();
    let casos: [(&str, &str); 5] = [
        ("/ram/nope.txt", "Erro ao ler: nao encontrado"),
        ("/ram/docs", "Erro ao ler: tipo errado (arquivo ou diretorio)"),
        ("a.txt", "Use um caminho absoluto, como /ram/nota.txt"),
        ("/ram/nomemuitolongo.txt", "Erro ao ler: caminho invalido"),
        ("/disco/docs/grande.bin", "Limite de 65536 bytes atingido"),
    ];
    for (nome, mensagem) in casos {
        let passos = roteiro(vec![
            texto("ab"),
            vec![ctrl('r')],
            texto(nome),
            vec![enter(), veja_linha(22, mensagem), veja_linha(1, "ab"), veja_cursor(1, 2), ctrl('x'), tecla('n')],
        ]);
        roda_mano("run mano", passos);
    }
    restaurar_tela();
}

#[test_case]
fn regravar_um_arquivo_menor_nunca_acrescenta() {
    restaurar_tela();
    cria("/ram/mn.txt", b"um\ndois\ntres\n");
    let passos = roteiro(vec![vezes(2, baixo), vec![ctrl('k'), ctrl('s'), ctrl('x')]]);
    roda_mano("exec mano /ram/mn.txt", passos);
    assert_eq!(arquivo("/ram/mn.txt"), b"um\ndois\n");
    apaga("/ram/mn.txt");
    restaurar_tela();
}

// ---------------------------------------------------------------------------
// História 6: buscar
// ---------------------------------------------------------------------------

/// As quatro primeiras células da linha `row` em vídeo reverso, e o resto no corpo.
fn confere_reverso(row: usize, de: usize, ate: usize) {
    for col in 0..80 {
        let esperado = if (de..ate).contains(&col) { 0x70 } else { 0x07 };
        assert_eq!(cor(row, col), esperado, "linha {} coluna {}", row, col);
    }
}

#[test_case]
fn a_busca_acha_para_a_frente_repete_com_o_campo_vazio_e_da_a_volta() {
    restaurar_tela();
    cria("/ram/bu.txt", b"um\ndois\ntres\ndois\n");
    let passos = roteiro(vec![
        vec![ctrl('w'), veja_linha(22, "Buscar:")],
        texto("dois"),
        vec![enter(), veja_cursor(2, 0), veja_modificado(false), veja_com(|| confere_reverso(2, 0, 4))],
        // Na tecla seguinte o reverso some, e a tecla também age.
        vec![direita(), veja_cursor(2, 1), veja_com(|| confere_reverso(2, 0, 0))],
        // `Enter` com o campo vazio repete o último termo: a próxima ocorrência é a da linha 4.
        vec![ctrl('w'), veja_linha(22, "Buscar [dois]:"), enter(), veja_cursor(4, 0), veja_com(|| confere_reverso(4, 0, 4))],
        // De novo: volta à linha 2 com o aviso.
        vec![ctrl('w'), enter(), veja_cursor(2, 0), veja_linha(22, "Busca reiniciada")],
        vec![ctrl('x')],
    ]);
    roda_mano("exec mano /ram/bu.txt", passos);
    apaga("/ram/bu.txt");
    restaurar_tela();
}

#[test_case]
fn a_busca_diferencia_maiusculas_avisa_quando_nao_acha_e_cancela_sem_mudar() {
    restaurar_tela();
    cria("/ram/bn.txt", b"um\ndois\n");
    let passos = roteiro(vec![
        vec![ctrl('w')],
        texto("xyz"),
        vec![enter(), veja_linha(22, "\"xyz\" nao encontrado"), veja_cursor(1, 0), veja_modificado(false)],
        vec![ctrl('w')],
        texto("Dois"),
        vec![enter(), veja_linha(22, "\"Dois\" nao encontrado"), veja_cursor(1, 0)],
        // `Ctrl+C` cancela o campo, sem mudança.
        vec![ctrl('w'), ctrl('c'), veja_linha(22, ""), veja_cursor(1, 0)],
        vec![ctrl('x')],
    ]);
    roda_mano("exec mano /ram/bn.txt", passos);
    // Sem termo anterior, `Enter` com o campo vazio cancela sem mensagem.
    let passos = roteiro(vec![vec![ctrl('w'), veja_linha(22, "Buscar:"), enter(), veja_linha(22, ""), veja_modificado(false), ctrl('x')]]);
    roda_mano("exec mano /ram/bn.txt", passos);
    apaga("/ram/bn.txt");
    restaurar_tela();
}

#[test_case]
fn o_unico_trecho_sob_o_cursor_e_achado_de_novo_com_o_aviso_de_volta() {
    restaurar_tela();
    cria("/ram/un.txt", b"so um dois\n");
    let passos = roteiro(vec![
        vec![ctrl('w')],
        texto("dois"),
        vec![enter(), veja_cursor(1, 6), ctrl('w'), enter(), veja_cursor(1, 6), veja_linha(22, "Busca reiniciada")],
        vec![ctrl('x')],
    ]);
    roda_mano("exec mano /ram/un.txt", passos);
    apaga("/ram/un.txt");
    restaurar_tela();
}

// ---------------------------------------------------------------------------
// História 7: buscar e substituir
// ---------------------------------------------------------------------------

const PERGUNTA_TROCAR: &str = "Substituir este? (S sim, N nao, A todos, Ctrl+C cancela)";

/// Os passos de pedir o termo e o texto novo de uma substituição (com o atalho dado).
fn pede_troca(atalho: Passo, termo: &str, novo: &str) -> Vec<Passo> {
    let mut passos = vec![atalho];
    passos.extend(texto(termo));
    passos.push(enter());
    passos.extend(texto(novo));
    passos.push(enter());
    passos
}

#[test_case]
fn substituir_com_s_e_n_troca_so_o_que_foi_confirmado() {
    restaurar_tela();
    cria("/ram/sb.txt", b"gato\ncasa gato\nrato\n");
    let passos = roteiro(vec![
        pede_troca(ctrl('\\'), "gato", "cao"),
        vec![veja_linha(22, PERGUNTA_TROCAR), veja_cursor(1, 0), veja_com(|| confere_reverso(1, 0, 4))],
        vec![tecla('s'), veja_cursor(2, 5), veja_com(|| confere_reverso(2, 5, 9)), tecla('n')],
        vec![
            veja_linha(1, "cao"),
            veja_linha(2, "casa gato"),
            veja_linha(22, "Substituicoes: 1"),
            veja_modificado(true),
            veja_cursor(2, 5),
            veja_com(|| confere_reverso(2, 0, 0)),
        ],
        vec![ctrl('s'), ctrl('x')],
    ]);
    roda_mano("exec mano /ram/sb.txt", passos);
    assert_eq!(arquivo("/ram/sb.txt"), b"cao\ncasa gato\nrato\n");
    apaga("/ram/sb.txt");
    restaurar_tela();
}

#[test_case]
fn substituir_com_a_troca_todas_sem_perguntar_e_o_percurso_comeca_no_inicio_do_arquivo() {
    restaurar_tela();
    cria("/ram/sa.txt", b"gato\ncasa gato\nrato\n");
    // `a` (e `A`) na primeira ocorrência: as duas são trocadas, sem mais perguntas.
    for resposta in ['a', 'A'] {
        let passos = roteiro(vec![
            pede_troca(ctrl('\\'), "gato", "cao"),
            vec![tecla(resposta)],
            vec![veja_linha(1, "cao"), veja_linha(2, "casa cao"), veja_linha(22, "Substituicoes: 2"), veja_cursor(2, 5), veja_modificado(true)],
            vec![ctrl('x'), tecla('n')],
        ]);
        roda_mano("exec mano /ram/sa.txt", passos);
    }
    // Com o cursor no meio do arquivo, a primeira pergunta é sobre a primeira ocorrência do arquivo.
    let passos = roteiro(vec![
        vezes(2, baixo),
        pede_troca(ctrl('\\'), "gato", "cao"),
        vec![veja_cursor(1, 0), esc(), veja_linha(22, "Substituicoes: 0"), veja_modificado(false)],
        vec![ctrl('x')],
    ]);
    roda_mano("exec mano /ram/sa.txt", passos);
    apaga("/ram/sa.txt");
    restaurar_tela();
}

#[test_case]
fn o_texto_novo_nao_e_buscado_de_novo_e_o_novo_vazio_remove() {
    restaurar_tela();
    cria("/ram/aa.txt", b"banana\ngato gato\n");
    // `a` por `aa` termina: cada `a` original vira `aa` uma vez.
    let passos = roteiro(vec![
        pede_troca(ctrl('\\'), "a", "aa"),
        vec![tecla('a'), veja_linha(1, "baanaanaa"), veja_linha(22, "Substituicoes: 5")],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/aa.txt", passos);
    // `a` (todas) por nada: as ocorrências somem.
    let passos = roteiro(vec![
        pede_troca(ctrl('t'), "a", ""),
        vec![tecla('A'), veja_linha(1, "bnn"), veja_linha(2, "gto gto"), veja_linha(22, "Substituicoes: 5")],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/aa.txt", passos);
    apaga("/ram/aa.txt");
    restaurar_tela();
}

#[test_case]
fn ctrl_c_e_esc_param_a_substituicao_o_que_foi_trocado_fica() {
    restaurar_tela();
    cria("/ram/sc.txt", b"gato\ncasa gato\nrato\n");
    for cancela in [ctrl('c'), esc()] {
        let passos = roteiro(vec![
            pede_troca(ctrl('\\'), "gato", "cao"),
            vec![tecla('s'), cancela],
            vec![
                veja_linha(1, "cao"),
                veja_linha(2, "casa gato"),
                veja_linha(22, "Substituicoes: 1"),
                veja_modificado(true),
                // O cursor fica no começo da ocorrência que estava sendo perguntada.
                veja_cursor(2, 5),
            ],
            vec![ctrl('x'), tecla('n')],
        ]);
        roda_mano("exec mano /ram/sc.txt", passos);
    }
    apaga("/ram/sc.txt");
    restaurar_tela();
}

#[test_case]
fn sem_ocorrencia_e_varias_na_mesma_linha_e_o_texto_novo_de_outro_tamanho() {
    restaurar_tela();
    cria("/ram/mu.txt", b"gato gato\n");
    // Nenhuma ocorrência: o cursor não se move e o texto não fica `Modificado`.
    let passos = roteiro(vec![
        vec![direita()],
        pede_troca(ctrl('\\'), "xyz", "w"),
        vec![veja_linha(22, "Nenhuma ocorrencia"), veja_cursor(1, 1), veja_modificado(false), ctrl('x')],
    ]);
    roda_mano("exec mano /ram/mu.txt", passos);
    // Cada ocorrência é perguntada em ordem; a troca da primeira (de outro tamanho) não desloca a pergunta da segunda.
    let passos = roteiro(vec![
        pede_troca(ctrl('\\'), "gato", "cachorro"),
        vec![veja_cursor(1, 0), tecla('s'), veja_cursor(1, 9), veja_linha(1, "cachorro gato"), tecla('n')],
        vec![veja_linha(1, "cachorro gato"), veja_linha(22, "Substituicoes: 1")],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/mu.txt", passos);
    let passos = roteiro(vec![
        pede_troca(ctrl('\\'), "gato", "r"),
        vec![tecla('s'), veja_cursor(1, 2), tecla('s')],
        vec![veja_linha(1, "r r"), veja_linha(22, "Substituicoes: 2")],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/mu.txt", passos);
    apaga("/ram/mu.txt");
    restaurar_tela();
}

#[test_case]
fn uma_troca_que_passa_do_limite_termina_o_percurso_com_a_mensagem_unica() {
    restaurar_tela();
    let antes = livres();
    // Uma linha de 65534 `a` mais `\n`: gravada tem 65535 bytes. A primeira troca (`a` por `bb`) chega a 65536; a segunda estoura.
    let mut bytes = vec![b'a'; 65534];
    bytes.push(b'\n');
    cria("/ram/ls.txt", &bytes);
    let passos = roteiro(vec![
        pede_troca(ctrl('\\'), "a", "bb"),
        vec![tecla('A')],
        vec![veja_linha(22, "Limite de 65536 bytes atingido. Substituicoes: 1"), veja_modificado(true), veja_cursor(1, 2)],
        vec![veja_com(|| assert!(linha(1).starts_with("bbaaaa")))],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/ls.txt", passos);
    apaga("/ram/ls.txt");
    assert_eq!(livres(), antes);
    restaurar_tela();
}

#[test_case]
fn ctrl_t_faz_o_mesmo_e_o_ultimo_termo_da_busca_vale_para_a_substituicao() {
    restaurar_tela();
    cria("/ram/ut.txt", b"gato\nrato\n");
    // A busca guarda o termo; a substituição o sugere entre colchetes e `Enter` vazio o usa.
    let passos = roteiro(vec![
        vec![ctrl('w')],
        texto("gato"),
        vec![enter(), ctrl('t'), veja_linha(22, "Substituir [gato]:"), enter()],
        texto("cao"),
        vec![enter(), tecla('s'), veja_linha(1, "cao"), veja_linha(22, "Substituicoes: 1")],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/ut.txt", passos);
    // Sem último termo, `Enter` com o campo vazio cancela sem mensagem.
    let passos = roteiro(vec![vec![ctrl('t'), veja_linha(22, "Substituir:"), enter(), veja_linha(22, ""), ctrl('x')]]);
    roda_mano("exec mano /ram/ut.txt", passos);
    apaga("/ram/ut.txt");
    restaurar_tela();
}

#[test_case]
fn trocar_um_trecho_por_ele_mesmo_conta_a_troca_mas_nao_marca_modificado() {
    restaurar_tela();
    cria("/ram/ig2.txt", b"gato\nrato\n");
    let passos = roteiro(vec![
        pede_troca(ctrl('\\'), "gato", "gato"),
        vec![tecla('s'), veja_linha(22, "Substituicoes: 1"), veja_modificado(false), ctrl('x')],
    ]);
    roda_mano("exec mano /ram/ig2.txt", passos);
    // Se já estava `Modificado`, continua.
    let passos = roteiro(vec![
        texto("z"),
        pede_troca(ctrl('\\'), "gato", "gato"),
        vec![tecla('s'), veja_linha(22, "Substituicoes: 1"), veja_modificado(true)],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/ig2.txt", passos);
    apaga("/ram/ig2.txt");
    restaurar_tela();
}

// ---------------------------------------------------------------------------
// História 8: ajuda, erros de uso e limites na abertura
// ---------------------------------------------------------------------------

/// A tela inteira (as 25 linhas) e o cursor, para comparar antes e depois da ajuda.
static FOTO: Mutex<(Vec<String>, (usize, usize))> = Mutex::new((Vec::new(), (0, 0)));

fn tira_foto() {
    *FOTO.lock() = ((0..25).map(linha_inteira).collect(), cursor());
}

fn confere_foto() {
    let foto = FOTO.lock();
    let agora: Vec<String> = (0..25).map(linha_inteira).collect();
    assert_eq!(agora, foto.0, "a tela voltou diferente");
    assert_eq!(cursor(), foto.1);
}

/// Cada atalho da tabela de teclas da spec, como a ajuda deve citá-lo.
const ATALHOS_DA_AJUDA: [&str; 20] = [
    "Enter",
    "Backspace, Ctrl+H",
    "Delete, Ctrl+D",
    "Tab",
    "setas, Ctrl+B, Ctrl+F, Ctrl+P, Ctrl+N",
    "Home, Ctrl+A",
    "End, Ctrl+E",
    "PageUp, Ctrl+Y",
    "PageDown, Ctrl+V",
    "Ctrl+_, Ctrl+/",
    "Ctrl+C",
    "Ctrl+K",
    "Ctrl+U",
    "Ctrl+W",
    "Ctrl+\\, Ctrl+T",
    "Ctrl+R",
    "Ctrl+O",
    "Ctrl+S",
    "Ctrl+X",
    "Ctrl+G",
];

fn confere_a_ajuda() {
    assert!(linha(0).contains("Ajuda do mano"), "{:?}", linha(0));
    assert_eq!(cor(0, 5), 0x70);
    assert!(linha(24).contains("Pressione qualquer tecla para voltar"), "{:?}", linha(24));
    for atalho in ATALHOS_DA_AJUDA {
        assert!((1..=21).any(|row| linha(row).contains(atalho)), "a ajuda nao cita {:?}", atalho);
    }
    // Nenhuma linha passa de 80 bytes e a 24 não rolou nada.
    confere_que_nada_rolou();
}

#[test_case]
fn ctrl_g_mostra_a_ajuda_e_qualquer_tecla_volta_exatamente_como_estava() {
    restaurar_tela();
    cria("/ram/aj.txt", b"um\ndois\ntres\n");
    let passos = roteiro(vec![
        // Deixa o texto modificado, com o cursor no meio, e tira uma foto da tela.
        vec![baixo(), direita(), tecla('!'), veja_com(tira_foto)],
        vec![ctrl('g'), veja_com(confere_a_ajuda)],
        // Uma letra qualquer só volta: não é inserida.
        vec![tecla('q'), veja_com(confere_foto), veja_modificado(true), veja_linha(2, "d!ois")],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/aj.txt", passos);
    apaga("/ram/aj.txt");
    restaurar_tela();
}

/// Roda `comando` sem teclas e confere que o programa falou **antes** de usar a tela.
fn erro_de_uso(comando: &str, mensagem: &str) {
    restaurar_tela();
    let tela = roda_mano(comando, Vec::new());
    exige(&tela, mensagem);
    exige(&tela, "mano terminou com codigo 1");
    // A tela de edição nunca apareceu: nem a barra de título, nem os atalhos.
    assert!(!tem(&tela, "^G Ajuda"), "{:?}", tela);
    assert!(!tem(&tela, " mano  Novo buffer"), "{:?}", tela);
    assert_ne!(cor(0, 0), 0x70);
    assert!(linha(24).starts_with("os-rust>"), "{:?}", linha(24));
    restaurar_tela();
}

#[test_case]
fn os_erros_de_uso_saem_antes_de_usar_a_tela_com_codigo_1() {
    erro_de_uso("exec mano a.txt", "mano: use um caminho absoluto, como /ram/nota.txt");
    erro_de_uso("exec mano /ram/a /ram/b", "mano: uso: exec mano [arquivo]");
    erro_de_uso(
        "exec mano /ram/aaaaaaaa/aaaaaaaa/aaaaaaaa/aaaaaaaa/aaaa",
        "mano: caminho muito longo (maximo 40 bytes)",
    );
    erro_de_uso("exec mano /ram/docs", "mano: tipo errado (arquivo ou diretorio)");
    erro_de_uso("exec mano /ram/nomemuitolongo.txt", "mano: caminho invalido");
    erro_de_uso("exec mano /disco/docs/grande.bin", "mano: arquivo grande demais (maximo 65536 bytes e 4096 linhas)");
}

/// Um arquivo `n` vezes `\n`.
fn so_quebras(n: usize) -> Vec<u8> {
    vec![b'\n'; n]
}

#[test_case]
fn os_limites_da_abertura_sao_medidos_pelo_tamanho_gravado() {
    let antes = livres();
    let mensagem = "mano: arquivo grande demais (maximo 65536 bytes e 4096 linhas)";
    // Abrem: 65536 bytes terminado em `\n`; 65535 sem `\n` (gravado: 65536); 4095 `\n` (4096 linhas).
    let mut a = vec![b'a'; 65535];
    a.push(b'\n');
    let casos_que_abrem: [(&str, Vec<u8>); 3] = [("/ram/o1.txt", a), ("/ram/o2.txt", vec![b'a'; 65535]), ("/ram/o3.txt", so_quebras(4095))];
    for (nome, bytes) in casos_que_abrem {
        cria(nome, &bytes);
        restaurar_tela();
        let comando = format!("exec mano {}", nome);
        roda_mano(&comando, vec![veja_contem(0, nome), ctrl('x')]);
        assert_eq!(arquivo(nome), bytes, "{}: abrir e sair nao altera", nome);
        apaga(nome);
    }
    // São recusados: 65536 sem `\n` (gravado: 65537); 4096 `\n` (4097 linhas).
    let casos_recusados: [(&str, Vec<u8>); 2] = [("/ram/r1.txt", vec![b'a'; 65536]), ("/ram/r2.txt", so_quebras(4096))];
    for (nome, bytes) in casos_recusados {
        cria(nome, &bytes);
        restaurar_tela();
        let tela = roda_mano(&format!("exec mano {}", nome), Vec::new());
        exige(&tela, mensagem);
        exige(&tela, "mano terminou com codigo 1");
        // Nenhum arquivo foi alterado pela falha da abertura.
        assert_eq!(arquivo(nome), bytes);
        apaga(nome);
    }
    assert_eq!(livres(), antes, "os arquivos grandes foram apagados");
    // A falha de abertura não cria arquivo.
    assert!(!existe("/ram/nao_ha.txt"));
    restaurar_tela();
}

#[test_case]
fn nos_prompts_de_gravar_e_inserir_o_nome_relativo_ou_invalido_da_mensagem_na_linha_22() {
    restaurar_tela();
    let passos = roteiro(vec![
        texto("ab"),
        grava_como("a.txt"),
        vec![veja_linha(22, "Use um caminho absoluto, como /ram/nota.txt"), veja_modificado(true), veja_linha(1, "ab")],
        grava_como("/ram/nomemuitolongo.txt"),
        vec![veja_linha(22, "Erro ao gravar: caminho invalido"), veja_modificado(true), veja_linha(1, "ab")],
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("run mano", passos);
    assert!(!existe("/ram/nomemuitolongo.txt"));
    restaurar_tela();
}

#[test_case]
fn o_texto_no_limite_com_muitas_linhas_pequenas_edita_corta_cola_e_grava_sem_panico() {
    restaurar_tela();
    let antes = livres();
    cria("/ram/pico.txt", &arquivo_no_limite_de_bytes());
    // Corta 40 linhas, cola de volta (o limite exato), tenta colar de novo (recusado) e grava: 65536 bytes.
    let passos = roteiro(vec![
        vezes(40, || ctrl('k')),
        vec![ctrl('u'), ctrl('u'), veja_linha(22, "Limite de 4096 linhas atingido"), ctrl('s'), veja_linha(22, "Gravadas 4096 linhas")],
        vec![ctrl('x')],
    ]);
    let tela = roda_mano("exec mano /ram/pico.txt", passos);
    assert!(!tem(&tela, "[panic]"), "{:?}", tela);
    assert!(!tem(&tela, "codigo 101"), "{:?}", tela);
    assert_eq!(arquivo("/ram/pico.txt").len(), 65536);
    apaga("/ram/pico.txt");
    assert_eq!(livres(), antes);
    restaurar_tela();
}

/// Conta os polls do teclado e só digita `Ctrl+X` depois de 6 deles.
static POLLS: Mutex<u32> = Mutex::new(0);

fn tick_atrasado() {
    let mut polls = POLLS.lock();
    *polls += 1;
    if *polls == 6 {
        for scancode in [CTRL, 0x2D, 0xAD, CTRL | 0x80] {
            interrupts::push_scancode(scancode);
        }
    }
}

#[test_case]
fn o_mano_esperando_tecla_deixa_o_kernel_dormir_em_vez_de_girar() {
    restaurar_tela();
    reset_teclado();
    *POLLS.lock() = 0;
    scheduler::set_poll_hook(Some(tick_atrasado));
    let imagem = user::builtin_image("mano").expect("mano embutido");
    let relatorio = user::run_images(&[("mano", imagem)], None).expect("run recusado");
    scheduler::set_poll_hook(None);
    assert_eq!(relatorio.finished[0].termination, Termination::Exit { code: 0 });
    // Ninguém estava pronto e o `mano` esperava tecla: o kernel dormiu (`sti; hlt`) até ela chegar.
    assert!(relatorio.idle_loops >= 1, "o kernel nao dormiu: {}", relatorio.idle_loops);
    reset_teclado();
    restaurar_tela();
}

// ---------------------------------------------------------------------------
// Invariantes da tela em todos os estados (FR-013 e SC-005)
// ---------------------------------------------------------------------------

/// Em qualquer estado: o quadro da linha 24 está inteiro, a linha 23 também, e nada rolou.
fn confere_tela_estavel() {
    confere_que_nada_rolou();
    for row in 0..25 {
        assert_eq!(linha_inteira(row).len(), 80, "linha {}", row);
    }
    assert!(linha(0).starts_with(" mano  "), "{:?}", linha(0));
}

fn confere_destaque_cortado_na_coluna_79() {
    // `dois` começa na coluna 77: só `doi` cabe nas 80 colunas, e o resto fica fora da janela.
    assert_eq!(&linha_inteira(1)[77..80], "doi");
    for col in 77..80 {
        assert_eq!(cor(1, col), 0x70, "coluna {}", col);
    }
    assert_eq!(cor(1, 76), 0x07);
    // O quadro da linha 24 continua onde estava.
    assert_eq!(linha_inteira(24), quadro(&ATALHOS_2));
}

#[test_case]
fn a_tela_tem_80_bytes_por_linha_em_todos_os_estados_e_o_destaque_na_borda_e_cortado() {
    restaurar_tela();
    let mut bytes = vec![b'x'; 77];
    bytes.extend_from_slice(b"dois\n");
    cria("/ram/bd.txt", &bytes);
    let passos = roteiro(vec![
        vec![veja_com(confere_tela_estavel)],
        // Campo aberto, com o rótulo mais longo.
        vec![ctrl('o'), veja_com(confere_tela_estavel), esc()],
        // Pergunta de uma tecla.
        vec![tecla('z'), ctrl('x'), veja_com(confere_tela_estavel), esc(), backspace()],
        // Busca com o trecho na borda direita.
        vec![ctrl('w')],
        texto("dois"),
        vec![enter(), veja_com(confere_tela_estavel), veja_com(confere_destaque_cortado_na_coluna_79), veja_cursor(1, 77)],
        // Ajuda.
        vec![ctrl('g'), veja_com(confere_que_nada_rolou), tecla('q'), veja_com(confere_tela_estavel)],
        // O texto foi alterado (o `z` entrou e saiu): sair pergunta, e `n` descarta.
        vec![ctrl('x'), tecla('n')],
    ]);
    roda_mano("exec mano /ram/bd.txt", passos);
    apaga("/ram/bd.txt");
    restaurar_tela();
}

// ---------------------------------------------------------------------------
// Documentos e versão (FR-023 a FR-027, FR-022)
// ---------------------------------------------------------------------------

const MANUAL: &str = include_str!("../MANO.md");
const MANUAL_EN: &str = include_str!("../MANO.en.md");
const PERCURSO: &str = include_str!("../WALKTHROUGH-MANO.md");
const PERCURSO_EN: &str = include_str!("../WALKTHROUGH-MANO.en.md");
const FONTE: &str = include_str!("../programs/src/bin/mano.rs");
const README: &str = include_str!("../README.md");
const README_EN: &str = include_str!("../README.en.md");
const CHANGELOG: &str = include_str!("../CHANGELOG.md");
const CHANGELOG_EN: &str = include_str!("../CHANGELOG.en.md");
const GUIA: &str = include_str!("../GUIA_DO_PROGRAMADOR.md");
const GUIA_EN: &str = include_str!("../PROGRAMMERS_GUIDE.md");

/// Os nomes entre crases, fora dos blocos de código, na forma `nome()` ou `fn nome`.
fn nomes_de_funcao(texto: &str) -> Vec<String> {
    let mut nomes = Vec::new();
    let mut no_bloco = false;
    for linha in texto.lines() {
        if linha.trim_start().starts_with("```") {
            no_bloco = !no_bloco;
            continue;
        }
        if no_bloco {
            continue;
        }
        for (i, trecho) in linha.split('`').enumerate() {
            if i % 2 == 0 {
                continue;
            }
            let nome = if let Some(resto) = trecho.strip_prefix("fn ") {
                Some(resto)
            } else {
                trecho.strip_suffix("()")
            };
            if let Some(nome) = nome {
                if !nome.is_empty() && nome.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                    nomes.push(String::from(nome));
                }
            }
        }
    }
    nomes
}

#[test_case]
fn todo_nome_de_funcao_citado_no_percurso_existe_no_codigo_do_mano() {
    for (arquivo, texto) in [("WALKTHROUGH-MANO.md", PERCURSO), ("WALKTHROUGH-MANO.en.md", PERCURSO_EN)] {
        let nomes = nomes_de_funcao(texto);
        assert!(nomes.len() >= 40, "{}: poucos nomes achados ({})", arquivo, nomes.len());
        for nome in nomes {
            let declaracao = format!("fn {}(", nome);
            assert!(FONTE.contains(declaracao.as_str()), "{}: a funcao {:?} nao existe em mano.rs", arquivo, nome);
        }
    }
}

#[test_case]
fn os_quatro_documentos_novos_existem_com_a_troca_de_idioma_e_o_readme_e_o_changelog_citam_o_mano() {
    assert!(MANUAL.starts_with("[ENGLISH VERSION](MANO.en.md)"));
    assert!(PERCURSO.starts_with("[ENGLISH VERSION](WALKTHROUGH-MANO.en.md)"));
    assert!(MANUAL_EN.starts_with("[VERS"));
    assert!(PERCURSO_EN.starts_with("[VERS"));
    for texto in [MANUAL, MANUAL_EN, PERCURSO, PERCURSO_EN] {
        assert!(texto.len() > 2000);
    }
    for (nome, texto, titulo) in [("README.md", README, "\n## O editor `mano`"), ("README.en.md", README_EN, "\n## The `mano` editor")] {
        assert!(texto.contains("0.13.1"), "{}", nome);
        // A seção do `mano` vem logo depois do Status e antes do Roadmap.
        let status = texto.find("\n## Status").expect("Status");
        let mano = texto.find(titulo).expect("a secao do mano");
        let roadmap = texto.find("\n## Roadmap").expect("Roadmap");
        assert!(status < mano && mano < roadmap, "{}: a secao do mano deve ficar entre o Status e o Roadmap", nome);
        // O Marco 13 continua descrito (os testes de versao anteriores dependem disso).
        assert!(texto.contains("0.13.0"), "{}", nome);
    }
    assert!(README.contains("](MANO.md)") && README.contains("](WALKTHROUGH-MANO.md)"));
    assert!(README_EN.contains("](MANO.en.md)") && README_EN.contains("](WALKTHROUGH-MANO.en.md)"));
    assert!(CHANGELOG.contains("## [0.13.1] - 2026-11-18") && CHANGELOG.contains("`mano`"));
    assert!(CHANGELOG_EN.contains("## [0.13.1] - 2026-11-18") && CHANGELOG_EN.contains("`mano`"));
    assert!(CHANGELOG.contains("[0.13.0]") && CHANGELOG_EN.contains("[0.13.0]"));
    // Os dois guias ganham um paragrafo, na secao do teclado e da tela.
    assert!(GUIA.contains("MANO.md") && GUIA.contains("WALKTHROUGH-MANO.md"));
    assert!(GUIA_EN.contains("MANO.en.md") && GUIA_EN.contains("WALKTHROUGH-MANO.en.md"));
}

#[test_case]
fn a_versao_e_0_13_1_e_o_mano_aparece_uma_vez_na_lista_de_programas_em_ordem_alfabetica() {
    restaurar_tela();
    assert_eq!(os_rust::VERSION, "os-rust v0.13.1");
    exige(&rodar_comando("sobre"), "os-rust v0.13.1");
    let nomes: Vec<&str> = user::program_names().collect();
    assert_eq!(nomes.iter().filter(|&&n| n == "mano").count(), 1);
    assert!(nomes.windows(2).all(|w| w[0] < w[1]), "fora de ordem alfabetica: {:?}", nomes);
    // O `run` sem argumento lista os programas, e o `mano` está lá.
    exige(&rodar_comando("run"), "mano");
    restaurar_tela();
}
