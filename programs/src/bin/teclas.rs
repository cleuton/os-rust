//! `teclas`: a demonstração do teclado cru, da tela de texto com cores, dos
//! argumentos de linha de comando e do arquivo regravável.
//!
//! A tela é limpa e o programa mostra, em vídeo reverso (texto preto sobre
//! fundo cinza claro), uma barra de título. Cada tecla lida (setas, Home, Ctrl,
//! qualquer uma) aparece pelo nome e entra na contagem; as setas movem um `@`
//! verde pela área do meio da tela. `Ctrl+Q` sai: o programa regrava um
//! arquivo com o total de teclas lidas (`FileWriter::overwrite` zera o arquivo,
//! então regravar nunca acrescenta) e mostra o resultado mais embaixo.
//!
//! O arquivo vem dos argumentos, que só o comando `exec` passa:
//!
//! ```text
//! os-rust> run teclas                      (grava /ram/teclas.txt)
//! os-rust> exec teclas /ram/contagem.txt   (grava o arquivo dado)
//! ```
//!
//! Quem usa a tela deve rodar **sozinho**: o kernel só tem um cursor. O kernel
//! não apaga nada sozinho, por isso cada texto é completado com espaços até a
//! largura em que foi escrito, para não sobrar resto da tecla anterior.
//!
//! Compilado à parte do kernel e embutido na imagem de boot.

#![no_std]
#![no_main]

use core::fmt::Write;

use runtime::screen::{self, COLOR_BLACK, COLOR_LIGHT_GRAY, COLOR_LIGHT_GREEN};
use runtime::{args, entry, print, println, read_key, FileWriter, FsError, Key, KeyEvent};

entry!(main);

/// O arquivo que o programa regrava quando não recebe outro.
const ARQUIVO_PADRAO: &str = "/ram/teclas.txt";
/// O maior caminho aceito: com ele, a linha `teclas: gravado em <arquivo> (N
/// bytes)` ainda cabe nas 80 colunas da tela.
const MAX_CAMINHO: usize = 40;
/// A barra de título, escrita em vídeo reverso na linha 0.
const TITULO: &str = "teclas: setas movem o @, Ctrl+Q sai";
/// Onde o programa mostra a última tecla e a contagem.
const LINHA_TECLA: usize = 2;
const LINHA_CONTAGEM: usize = 3;
/// Largura em que essas duas linhas são reescritas (apaga o resto da anterior).
const LARGURA_STATUS: usize = 40;
/// Onde o `@` pode andar (linhas 5 a 18, colunas 0 a 79) e onde ele começa.
const PRIMEIRA_LINHA: usize = 5;
const ULTIMA_LINHA: usize = 18;
const INICIO: (usize, usize) = (12, 40);
/// Onde o resultado é mostrado ao sair (o que vem antes não é apagado).
const LINHA_RESULTADO: usize = 20;

/// Um texto curto montado na pilha, para completar com espaços antes de
/// escrever (sem usar o heap).
struct Texto {
    bytes: [u8; 96],
    len: usize,
}

impl Texto {
    fn new() -> Texto {
        Texto { bytes: [0; 96], len: 0 }
    }

    fn como_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl Write for Texto {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let livre = self.bytes.len() - self.len;
        let n = s.len().min(livre);
        self.bytes[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// O nome da tecla, como aparece na linha 2.
fn nome_da_tecla(evento: KeyEvent, saida: &mut Texto) {
    if evento.ctrl {
        let _ = saida.write_str("Ctrl+");
    }
    let _ = match evento.key {
        // Com Ctrl, uma letra chega minúscula e aparece maiúscula (`Ctrl+Q`).
        Key::Char(c) if evento.ctrl => saida.write_char(c.to_ascii_uppercase()),
        Key::Char(c) => write!(saida, "caractere '{}'", c),
        Key::Enter => saida.write_str("Enter"),
        Key::Backspace => saida.write_str("Backspace"),
        Key::Tab => saida.write_str("Tab"),
        Key::Esc => saida.write_str("Esc"),
        Key::Up => saida.write_str("seta acima"),
        Key::Down => saida.write_str("seta abaixo"),
        Key::Left => saida.write_str("seta esquerda"),
        Key::Right => saida.write_str("seta direita"),
        Key::Home => saida.write_str("Home"),
        Key::End => saida.write_str("End"),
        Key::PageUp => saida.write_str("Page Up"),
        Key::PageDown => saida.write_str("Page Down"),
        Key::Delete => saida.write_str("Delete"),
        Key::Insert => saida.write_str("Insert"),
    };
}

/// Vai para o começo de `linha`. As linhas usadas estão sempre dentro da tela,
/// então a recusa (`OutOfRange`) não acontece; ignorá-la deixa o cursor onde
/// estava, que é o pior caso inofensivo.
fn vai_para(linha: usize, coluna: usize) {
    let _ = screen::move_to(linha, coluna);
}

/// Reescreve a linha `numero` com `texto`, completada com espaços até
/// `largura` colunas.
fn escreve_linha(numero: usize, texto: &str, largura: usize) {
    vai_para(numero, 0);
    print!("{:<1$}", texto, largura);
}

/// As duas linhas de status: a última tecla e quantas já foram lidas.
fn mostra_status(tecla: &str, lidas: u32) {
    let mut linha = Texto::new();
    let _ = write!(linha, "ultima tecla: {}", tecla);
    escreve_linha(LINHA_TECLA, linha.como_str(), LARGURA_STATUS);
    let mut contagem = Texto::new();
    let _ = write!(contagem, "teclas lidas: {}", lidas);
    escreve_linha(LINHA_CONTAGEM, contagem.como_str(), LARGURA_STATUS);
}

/// Escreve o `@` em verde claro sobre preto na posição dada.
fn desenha_arroba((linha, coluna): (usize, usize)) {
    let _ = screen::set_color(COLOR_LIGHT_GREEN, COLOR_BLACK);
    vai_para(linha, coluna);
    print!("@");
    screen::reset_color();
}

/// Apaga a posição com um espaço, na cor padrão.
fn apaga((linha, coluna): (usize, usize)) {
    vai_para(linha, coluna);
    print!(" ");
}

/// Para onde a seta leva o `@`, sem sair da área (nas bordas, não se move).
fn proxima_posicao(tecla: Key, (linha, coluna): (usize, usize)) -> (usize, usize) {
    match tecla {
        Key::Up if linha > PRIMEIRA_LINHA => (linha - 1, coluna),
        Key::Down if linha < ULTIMA_LINHA => (linha + 1, coluna),
        Key::Left if coluna > 0 => (linha, coluna - 1),
        Key::Right if coluna + 1 < screen::COLS => (linha, coluna + 1),
        _ => (linha, coluna),
    }
}

/// Regrava `arquivo` com `<lidas>\n`, do zero, e devolve quantos bytes gravou.
fn grava(arquivo: &str, lidas: u32) -> Result<usize, FsError> {
    let mut texto = Texto::new();
    let _ = writeln!(texto, "{}", lidas);
    let mut destino = FileWriter::overwrite(arquivo)?;
    destino.write_all(texto.como_str().as_bytes())?;
    Ok(texto.len)
}

fn main() -> i32 {
    // Os argumentos chegam como um texto só; sem argumento vale o padrão, com
    // uma palavra é o arquivo, com mais de uma é erro de uso. Tudo isso é
    // decidido **antes** de mexer na tela.
    let mut area = [0u8; args::MAX_ARGS_LEN];
    let texto = args::read(&mut area);
    let mut palavras = args::words(texto);
    let arquivo = match (palavras.next(), palavras.next()) {
        (None, _) => ARQUIVO_PADRAO,
        (Some(arquivo), None) => arquivo,
        (Some(_), Some(_)) => {
            println!("teclas: uso: teclas [arquivo]");
            return 1;
        }
    };
    if arquivo.len() > MAX_CAMINHO {
        println!("teclas: caminho muito longo (maximo {} bytes)", MAX_CAMINHO);
        return 1;
    }

    screen::clear();
    // A barra de título em vídeo reverso, completada até as 80 colunas.
    let _ = screen::set_color(COLOR_BLACK, COLOR_LIGHT_GRAY);
    escreve_linha(0, TITULO, screen::COLS);
    screen::reset_color();
    mostra_status("(nenhuma)", 0);
    let mut posicao = INICIO;
    desenha_arroba(posicao);

    let mut lidas: u32 = 0;
    loop {
        let evento = read_key();
        lidas += 1;
        let nova = proxima_posicao(evento.key, posicao);
        if nova != posicao {
            apaga(posicao);
            desenha_arroba(nova);
            posicao = nova;
        }
        let mut nome = Texto::new();
        nome_da_tecla(evento, &mut nome);
        mostra_status(nome.como_str(), lidas);
        // `Ctrl+Q` também conta e aparece antes de sair.
        if evento.ctrl && evento.key == Key::Char('q') {
            break;
        }
    }

    // Tenta regravar; mostra o resultado mais embaixo, sem apagar a tela.
    let resultado = grava(arquivo, lidas);
    vai_para(LINHA_RESULTADO, 0);
    print!("teclas: {} teclas lidas", lidas);
    vai_para(LINHA_RESULTADO + 1, 0);
    match resultado {
        Ok(bytes) => {
            print!("teclas: gravado em {} ({} bytes)", arquivo, bytes);
            0
        }
        Err(erro) => {
            print!("teclas: nao gravou {}: {}", arquivo, erro);
            1
        }
    }
}
