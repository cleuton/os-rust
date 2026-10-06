//! Higiene dos artefatos entregues: nenhum arquivo do projeto cita a
//! ferramenta usada para escrever as especificações, nem aponta para as pastas
//! de especificação. As pastas do próprio fluxo de especificação ficam de fora
//! (ver `build.rs`).
//!
//! O teste roda dentro do QEMU, sem sistema de arquivos: o `build.rs` embute o
//! texto de cada arquivo entregue em tempo de compilação, e a lista é incluída
//! abaixo.
//!
//! Os termos procurados são **montados em tempo de execução**, a partir de
//! pedaços: se estivessem escritos por inteiro, este arquivo (que também é
//! entregue) seria o primeiro a ser acusado.

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

/// Todos os arquivos de texto entregues: `(caminho relativo, conteúdo)`.
static ARQUIVOS: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/arquivos_entregues.rs"));

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

/// Junta pedaços em um termo, em minúsculas.
fn termo(pedacos: &[&str]) -> String {
    pedacos.concat()
}

/// Os termos que nenhum arquivo entregue pode conter, sem diferenciar
/// maiúsculas de minúsculas.
fn termos_proibidos() -> Vec<String> {
    alloc::vec![
        termo(&["spec", "kit"]),
        termo(&["spec", "-", "kit"]),
        termo(&["spec", " ", "kit"]),
        termo(&["/spec", "kit"]),
        termo(&[".spec", "ify"]),
    ]
}

/// Minúsculas de um texto ASCII (o suficiente para estes termos).
fn minusculas(texto: &str) -> String {
    texto.chars().map(|c| c.to_ascii_lowercase()).collect()
}

#[test_case]
fn nenhum_artefato_cita_a_ferramenta_de_especificacao() {
    let proibidos = termos_proibidos();
    for (caminho, texto) in ARQUIVOS {
        for (numero, linha) in texto.lines().enumerate() {
            let linha = minusculas(linha);
            for proibido in &proibidos {
                assert!(
                    !linha.contains(proibido.as_str()),
                    "{}:{}: referencia a ferramenta de especificacao",
                    caminho,
                    numero + 1
                );
            }
        }
    }
}

#[test_case]
fn nenhum_artefato_cita_pasta_de_spec() {
    // O padrão é `specs/` seguido de três dígitos e um hífen (o nome de uma
    // pasta de especificação). Montado em pedaços, como os termos acima.
    let prefixo = termo(&["spe", "cs/"]);
    for (caminho, texto) in ARQUIVOS {
        for (numero, linha) in texto.lines().enumerate() {
            let mut resto = linha;
            while let Some(posicao) = resto.find(prefixo.as_str()) {
                let depois = resto[posicao + prefixo.len()..].as_bytes();
                let parece_pasta = depois.len() >= 4
                    && depois[..3].iter().all(|b| b.is_ascii_digit())
                    && depois[3] == b'-';
                assert!(
                    !parece_pasta,
                    "{}:{}: aponta para uma pasta de especificacao",
                    caminho,
                    numero + 1
                );
                resto = &resto[posicao + prefixo.len()..];
            }
        }
    }
}

#[test_case]
fn a_varredura_enxerga_arquivos_de_verdade() {
    // Se a lista viesse vazia (ou sem os arquivos principais), os dois testes
    // acima passariam sem verificar nada.
    assert!(!ARQUIVOS.is_empty());
    for esperado in ["README.md", "src/scheduler.rs", "tests/artefatos.rs", "build.rs"] {
        assert!(
            ARQUIVOS.iter().any(|(caminho, _)| *caminho == esperado),
            "a varredura nao inclui {}",
            esperado
        );
    }
    // E as pastas isentas ficam de fora.
    let isenta = termo(&["spe", "cs/"]);
    assert!(
        !ARQUIVOS.iter().any(|(caminho, _)| caminho.starts_with(isenta.as_str())),
        "a varredura incluiu uma pasta isenta"
    );
}
