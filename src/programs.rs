//! Tabela dos programas de usuário embutidos na imagem de boot.
//!
//! Não há sistema de arquivos: cada programa é um ELF64 estático compilado
//! pela crate `programs/` (para o target de usuário) e embutido no kernel em
//! tempo de compilação. A tabela abaixo é gerada pelo `build.rs` da raiz
//! (`OUT_DIR/programs.rs`), uma entrada por arquivo em `programs/src/bin/`,
//! ordenada por nome: acrescentar um programa é criar um arquivo, sem
//! editar esta lista.

/// Um programa embutido: o nome (argumento de `run`) e a imagem ELF.
pub struct Program {
    pub name: &'static str,
    pub image: &'static [u8],
}

/// Todos os programas embutidos, em ordem alfabética.
pub static PROGRAMS: &[Program] = &include!(concat!(env!("OUT_DIR"), "/programs.rs"));
