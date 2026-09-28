// Passa ao linker o linker script dos programas de usuário (base em
// 0x4000_0000, seções em páginas separadas). Fica aqui, e não em flags do
// cargo, para valer para todo binário deste pacote sem depender de
// configuração local: o `build.rs` do kernel só precisa rodar
// `cargo build` neste diretório.
fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR definido pelo cargo");
    println!("cargo:rustc-link-arg=-T{manifest}/link.ld");
    println!("cargo:rerun-if-changed=link.ld");
}
