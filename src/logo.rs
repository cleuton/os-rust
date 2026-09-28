//! Logo ASCII de 20 linhas, incluído direto de `logo.txt` em tempo de
//! compilação via `include_str!`, em vez de uma string literal Rust —
//! o logo usa muitas barras invertidas (`\`), e uma string literal
//! exigiria escapar cada uma delas (`\\`), o que deixaria o texto no
//! `.rs` diferente do texto real do logo. `include_str!` lê o arquivo
//! como está, então o texto aqui é byte a byte igual ao de `logo.txt` e
//! ao do `README.md`.

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
