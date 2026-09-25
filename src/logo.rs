//! Texto canônico do logo, incluído direto do arquivo-fonte em tempo de
//! compilação — sem nenhum escape de Rust, então este texto é byte a byte
//! igual ao do `README.md` (FR-008).

/// 20 linhas, no máximo 45 colunas, sem espaços no final de linha.
/// Desenhado na tela por `vga_buffer::draw_logo()`.
pub const LOGO: &str = include_str!("logo.txt");

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn logo_tem_vinte_linhas_e_no_maximo_oitenta_colunas() {
        let mut total_linhas = 0;
        for linha in LOGO.lines() {
            total_linhas += 1;
            assert!(linha.chars().count() <= 80);
        }
        assert_eq!(total_linhas, 20);
    }
}
