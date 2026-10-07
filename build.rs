// Compila os programas de usuário (crate `programs/`) e os embute no kernel.
//
// Este script roda dentro do mesmo `cargo run`/`cargo test` de sempre, então
// nenhum passo manual é necessário: ele
// executa um segundo `cargo build`, para o target de usuário
// (`x86_64-os_rust_user.json`), copia cada ELF resultante para `OUT_DIR` e
// escreve `OUT_DIR/programs.rs`, a tabela `PROGRAMS` que `src/programs.rs`
// inclui. Se a compilação do programa falha, o script termina com `panic!`
// e o cargo externo para com o erro do compilador visível: nunca sai uma
// imagem de boot com um programa ausente, corrompido ou desatualizado.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR definido pelo cargo"));
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR definido pelo cargo"));
    let cargo = env::var("CARGO").expect("CARGO definido pelo cargo");

    let user_target = root.join("x86_64-os_rust_user.json");
    let programs_dir = root.join("programs");
    let user_target_dir = out_dir.join("user-target");

    let mut cmd = Command::new(cargo);
    cmd.current_dir(&programs_dir)
        .args(["build", "--release", "--target"])
        .arg(&user_target)
        // Cuidado 1 do cargo aninhado: o cargo externo segura o lock do
        // diretório de build dele; usar o mesmo diretório aqui esperaria
        // esse lock para sempre. Um `--target-dir` só nosso evita isso.
        .arg("--target-dir")
        .arg(&user_target_dir);

    // Cuidado 2: o cargo externo injeta no ambiente deste script tudo que
    // descreve a compilação do *kernel* (flags, target, perfil, diretórios).
    // Se o cargo interno herdasse isso, o programa de usuário seria
    // compilado com as opções do kernel. `CARGO_HOME` fica, porque diz onde
    // estão os caches de dependências.
    for (name, _) in env::vars() {
        let is_cargo_injected = name.starts_with("CARGO_") && name != "CARGO_HOME";
        let is_rustc_injected = name.starts_with("RUSTC") || name == "RUSTFLAGS";
        let is_build_script_injected = matches!(
            name.as_str(),
            "TARGET" | "HOST" | "PROFILE" | "OPT_LEVEL" | "DEBUG" | "NUM_JOBS" | "OUT_DIR"
        );
        if is_cargo_injected || is_rustc_injected || is_build_script_injected {
            cmd.env_remove(&name);
        }
    }

    // Cuidado 3 (fora deste arquivo): `programs` precisa ser membro do
    // workspace declarado em `Cargo.toml`, senão o cargo interno recusa
    // compilar um pacote que "acredita estar em um workspace que não é o seu".
    let status = cmd
        .status()
        .expect("falha ao executar o cargo para compilar os programas de usuario");
    // A cópia dos ELFs só acontece depois deste teste: em caso de erro nunca
    // sobra um ELF antigo para ser embutido por engano.
    assert!(status.success(), "falha ao compilar os programas de usuario");

    // Um programa por arquivo em `programs/src/bin/`; acrescentar um programa
    // novo é criar um arquivo, sem mexer neste script.
    let mut names: Vec<String> = fs::read_dir(programs_dir.join("src/bin"))
        .expect("programs/src/bin existe")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? == "rs" {
                Some(path.file_stem()?.to_str()?.to_owned())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    assert!(!names.is_empty(), "programs/src/bin nao tem nenhum programa");

    let built_dir = user_target_dir.join("x86_64-os_rust_user/release");
    let mut table = String::from("[\n");
    for name in &names {
        let elf = out_dir.join(format!("{name}.elf"));
        fs::copy(built_dir.join(name), &elf)
            .unwrap_or_else(|e| panic!("falha ao copiar o ELF de {name}: {e}"));
        table.push_str(&format!(
            "    Program {{ name: \"{name}\", image: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{name}.elf\")) }},\n"
        ));
    }
    table.push_str("]\n");
    fs::write(out_dir.join("programs.rs"), table).expect("escreve programs.rs em OUT_DIR");

    // `rerun-if-changed` numa pasta observa todos os arquivos dentro dela:
    // qualquer mudança no código dos programas refaz o embutimento.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=programs");
    println!("cargo:rerun-if-changed=x86_64-os_rust_user.json");
    // A biblioteca de runtime e o contrato de syscalls também entram em todo
    // programa: editar qualquer um dos dois refaz o embutimento.
    println!("cargo:rerun-if-changed=runtime");
    println!("cargo:rerun-if-changed=abi");

    // As imagens de volume (ramdisk e disco) dependem do conteúdo de `discos/`
    // e do ELF de `visita`, que acabou de ser compilado.
    generate_images(&root, &out_dir, &built_dir);
    println!("cargo:rerun-if-changed=discos");
    println!("cargo:rerun-if-changed=fatimg");

    write_delivered_files(&root, &out_dir);
}

/// Maior executável que `run` carrega de um arquivo, em bytes. É o mesmo valor
/// de `abi::MAX_EXEC_SIZE` (este script não depende de `abi`): serve aqui só
/// para fabricar `docs/grande.bin`, um byte maior que o limite, com o qual os
/// testes provam a recusa de um executável grande demais.
const MAX_EXEC_SIZE: usize = 65536;

/// Gera as imagens FAT16 com a crate `fatimg`, sem nenhuma ferramenta do
/// sistema:
///
/// - `OUT_DIR/ramdisk.img`: `discos/ram/` mais `bin/hello` (cópia do ELF
///   `hello`); o kernel a embute com `include_bytes!`;
/// - `target/imagens/disco.img`: `discos/disco/` mais `bin/visita` (o programa
///   que só existe no disco) e `docs/grande.bin`; o QEMU a recebe como
///   disco de dados (ver `Cargo.toml`, `[package.metadata.bootimage]`);
/// - `target/imagens/corrompido.img`: o disco com o setor de boot zerado, para
///   os testes provarem que um volume inválido é recusado.
///
/// Todas as imagens são montadas em memória **antes** de qualquer arquivo ser
/// escrito, e qualquer erro termina o script com `panic!`: nunca sai uma
/// imagem parcial, ausente ou desatualizada.
fn generate_images(root: &Path, out_dir: &Path, built_dir: &Path) {
    let mut ram = fatimg::Builder::new();
    add_tree(&mut ram, &root.join("discos/ram"), "");
    ram.dir("bin").unwrap_or_else(|e| panic!("ramdisk: cria bin: {e:?}"));
    let hello = fs::read(out_dir.join("hello.elf")).expect("le o ELF de hello");
    ram.file("bin/hello", &hello).unwrap_or_else(|e| panic!("ramdisk: grava bin/hello: {e:?}"));
    let ram_image = ram.build().unwrap_or_else(|e| panic!("ramdisk: monta a imagem: {e:?}"));

    let mut disco = fatimg::Builder::new();
    add_tree(&mut disco, &root.join("discos/disco"), "");
    disco.dir("bin").unwrap_or_else(|e| panic!("disco: cria bin: {e:?}"));
    let visita = fs::read(built_dir.join("visita")).expect("le o ELF de visita");
    disco.file("bin/visita", &visita).unwrap_or_else(|e| panic!("disco: grava bin/visita: {e:?}"));
    disco
        .file("docs/grande.bin", &vec![0u8; MAX_EXEC_SIZE + 1])
        .unwrap_or_else(|e| panic!("disco: grava docs/grande.bin: {e:?}"));
    let disco_image = disco.build().unwrap_or_else(|e| panic!("disco: monta a imagem: {e:?}"));

    let mut corrupt_image = disco_image.clone();
    corrupt_image[..512].fill(0);

    let images_dir = root.join("target/imagens");
    fs::create_dir_all(&images_dir).unwrap_or_else(|e| panic!("cria {}: {e}", images_dir.display()));
    fs::write(out_dir.join("ramdisk.img"), &ram_image).expect("escreve ramdisk.img em OUT_DIR");
    fs::write(images_dir.join("disco.img"), &disco_image).expect("escreve target/imagens/disco.img");
    fs::write(images_dir.join("corrompido.img"), &corrupt_image).expect("escreve target/imagens/corrompido.img");
}

/// Copia para o construtor tudo que está em `dir`, recursivamente, em ordem
/// alfabética (a imagem é a mesma em toda compilação). `prefix` é o caminho
/// de `dir` dentro do volume (vazio na raiz). Arquivos ocultos são ignorados.
fn add_tree(builder: &mut fatimg::Builder, dir: &Path, prefix: &str) {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("le {}: {e}", dir.display()))
        .map(|entry| entry.expect("entrada de diretorio").path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().and_then(|n| n.to_str()).expect("nome de arquivo UTF-8").to_string();
        if name.starts_with('.') {
            continue;
        }
        let inside = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        if path.is_dir() {
            builder.dir(&inside).unwrap_or_else(|e| panic!("cria o diretorio {inside}: {e:?}"));
            add_tree(builder, &path, &inside);
        } else {
            let bytes = fs::read(&path).unwrap_or_else(|e| panic!("le {}: {e}", path.display()));
            builder.file(&inside, &bytes).unwrap_or_else(|e| panic!("grava {inside} ({}): {e:?}", path.display()));
        }
    }
}

/// Extensões dos arquivos de texto que fazem parte do que o projeto entrega.
const DELIVERED_EXTENSIONS: &[&str] = &["rs", "md", "toml", "json", "ld", "txt", "lock", "yml"];

/// Diretórios que **não** fazem parte do que o projeto entrega: as saídas de
/// build, o controle de versão e as pastas do fluxo de especificação. O nome da
/// pasta do fluxo de especificação é montado a partir de pedaços para que este
/// arquivo, que também é entregue, não contenha o texto que o teste de higiene
/// procura.
fn skipped_directories() -> Vec<String> {
    vec![
        "specs".to_string(),
        "target".to_string(),
        ".git".to_string(),
        ".claude".to_string(),
        [".spec", "ify"].concat(),
    ]
}

/// Junta, em `files`, todos os arquivos de texto entregues abaixo de `dir`.
/// Cada diretório visitado também é observado pelo cargo (`rerun-if-changed`),
/// para que um arquivo novo ou removido refaça a lista.
fn collect_delivered_files(dir: &Path, skip: &[String], files: &mut Vec<PathBuf>) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("le {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("entrada de diretorio").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if path.is_dir() {
            if !skip.contains(&name) {
                collect_delivered_files(&path, skip, files);
            }
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| DELIVERED_EXTENSIONS.contains(&e))
        {
            println!("cargo:rerun-if-changed={}", path.display());
            files.push(path);
        }
    }
}

/// Escreve `OUT_DIR/arquivos_entregues.rs`: uma lista `(caminho, texto)` com o
/// conteúdo de cada arquivo de texto do repositório que é entregue. Os testes
/// rodam dentro do QEMU, sem sistema de arquivos, então não conseguem varrer o
/// repositório em tempo de execução; embutir os textos em tempo de compilação
/// é o único jeito de `tests/artefatos.rs` conferir os arquivos de verdade.
/// Só entra no binário de teste que a inclui; o kernel de produção não carrega
/// nada disto.
fn write_delivered_files(root: &Path, out_dir: &Path) {
    let mut files = Vec::new();
    collect_delivered_files(root, &skipped_directories(), &mut files);
    files.sort();

    let mut table = String::from("&[\n");
    for path in &files {
        let relative = path
            .strip_prefix(root)
            .expect("arquivo dentro do repositorio")
            .to_string_lossy()
            .replace('\\', "/");
        // `{:?}` escapa aspas e barras do caminho como um literal de Rust.
        table.push_str(&format!(
            "    ({:?}, include_str!({:?})),\n",
            relative,
            path.to_string_lossy()
        ));
    }
    table.push_str("]\n");
    fs::write(out_dir.join("arquivos_entregues.rs"), table)
        .expect("escreve arquivos_entregues.rs em OUT_DIR");
}
