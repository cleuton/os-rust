//! Testes do compilador `nanorust` (versão 0.13.2), de ponta a ponta dentro do
//! QEMU. O `nanorust` roda **de verdade**, iniciado pelo prompt
//! (`exec nanorust <fonte> <saida>`), e o programa que ele gera também roda de
//! verdade (`run <saida>`). Quando um programa lê números (`ler_i64`), os testes
//! "digitam" empurrando scancodes na fila do kernel, um caractere por vez, só
//! quando o programa está esperando.
//!
//! O padrão é o de `tests/mano.rs`. Os ajudantes são duplicados de propósito:
//! cada arquivo de `tests/` é um kernel próprio e não há módulo comum.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os_rust::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use os_rust::user;
use os_rust::{elf, fs, interrupts, keyboard, scheduler, shell, vga_buffer};
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

/// Os fontes dos exemplos entregues: o arquivo e o fonte testado são o mesmo.
const OLA: &str = include_str!("../programs/nano/exemplos/ola.rs");
const ARITM: &str = include_str!("../programs/nano/exemplos/aritm.rs");
const FIBO: &str = include_str!("../programs/nano/exemplos/fibo.rs");
const PRIMOS: &str = include_str!("../programs/nano/exemplos/primos.rs");
const LOGICA: &str = include_str!("../programs/nano/exemplos/logica.rs");
const ENTRADA: &str = include_str!("../programs/nano/exemplos/entrada.rs");

/// Os documentos e o código do compilador, para os testes da seção final.
const CODIGO: &str = include_str!("../programs/src/bin/nanorust.rs");
const MANUAL: &str = include_str!("../NANORUST.md");
const MANUAL_EN: &str = include_str!("../NANORUST.en.md");
const PERCURSO: &str = include_str!("../WALKTHROUGH-NANORUST.md");
const PERCURSO_EN: &str = include_str!("../WALKTHROUGH-NANORUST.en.md");
const README: &str = include_str!("../README.md");
const README_EN: &str = include_str!("../README.en.md");
const CHANGELOG: &str = include_str!("../CHANGELOG.md");
const CHANGELOG_EN: &str = include_str!("../CHANGELOG.en.md");
const GUIA: &str = include_str!("../GUIA_DO_PROGRAMADOR.md");
const GUIA_EN: &str = include_str!("../PROGRAMMERS_GUIDE.md");

// ---------------------------------------------------------------------------
// Tela, teclado e arquivos
// ---------------------------------------------------------------------------

/// Devolve o `Writer` ao prompt de sempre (cursor na última linha, cor padrão).
fn restaurar_tela() {
    vga_buffer::restore_prompt_line();
    vga_buffer::clear_screen();
}

/// Esvazia a fila de scancodes, solta Shift e Ctrl e zera o decodificador.
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

/// As linhas não vazias da tela, sem os espaços do fim.
fn linhas_da_tela() -> Vec<String> {
    (0..25).map(linha).filter(|l| !l.is_empty()).collect()
}

/// A tela inteira como um texto corrido (uma mensagem de mais de 80 colunas
/// quebra em duas linhas, e aqui continua contígua).
fn corrida() -> String {
    (0..25).map(linha_inteira).collect()
}

/// Digita uma linha no shell, byte a byte, como o teclado faria, e aperta Enter.
fn digitar_no_prompt(texto: &str) {
    for byte in texto.bytes() {
        shell::feed(byte);
    }
    shell::feed(b'\n');
}

/// Limpa a tela, roda o comando no prompt e devolve a tela corrida.
fn rodar_comando(comando: &str) -> String {
    vga_buffer::clear_screen();
    digitar_no_prompt(comando);
    corrida()
}

/// Cria `caminho` (no `/ram`) com `bytes`, apagando antes o que houver lá.
fn cria(caminho: &str, bytes: &[u8]) {
    let _ = fs::remove(caminho.as_bytes());
    let mut arquivo = fs::OpenFile::create(caminho.as_bytes()).expect("cria o arquivo no /ram");
    for pedaco in bytes.chunks(512) {
        let mut feito = 0;
        while feito < pedaco.len() {
            feito += arquivo.write(&pedaco[feito..]).expect("grava no /ram");
        }
    }
}

fn apaga(caminho: &str) {
    let _ = fs::remove(caminho.as_bytes());
}

fn arquivo(caminho: &str) -> Vec<u8> {
    fs::read_whole(caminho.as_bytes(), 1 << 20).expect("le o arquivo")
}

fn existe(caminho: &str) -> bool {
    fs::lookup(caminho.as_bytes()).is_ok()
}

// ---------------------------------------------------------------------------
// Entrada digitada: um caractere por passo, só quando o programa espera
// ---------------------------------------------------------------------------

struct Digitacao {
    passos: Vec<Vec<u8>>,
    proximo: usize,
}

static DIGITACAO: Mutex<Digitacao> = Mutex::new(Digitacao { passos: Vec::new(), proximo: 0 });

/// O gancho do escalonador: se algum programa espera tecla, empurra os
/// scancodes do próximo caractere (no máximo 2 por passo; a fila tem 16).
fn tick_digitacao() {
    if scheduler::keyboard_waiters() == 0 {
        return;
    }
    let mut d = DIGITACAO.lock();
    let k = d.proximo;
    if k >= d.passos.len() {
        return;
    }
    d.proximo += 1;
    for &scancode in d.passos[k].iter() {
        interrupts::push_scancode(scancode);
    }
}

/// O scancode (Set 1, layout US) de um caractere da entrada digitada.
fn codigo(c: char) -> u8 {
    match c {
        '1'..='9' => 0x02 + (c as u8 - b'1'),
        '0' => 0x0B,
        '-' => 0x0C,
        'a' => 0x1E,
        'b' => 0x30,
        'c' => 0x2E,
        'x' => 0x2D,
        _ => panic!("sem scancode para {:?}", c),
    }
}

/// Roda `comando` no prompt, digitando `linhas` (uma por leitura, cada uma
/// terminada em Enter) e devolve a tela corrida no fim.
fn roda_digitando(comando: &str, linhas: &[&str]) -> String {
    reset_teclado();
    {
        let mut d = DIGITACAO.lock();
        d.passos.clear();
        d.proximo = 0;
        for l in linhas {
            for c in l.chars() {
                let make = codigo(c);
                d.passos.push(alloc::vec![make, make | 0x80]);
            }
            d.passos.push(alloc::vec![0x1C, 0x9C]); // Enter
        }
    }
    scheduler::set_poll_hook(Some(tick_digitacao));
    let tela = rodar_comando(comando);
    scheduler::set_poll_hook(None);
    let consumidos = DIGITACAO.lock().proximo;
    let total = DIGITACAO.lock().passos.len();
    assert_eq!(consumidos, total, "o programa terminou antes de ler toda a entrada");
    reset_teclado();
    tela
}

// ---------------------------------------------------------------------------
// Compilar, validar e rodar
// ---------------------------------------------------------------------------

/// Compila `fonte` (gravado em `/ram/x.rs`) para `saida` e devolve a tela.
fn compila(fonte: &str, saida: &str) -> String {
    cria("/ram/x.rs", fonte.as_bytes());
    let tela = rodar_comando(&format!("exec nanorust /ram/x.rs {}", saida));
    apaga("/ram/x.rs");
    tela
}

/// O `N` de `nanorust: ok, <N> bytes` na tela, ou o `panic` do teste.
fn bytes_ok(tela: &str) -> usize {
    let marca = "nanorust: ok, ";
    let i = tela.find(marca).unwrap_or_else(|| panic!("a compilacao nao deu certo: {:?}", tela.trim()));
    let resto = &tela[i + marca.len()..];
    let fim = resto.find(" bytes").expect("a mensagem tem 'bytes'");
    resto[..fim].parse().expect("o tamanho e um numero")
}

/// Confere o executável gerado: aceito por `elf::parse`, um segmento R+X,
/// `e_entry = 0x4000_0078`, tamanho igual a `n` e no máximo 65536.
fn valida_elf(caminho: &str, n: usize) {
    let bytes = arquivo(caminho);
    assert_eq!(bytes.len(), n);
    assert!(n <= 65536);
    let imagem = elf::parse(&bytes).expect("o kernel aceita o executavel gerado");
    assert_eq!(imagem.entry, 0x4000_0078);
    let segs = imagem.segments();
    assert_eq!(segs.len(), 1);
    assert!(segs[0].flags.execute && !segs[0].flags.write, "o segmento e R+X");
    assert_eq!(segs[0].vaddr, 0x4000_0000);
    assert_eq!(segs[0].file_bytes.len(), n);
}

/// Compila `fonte` em `/ram/x`, valida o ELF e devolve o tamanho.
fn compila_e_valida(fonte: &str) -> usize {
    apaga("/ram/x");
    let tela = compila(fonte, "/ram/x");
    let n = bytes_ok(&tela);
    valida_elf("/ram/x", n);
    n
}

/// Roda `/ram/x` e devolve as linhas não vazias da tela.
fn executa() -> Vec<String> {
    rodar_comando("run /ram/x");
    linhas_da_tela()
}

fn sem_codigo(linhas: &[String]) -> bool {
    !linhas.iter().any(|l| l.contains("terminou com codigo"))
}

/// Cada linha esperada aparece na tela, nesta ordem (outras linhas, como a
/// entrada digitada, podem vir no meio).
fn contem_em_ordem(linhas: &[String], esperado: &[&str]) {
    let mut i = 0;
    for e in esperado {
        match linhas[i..].iter().position(|l| l == e) {
            Some(p) => i += p + 1,
            None => panic!("falta a linha {:?} (na ordem) em {:?}", e, linhas),
        }
    }
}

/// Compila o exemplo, valida e roda; confere a saída e o código 0.
fn exemplo(fonte: &str, esperado: &[&str]) {
    compila_e_valida(fonte);
    let linhas = executa();
    contem_em_ordem(&linhas, esperado);
    assert!(sem_codigo(&linhas), "{:?}", linhas);
    apaga("/ram/x");
}

// ---------------------------------------------------------------------------
// Grupo 1: os exemplos
// ---------------------------------------------------------------------------

#[test_case]
fn exemplo_ola() {
    restaurar_tela();
    exemplo(
        OLA,
        &[
            "Ola, os-rust!",
            "Compilado dentro do proprio sistema.",
            "sem quebra, duas partes",
            "aspas \" e barra \\ funcionam",
        ],
    );
}

#[test_case]
fn exemplo_aritm() {
    restaurar_tela();
    exemplo(
        ARITM,
        &[
            "22 12 85",
            "3 2",
            "-3 -2",
            "14 20 50",
            "-9223372036854775808",
            "9223372036854775807",
            "logica ok",
            "a > b",
            "a != b",
            "a >= 17",
            "b < a",
        ],
    );
}

#[test_case]
fn exemplo_fibo() {
    restaurar_tela();
    exemplo(
        FIBO,
        &[
            "fib(0) = 0",
            "fib(1) = 1",
            "fib(2) = 1",
            "fib(3) = 2",
            "fib(4) = 3",
            "fib(5) = 5",
            "fib(6) = 8",
            "fib(7) = 13",
            "fib(8) = 21",
            "fib(9) = 34",
            "fib(10) = 55",
        ],
    );
}

#[test_case]
fn exemplo_primos() {
    restaurar_tela();
    exemplo(PRIMOS, &["2", "3", "5", "7", "11", "13", "17", "19", "23", "29", "31", "37", "41", "43", "47"]);
}

#[test_case]
fn exemplo_logica() {
    restaurar_tela();
    exemplo(
        LOGICA,
        &[
            "f(1)",
            "t(3)",
            "b",
            "f(5)",
            "t(6)",
            "c",
            "t(7)",
            "t(8)",
            "d",
            "f(9)",
            "e",
            "avalia 1",
            "avalia 2",
            "avalia 3",
            "1 2 3",
        ],
    );
}

#[test_case]
fn exemplo_entrada() {
    restaurar_tela();
    compila_e_valida(ENTRADA);
    // Valido, negativo, lixo, 19 digitos, e uma linha de 30 digitos (o kernel
    // descarta o excedente) seguida de um numero valido.
    let tela = roda_digitando(
        "run /ram/x",
        &["42", "-7", "abc", "1234567890123456789", "123456789012345678901234567890", "5"],
    );
    let linhas: Vec<String> = (0..25).map(linha).filter(|l| !l.is_empty()).collect();
    contem_em_ordem(
        &linhas,
        &["a = 42", "b = -7", "c = 0", "d = 0", "e = 0", "f = 5", "soma = 40"],
    );
    assert!(!tela.contains("terminou com codigo"), "{:?}", tela.trim());
    apaga("/ram/x");
}

// ---------------------------------------------------------------------------
// Grupo 2: erros
// ---------------------------------------------------------------------------

/// Compila `fonte` e exige a mensagem `esperado` (texto exato, com linha e
/// coluna), o código de término 1 e a ausência do arquivo de saída.
fn erro(fonte: &str, esperado: &str) {
    apaga("/ram/s");
    let tela = compila(fonte, "/ram/s");
    assert!(tela.contains(esperado), "esperado {:?}, a tela mostra {:?}", esperado, tela.trim());
    assert!(tela.contains("terminou com codigo 1"), "{:?}", tela.trim());
    assert!(!existe("/ram/s"), "um erro de compilacao nao cria arquivo");
}

#[test_case]
fn erros_de_lexico_m01_m02_m03() {
    restaurar_tela();
    erro("fn main() { @ }", "erro na linha 1, coluna 13: caractere inesperado '@'");
    erro("fn main() { \x01 }", "erro na linha 1, coluna 13: byte inesperado 0x01");
    erro("fn main() { let x: i64 = 12abc; }", "erro na linha 1, coluna 26: numero invalido");
    erro("fn main() { let x: i64 = 9223372036854775808; }", "erro na linha 1, coluna 26: numero invalido");
    erro("fn main() { println!(\"abc); }", "erro na linha 1, coluna 22: texto invalido");
    erro("fn main() { let x: i64 = \"a\"; }", "erro na linha 1, coluna 26: texto invalido");
}

#[test_case]
fn erro_m04_o_exemplo_da_especificacao() {
    restaurar_tela();
    erro(
        "fn main() { let x: i64 = 1 + ; }",
        "erro na linha 1, coluna 30: esperado expressao, encontrado ';'",
    );
    erro("fn main() { loop }", "erro na linha 1, coluna 13: esperado expressao, encontrado 'loop'");
    erro("fn main() {} }", "erro na linha 1, coluna 14: esperado 'fn', encontrado '}'");
    erro("fn main() {", "erro na linha 1, coluna 12: esperado '}', encontrado fim do fonte");
}

#[test_case]
fn erro_m06_nome_que_nao_existe() {
    restaurar_tela();
    erro("fn main() { let x: i64 = fib + 1; }", "erro na linha 1, coluna 26: 'fib' nao existe");
    erro("fn main() { let y: i64 = x(1); }", "erro na linha 1, coluna 26: 'x' nao existe");
}

#[test_case]
fn erro_m07_nome_invalido_de_funcao() {
    restaurar_tela();
    erro(
        "fn let() {} fn main() {}",
        "erro na linha 1, coluna 4: nome 'let' invalido (reservado ou ja existe)",
    );
}

#[test_case]
fn erro_m08_o_exemplo_da_especificacao() {
    restaurar_tela();
    erro(
        "fn main() { let x: bool = 1; }",
        "erro na linha 1, coluna 27: tipos incompativeis: esperado bool, encontrado i64",
    );
}

#[test_case]
fn erro_m09_quantidade_de_argumentos() {
    restaurar_tela();
    erro(
        "fn f(a: i64) {} fn main() { f(1, 2); }",
        "erro na linha 1, coluna 29: a funcao 'f' espera 1 argumentos, recebeu 2",
    );
}

#[test_case]
fn erro_m10_variavel_que_nao_e_mutavel() {
    restaurar_tela();
    erro(
        "fn main() { let x: i64 = 1; x = 2; }",
        "erro na linha 1, coluna 29: 'x' nao e mutavel (declare com let mut)",
    );
}

#[test_case]
fn erro_m11_inteiro_solto() {
    restaurar_tela();
    erro(
        "fn main() { 2147483647 + 1; }",
        "erro na linha 1, coluna 13: inteiro solto: use uma variavel ou chamada para fixar o tipo i64",
    );
}

#[test_case]
fn erro_m12_divisao_por_zero_literal() {
    restaurar_tela();
    erro("fn main() { let x: i64 = 1 / 0; }", "erro na linha 1, coluna 28: divisao por zero literal");
}

#[test_case]
fn erro_m13_o_exemplo_da_especificacao() {
    restaurar_tela();
    erro(
        "fn main() { println!(\"{}\"); }",
        "erro na linha 1, coluna 22: texto de formato invalido",
    );
}

#[test_case]
fn erro_m14_sem_main() {
    restaurar_tela();
    erro("", "erro na linha 1, coluna 1: funcao main nao encontrada");
}

#[test_case]
fn erro_m15_main_com_parametros_ou_chamada() {
    restaurar_tela();
    erro(
        "fn main(x: i64) {}",
        "erro na linha 1, coluna 4: main nao pode ter parametros nem retorno, nem ser chamada",
    );
    erro(
        "fn main() { main(); }",
        "erro na linha 1, coluna 13: main nao pode ter parametros nem retorno, nem ser chamada",
    );
}

#[test_case]
fn erro_m18_fonte_inexistente() {
    restaurar_tela();
    apaga("/ram/nada.rs");
    apaga("/ram/s");
    let tela = rodar_comando("exec nanorust /ram/nada.rs /ram/s");
    assert!(tela.contains("erro: nao consegui abrir '/ram/nada.rs': nao encontrado"), "{:?}", tela.trim());
    assert!(tela.contains("terminou com codigo 1"));
    assert!(!existe("/ram/s"));
}

#[test_case]
fn erro_m19_saida_aberta_por_outro_programa() {
    restaurar_tela();
    cria("/ram/s", b"antigo");
    // Uma tabela de arquivos do kernel segura o arquivo aberto: o `overwrite` do
    // nanorust recebe `em uso`.
    let mut tabela = fs::FileTable::new();
    tabela.open_append(b"/ram/s").expect("abre a saida");
    let tela = compila("fn main() {}", "/ram/s");
    drop(tabela);
    assert!(tela.contains("erro: nao consegui gravar '/ram/s': em uso"), "{:?}", tela.trim());
    assert!(tela.contains("terminou com codigo 1"));
    assert!(existe("/ram/s"), "nada foi removido");
    assert_eq!(arquivo("/ram/s"), b"antigo", "o conteudo antigo continua intacto");
    apaga("/ram/s");
}

#[test_case]
fn erro_m20_fonte_e_saida_sao_o_mesmo_arquivo() {
    restaurar_tela();
    cria("/ram/a.rs", b"fn main() {}");
    let tela = rodar_comando("exec nanorust /ram/a.rs /ram/A.RS");
    assert!(tela.contains("erro: fonte e saida sao o mesmo arquivo"), "{:?}", tela.trim());
    assert!(tela.contains("terminou com codigo 1"));
    assert_eq!(arquivo("/ram/a.rs"), b"fn main() {}", "o fonte continua como estava");
    apaga("/ram/a.rs");
}

#[test_case]
fn ordem_e_escopo_dos_nomes() {
    restaurar_tela();
    // (i) o parametro com o nome de uma funcao declarada depois: M07 de parametro.
    erro(
        "fn f(g: i64) {} fn g() {} fn main() {}",
        "erro na linha 1, coluna 6: nome 'g' invalido (reservado ou ja existe)",
    );
    // (ii) M07 de `let`, na coluna de `fn`.
    erro(
        "fn main() { let fn: i64 = 1; }",
        "erro na linha 1, coluna 17: nome 'fn' invalido (reservado ou ja existe)",
    );
    // (iii) `x` no proprio inicializador, sem existir antes: M06.
    erro("fn main() { let x: i64 = x + 1; }", "erro na linha 1, coluna 26: 'x' nao existe");
}

#[test_case]
fn saida_existente_com_erro_de_compilacao_continua_intacta() {
    restaurar_tela();
    cria("/ram/s", b"conteudo antigo");
    let tela = compila("fn main() { let x: i64 = 1 + ; }", "/ram/s");
    assert!(tela.contains("esperado expressao"), "{:?}", tela.trim());
    assert_eq!(arquivo("/ram/s"), b"conteudo antigo");
    apaga("/ram/s");
}

#[test_case]
fn sem_argumentos_mostra_o_uso() {
    restaurar_tela();
    for comando in ["exec nanorust", "run nanorust", "exec nanorust /ram/a.rs"] {
        let tela = rodar_comando(comando);
        assert!(tela.contains("nanorust: uso: exec nanorust <fonte> <saida>"), "{}: {:?}", comando, tela.trim());
        assert!(tela.contains("terminou com codigo 1"), "{}", comando);
    }
}

// ---------------------------------------------------------------------------
// Grupo 3: execucao (panics)
// ---------------------------------------------------------------------------

/// Compila `fonte`, roda digitando `linhas` e exige `panic:` na tela e código 101.
fn panico(fonte: &str, linhas: &[&str], mensagem: &str) {
    compila_e_valida(fonte);
    let tela = roda_digitando("run /ram/x", linhas);
    assert!(tela.contains(mensagem), "esperado {:?}, a tela mostra {:?}", mensagem, tela.trim());
    assert!(tela.contains("terminou com codigo 101"), "{:?}", tela.trim());
    apaga("/ram/x");
}

#[test_case]
fn estouro_aritmetico_termina_com_101() {
    restaurar_tela();
    panico(
        "fn main() { let a: i64 = ler_i64(); let b: i64 = a * 10; println!(\"{}\", b); }",
        &["999999999999999999"],
        "panic: estouro aritmetico",
    );
}

#[test_case]
fn divisao_por_zero_termina_com_101() {
    restaurar_tela();
    panico(
        "fn main() { let a: i64 = ler_i64(); println!(\"{}\", 5 / a); }",
        &["0"],
        "panic: divisao por zero",
    );
}

#[test_case]
fn recursao_sem_fim_da_pilha_esgotada_e_nao_falha_de_pagina() {
    restaurar_tela();
    panico(
        "fn f(n: i64) -> i64 { f(n + 1) + 1 } fn main() { let x: i64 = f(0); println!(\"{}\", x); }",
        &[],
        "panic: pilha esgotada",
    );
}

// ---------------------------------------------------------------------------
// Grupo 4: regravar
// ---------------------------------------------------------------------------

#[test_case]
fn recompilar_substitui_o_executavel_e_gera_arquivos_iguais() {
    restaurar_tela();
    let n1 = compila_e_valida(FIBO);
    let primeiro = arquivo("/ram/x");
    // Outro fonte sobre o mesmo nome: o segundo executavel e o que roda.
    let tela = compila(OLA, "/ram/x");
    let n2 = bytes_ok(&tela);
    assert_ne!(n1, n2);
    let linhas = executa();
    contem_em_ordem(&linhas, &["Ola, os-rust!"]);
    assert!(!linhas.iter().any(|l| l.contains("fib(")));
    // O mesmo fonte duas vezes: arquivos iguais, byte a byte (saida deterministica).
    let tela = compila(FIBO, "/ram/x");
    assert_eq!(bytes_ok(&tela), n1);
    assert_eq!(arquivo("/ram/x"), primeiro);
    apaga("/ram/x");
}

// ---------------------------------------------------------------------------
// Grupo 5: limites
// ---------------------------------------------------------------------------

fn aninhado(pares: usize) -> String {
    format!("fn main() {{ let x: i64 = {}0{}; println!(\"{{}}\", x); }}", "(".repeat(pares), ")".repeat(pares))
}

#[test_case]
fn aninhamento_10_aceito_e_11_recusado() {
    restaurar_tela();
    compila_e_valida(&aninhado(9));
    let linhas = executa();
    contem_em_ordem(&linhas, &["0"]);
    apaga("/ram/x");
    // 10 pares + o corpo de `main` = 11 niveis: o 10o `(` esta na coluna 35.
    erro(&aninhado(10), "erro na linha 1, coluna 35: aninhamento demais (maximo 10)");
}

/// 10 niveis misturando `if` na condicao, chamadas com argumentos que sao
/// chamadas, blocos, parenteses e operadores unarios. `extra` acrescenta o 11o.
fn misto(extra: bool) -> String {
    let fundo = if extra { "((x))" } else { "(x)" };
    format!(
        "fn g(a: i64) -> i64 {{\n    a\n}}\n\nfn main() {{\n    let x: i64 = 1;\n    if x > 0 {{\n        let r: i64 = g(x + 2 * -(if true || true && x + 2 * g(x + 2 * (-{})) > 0 {{ 1 }} else {{ 2 }}));\n        println!(\"{{}}\", r);\n    }}\n}}\n",
        fundo
    )
}

#[test_case]
fn aninhamento_misto_10_aceito_e_11_recusado() {
    restaurar_tela();
    compila_e_valida(&misto(false));
    let linhas = executa();
    contem_em_ordem(&linhas, &["-1"]);
    assert!(sem_codigo(&linhas), "{:?}", linhas);
    apaga("/ram/x");
    erro(&misto(true), "erro na linha 8, coluna 74: aninhamento demais (maximo 10)");
}

fn com_parametros(n: usize) -> String {
    let lista: Vec<String> = (1..=n).map(|i| format!("a{}: i64", i)).collect();
    format!("fn f({}) {{}} fn main() {{}}", lista.join(", "))
}

#[test_case]
fn parametros_8_aceitos_e_9_recusados() {
    restaurar_tela();
    compila_e_valida(&com_parametros(8));
    apaga("/ram/x");
    // "fn f(" tem 5 bytes e cada "aN: i64, " tem 9: o 9o nome esta na coluna 6 + 8 * 9.
    erro(&com_parametros(9), "erro na linha 1, coluna 78: limite excedido: parametros (maximo 8)");
}

fn com_argumentos(n: usize) -> String {
    let chaves = "{}".repeat(n);
    let args = alloc::vec!["v"; n].join(", ");
    format!("fn main() {{ let v: i64 = 1; println!(\"{}\", {}); }}", chaves, args)
}

#[test_case]
fn argumentos_de_println_8_aceitos_e_9_recusados() {
    restaurar_tela();
    compila_e_valida(&com_argumentos(8));
    let linhas = executa();
    contem_em_ordem(&linhas, &["11111111"]);
    apaga("/ram/x");
    erro(&com_argumentos(9), "erro na linha 1, coluna 84: limite excedido: argumentos (maximo 8)");
}

fn com_tamanho(total: usize) -> String {
    let base = "fn main() { println!(\"ok\"); }\n";
    let mut s = String::from(base);
    s.push_str("//");
    while s.len() < total {
        s.push('a');
    }
    s
}

#[test_case]
fn fonte_de_16384_bytes_aceito_e_de_16385_recusado() {
    restaurar_tela();
    let fonte = com_tamanho(16384);
    assert_eq!(fonte.len(), 16384);
    compila_e_valida(&fonte);
    apaga("/ram/x");
    let fonte = com_tamanho(16385);
    assert_eq!(fonte.len(), 16385);
    erro(&fonte, "erro na linha 1, coluna 1: limite excedido: fonte (maximo 16384 bytes)");
}

fn soma(termos: usize) -> String {
    format!("fn main() {{ let x: i64 = {}; println!(\"{{}}\", x); }}", alloc::vec!["1"; termos].join("+"))
}

#[test_case]
fn soma_de_1000_termos_compila_e_imprime_1000() {
    restaurar_tela();
    compila_e_valida(&soma(1000));
    let linhas = executa();
    contem_em_ordem(&linhas, &["1000"]);
    assert!(sem_codigo(&linhas), "{:?}", linhas);
    apaga("/ram/x");
}

#[test_case]
fn soma_de_8000_termos_passa_de_65536_bytes_e_da_m17() {
    restaurar_tela();
    let fonte = soma(8000);
    assert!(fonte.len() <= 16384);
    erro(&fonte, "erro: programa grande demais (maximo 65536 bytes)");
}

// ---------------------------------------------------------------------------
// Grupo 7: versao e documentos
// ---------------------------------------------------------------------------

#[test_case]
fn a_versao_e_0_13_2_e_o_nanorust_aparece_uma_vez_na_lista_de_programas_em_ordem_alfabetica() {
    restaurar_tela();
    assert_eq!(os_rust::VERSION, "os-rust v0.13.2");
    assert!(rodar_comando("sobre").contains("os-rust v0.13.2"));
    let nomes: Vec<&str> = user::program_names().collect();
    assert_eq!(nomes.iter().filter(|&&n| n == "nanorust").count(), 1);
    assert!(nomes.windows(2).all(|w| w[0] < w[1]), "fora de ordem alfabetica: {:?}", nomes);
}

/// Todo `nome()` citado entre crases em um percurso existe como `fn nome(` no codigo.
fn nomes_do_percurso_existem(percurso: &str) {
    let mut conferidos = 0;
    for (i, trecho) in percurso.split('`').enumerate() {
        if i % 2 == 0 || !trecho.ends_with("()") {
            continue;
        }
        let nome = trecho.trim_end_matches("()");
        if nome.is_empty() || !nome.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':') {
            continue;
        }
        let simples = nome.rsplit("::").next().expect("nome");
        assert!(CODIGO.contains(&format!("fn {}(", simples)), "o percurso cita {:?}, que nao existe no codigo", trecho);
        conferidos += 1;
    }
    assert!(conferidos > 30, "o percurso cita poucos nomes: {}", conferidos);
}

#[test_case]
fn os_quatro_documentos_novos_existem_com_a_troca_de_idioma_e_o_readme_o_changelog_e_os_guias_citam_o_nanorust() {
    assert!(MANUAL.starts_with("[ENGLISH VERSION](NANORUST.en.md)"));
    assert!(PERCURSO.starts_with("[ENGLISH VERSION](WALKTHROUGH-NANORUST.en.md)"));
    assert!(MANUAL_EN.starts_with("[VERS"));
    assert!(PERCURSO_EN.starts_with("[VERS"));
    for texto in [MANUAL, MANUAL_EN, PERCURSO, PERCURSO_EN] {
        assert!(texto.len() > 4000);
    }
    // O manual traz a tabela de erros inteira (M01 a M20) e os Limites da garantia.
    for texto in [MANUAL, MANUAL_EN] {
        for n in 1..=20 {
            assert!(texto.contains(&format!("| M{:02} |", n)), "falta M{:02}", n);
        }
        assert!(texto.contains("cat programs/nano/prelude.rs fonte.rs > x.rs && rustc x.rs"));
        assert!(texto.contains("snapshot=on"));
    }
    for (nome, texto, titulo) in [
        ("README.md", README, "\n## O compilador `nanorust`"),
        ("README.en.md", README_EN, "\n## The `nanorust` compiler"),
    ] {
        assert!(texto.contains("0.13.2"), "{}", nome);
        // A secao do nanorust vem logo depois da do mano e antes do Roadmap.
        let status = texto.find("\n## Status").expect("Status");
        let mano = texto.find(if nome == "README.md" { "\n## O editor `mano`" } else { "\n## The `mano` editor" }).expect("mano");
        let nano = texto.find(titulo).expect("a secao do nanorust");
        let roadmap = texto.find("\n## Roadmap").expect("Roadmap");
        assert!(status < mano && mano < nano && nano < roadmap, "{}: ordem das secoes", nome);
    }
    assert!(README.contains("](NANORUST.md)") && README.contains("](WALKTHROUGH-NANORUST.md)"));
    assert!(README_EN.contains("](NANORUST.en.md)") && README_EN.contains("](WALKTHROUGH-NANORUST.en.md)"));
    assert!(CHANGELOG.contains("## [0.13.2] - 2026-11-18") && CHANGELOG.contains("`nanorust`"));
    assert!(CHANGELOG_EN.contains("## [0.13.2] - 2026-11-18") && CHANGELOG_EN.contains("`nanorust`"));
    assert!(CHANGELOG.contains("[0.13.1]") && CHANGELOG_EN.contains("[0.13.1]"));
    assert!(GUIA.contains("## Duas formas de criar programas") && GUIA.contains("NANORUST.md"));
    assert!(GUIA_EN.contains("## Two ways to create programs") && GUIA_EN.contains("NANORUST.en.md"));
    nomes_do_percurso_existem(PERCURSO);
    nomes_do_percurso_existem(PERCURSO_EN);
}
