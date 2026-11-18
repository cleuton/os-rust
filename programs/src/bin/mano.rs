//! `mano`: um editor de textos de tela cheia, simples, no jeito do `nano`.
//!
//! A tela de 25 por 80 tem cinco regiões: a linha 0 é a barra de título (em
//! vídeo reverso), as linhas 1 a 21 mostram o texto, a linha 22 guarda
//! mensagens e perguntas, e as linhas 23 e 24 são os dois quadros de atalhos.
//! A pessoa edita com setas e teclado, corta e cola linhas, busca, substitui,
//! grava com `Ctrl+O` e sai com `Ctrl+X`. `Ctrl+G` mostra a ajuda.
//!
//! ```text
//! os-rust> run mano                      (texto novo, sem nome)
//! os-rust> exec mano /ram/nota.txt       (abre o arquivo, ou começa um novo)
//! ```
//!
//! O texto é uma lista de linhas (`Vec<Vec<u8>>`), cada uma sem o `\n`. Ao
//! abrir, o arquivo é dividido em cada `\n`; ao gravar, as linhas são unidas
//! por `\n` e, se a última não é vazia, ganha-se um `\n` no fim. Assim todo
//! arquivo vazio ou terminado em `\n` volta idêntico, byte a byte.
//!
//! Quem usa a tela deve rodar **sozinho**: o kernel só tem um cursor. O kernel
//! não apaga nada sozinho, por isso toda linha é redesenhada inteira, a cada
//! tecla, completada com espaços até a coluna 79. A linha 24 nunca recebe mais
//! de 80 bytes: o 81º byte rolaria a tela.
//!
//! Compilado à parte do kernel e embutido na imagem de boot.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use runtime::screen::{
    self, COLOR_BLACK, COLOR_DEFAULT_BACKGROUND, COLOR_DEFAULT_TEXT, COLOR_LIGHT_GRAY,
};
use runtime::sys::write;
use runtime::{args, entry, println, read_key, File, FileWriter, FsError, Key, KeyEvent};

entry!(main);

/// O texto tem no máximo 65536 bytes **gravados** (contando um `\n` por linha,
/// como o arquivo ficaria) e 4096 linhas. Cabe no heap de 1 MiB com folga.
const MAX_BYTES: usize = 65536;
const MAX_LINHAS: usize = 4096;
/// O maior caminho aceito (para as mensagens caberem nas 80 colunas).
const MAX_CAMINHO: usize = 40;
/// O maior campo digitado num prompt.
const MAX_CAMPO: usize = 40;
/// Linhas da tela reservadas ao texto (as linhas 1 a 21).
const LINHAS_TEXTO: usize = 21;
/// Colunas da tela.
const LARGURA: usize = 80;
/// Quantas linhas andam `PageUp` e `PageDown`.
const PAGINA: usize = 20;
/// A tabulação insere espaços até a próxima coluna múltipla disto.
const TAB: usize = 4;
/// O último termo buscado aparece no rótulo cortado neste tamanho.
const ULTIMO_NO_ROTULO: usize = 20;
/// Onde ficam as mensagens e os quadros de atalhos.
const LINHA_MENSAGEM: usize = 22;
const LINHA_ATALHOS: usize = 23;
/// Largura de cada atalho dos quadros (a tecla, um espaço, o rótulo).
const LARGURA_ATALHO: usize = 13;

/// Cores (texto, fundo): o texto e os rótulos, o vídeo reverso (preto sobre
/// cinza claro) e as mensagens (o amarelo sobre preto de sempre).
const CORPO: (u8, u8) = (COLOR_LIGHT_GRAY, COLOR_BLACK);
const REVERSO: (u8, u8) = (COLOR_BLACK, COLOR_LIGHT_GRAY);
const MENSAGEM: (u8, u8) = (COLOR_DEFAULT_TEXT, COLOR_DEFAULT_BACKGROUND);

/// O aviso de caminho relativo: como erro de uso (depois de `mano: `) e como
/// mensagem da linha 22.
const SO_ABSOLUTO: &str = "use um caminho absoluto, como /ram/nota.txt";
const SO_ABSOLUTO_NA_TELA: &str = "Use um caminho absoluto, como /ram/nota.txt";

/// Os atalhos dos dois quadros (tecla, rótulo), 6 por linha.
const ATALHOS_1: [(&str, &str); 6] = [
    ("^G", "Ajuda"),
    ("^O", "Gravar"),
    ("^W", "Buscar"),
    ("^K", "Cortar"),
    ("^C", "Posicao"),
    ("^X", "Sair"),
];
const ATALHOS_2: [(&str, &str); 6] = [
    ("^R", "Ler"),
    ("^\\", "Subst."),
    ("^U", "Colar"),
    ("^_", "Linha"),
    ("^Y", "Pag ant"),
    ("^V", "Pag seg"),
];

/// A tela de ajuda: uma linha por linha da tabela de teclas.
const AJUDA: [(&str, &str); 21] = [
    ("letra, numero, simbolo", "insere o caractere"),
    ("Enter", "parte a linha"),
    ("Backspace, Ctrl+H", "apaga a esquerda (junta linhas)"),
    ("Delete, Ctrl+D", "apaga sob o cursor (junta linhas)"),
    ("Tab", "insere espacos ate coluna multipla de 4"),
    ("setas, Ctrl+B, Ctrl+F, Ctrl+P, Ctrl+N", "movem o cursor"),
    ("Home, Ctrl+A", "comeco da linha"),
    ("End, Ctrl+E", "fim da linha"),
    ("PageUp, Ctrl+Y", "20 linhas para cima"),
    ("PageDown, Ctrl+V", "20 linhas para baixo"),
    ("Ctrl+_, Ctrl+/", "ir para a linha"),
    ("Ctrl+C", "mostra a posicao do cursor"),
    ("Ctrl+K", "corta a linha"),
    ("Ctrl+U", "cola as linhas cortadas"),
    ("Ctrl+W", "buscar"),
    ("Ctrl+\\, Ctrl+T", "buscar e substituir"),
    ("Ctrl+R", "inserir um arquivo no cursor"),
    ("Ctrl+O", "gravar (pergunta o nome)"),
    ("Ctrl+S", "gravar (direto, se ja tem nome)"),
    ("Ctrl+X", "sair"),
    ("Ctrl+G", "esta ajuda"),
];

/// O trecho do texto que aparece em vídeo reverso (uma busca que achou, ou a
/// ocorrência em pergunta na substituição). Posição **no texto**, não na tela.
#[derive(Clone, Copy)]
struct Destaque {
    linha: usize,
    coluna: usize,
    tamanho: usize,
}

/// O que `trata` diz ao laço principal.
enum Fluxo {
    Continua,
    Sai,
}

/// Qual limite do texto uma operação estouraria.
enum Limite {
    Bytes,
    Linhas,
}

/// A resposta a uma pergunta de uma tecla.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Resposta {
    Sim,
    Nao,
    Todos,
    Cancela,
}

/// Qual quadro de atalhos desenhar.
#[derive(Clone, Copy)]
enum Rodape {
    Normal,
    Campo,
    Pergunta,
}

/// Por que um arquivo não abriu.
enum ErroAbrir {
    NaoExiste,
    Fs(FsError),
    Grande,
}

/// O estado do editor: o texto, o cursor, as janelas e o que mais é preciso
/// para desenhar a tela e tratar a próxima tecla.
struct Editor {
    /// O texto: cada linha sem o `\n`. Nunca fica vazio (sempre há uma linha).
    linhas: Vec<Vec<u8>>,
    /// A soma dos tamanhos de todas as linhas (para medir o texto sem somar de novo).
    soma: usize,
    /// A linha e a coluna (em bytes) do cursor.
    cy: usize,
    cx: usize,
    /// A coluna a que `Up` e `Down` tentam voltar.
    desejada: usize,
    /// A primeira linha e a primeira coluna visíveis.
    topo: usize,
    esquerda: usize,
    /// O caminho atual, se o texto tem nome.
    nome: Option<String>,
    /// O texto mudou desde a última gravação.
    modificado: bool,
    /// O texto da linha 22, até a próxima tecla.
    mensagem: String,
    /// O último termo buscado ou substituído.
    ultimo: Option<Vec<u8>>,
    /// As linhas cortadas por `Ctrl+K`.
    area: Vec<Vec<u8>>,
    /// A tecla anterior foi um `Ctrl+K` que cortou (os cortes seguidos acumulam).
    corte_seguido: bool,
    /// O trecho em vídeo reverso, até a próxima tecla.
    destaque: Option<Destaque>,
}

// ---------------------------------------------------------------------------
// Texto puro: dividir, juntar, medir e procurar. Não tocam a tela nem o teclado.
// ---------------------------------------------------------------------------

/// Divide os bytes de um arquivo em cada `\n`, sem descartar nada: `a\nb\n`
/// vira `a`, `b` e uma linha vazia; um arquivo vazio vira uma linha vazia.
fn divide(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut linhas: Vec<Vec<u8>> = bytes.split(|&b| b == b'\n').map(|l| l.to_vec()).collect();
    linhas.shrink_to_fit();
    linhas
}

/// Une as linhas por `\n` e, se a última não é vazia, acrescenta um `\n`. É a
/// inversa de [`divide`] para todo arquivo vazio ou terminado em `\n`.
fn junta(linhas: &[Vec<u8>]) -> Vec<u8> {
    let soma: usize = linhas.iter().map(|l| l.len()).sum();
    let ultima_vazia = linhas.last().map_or(true, |l| l.is_empty());
    let mut bytes = Vec::with_capacity(medida(soma, linhas.len(), ultima_vazia));
    for (i, linha) in linhas.iter().enumerate() {
        if i > 0 {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(linha);
    }
    if !ultima_vazia {
        bytes.push(b'\n');
    }
    bytes
}

/// O tamanho que o texto teria gravado: os bytes das linhas, um `\n` entre
/// elas e outro no fim se a última linha não é vazia. Um texto de uma linha
/// vazia mede 0.
fn medida(soma: usize, linhas: usize, ultima_vazia: bool) -> usize {
    soma + linhas.saturating_sub(1) + if ultima_vazia { 0 } else { 1 }
}

/// Confere os dois limites sobre o texto que **resultaria** de uma operação.
fn cabe(soma: usize, linhas: usize, ultima_vazia: bool) -> Result<(), Limite> {
    if linhas > MAX_LINHAS {
        Err(Limite::Linhas)
    } else if medida(soma, linhas, ultima_vazia) > MAX_BYTES {
        Err(Limite::Bytes)
    } else {
        Ok(())
    }
}

/// A primeira ocorrência de `termo` em `linha` a partir da coluna `desde`.
fn acha(linha: &[u8], termo: &[u8], desde: usize) -> Option<usize> {
    if termo.is_empty() || desde > linha.len() || termo.len() > linha.len() - desde {
        return None;
    }
    (desde..=linha.len() - termo.len()).find(|&i| &linha[i..i + termo.len()] == termo)
}

/// Um caminho tem de começar com `/` (não há diretório atual).
fn nome_valido(nome: &str) -> bool {
    nome.starts_with('/')
}

/// A mensagem de um limite estourado.
fn texto_do_limite(limite: Limite) -> String {
    match limite {
        Limite::Bytes => format!("Limite de {} bytes atingido", MAX_BYTES),
        Limite::Linhas => format!("Limite de {} linhas atingido", MAX_LINHAS),
    }
}

// ---------------------------------------------------------------------------
// Arquivos: ler inteiro (e fechar) e abrir.
// ---------------------------------------------------------------------------

/// Lê o arquivo inteiro. Recusa o que passa de 65536 bytes sem ler o resto. O
/// arquivo fecha ao sair desta função, antes de qualquer gravação.
fn le_arquivo(caminho: &str) -> Result<Vec<u8>, ErroAbrir> {
    let mut arquivo = match File::open(caminho) {
        Ok(arquivo) => arquivo,
        Err(FsError::NotFound) => return Err(ErroAbrir::NaoExiste),
        Err(erro) => return Err(ErroAbrir::Fs(erro)),
    };
    let mut dados = Vec::new();
    let mut pedaco = [0u8; 4096];
    loop {
        // Abrir um diretório funciona; é a primeira leitura que dá `WrongType`.
        let lidos = arquivo.read(&mut pedaco).map_err(ErroAbrir::Fs)?;
        if lidos == 0 {
            return Ok(dados);
        }
        if dados.len() + lidos > MAX_BYTES {
            return Err(ErroAbrir::Grande);
        }
        dados.extend_from_slice(&pedaco[..lidos]);
    }
}

/// Abre o arquivo como lista de linhas e confere os limites sobre o texto que
/// seria gravado (o editor nunca abre o que não poderia regravar).
fn abre(caminho: &str) -> Result<Vec<Vec<u8>>, ErroAbrir> {
    let dados = le_arquivo(caminho)?;
    let linhas = divide(&dados);
    drop(dados);
    let soma: usize = linhas.iter().map(|l| l.len()).sum();
    let ultima_vazia = linhas.last().map_or(true, |l| l.is_empty());
    match cabe(soma, linhas.len(), ultima_vazia) {
        Ok(()) => Ok(linhas),
        Err(_) => Err(ErroAbrir::Grande),
    }
}

/// Grava `bytes` em `nome`, do zero (o arquivo é zerado antes).
fn grava_arquivo(nome: &str, bytes: &[u8]) -> Result<(), FsError> {
    let mut destino = FileWriter::overwrite(nome)?;
    destino.write_all(bytes)
}

// ---------------------------------------------------------------------------
// Tela: tudo sai por `write`, em bytes crus (o arquivo pode ter bytes que não
// são texto; o kernel mostra o que não é ASCII imprimível como o quadrado 0xfe).
// ---------------------------------------------------------------------------

/// Escreve bytes na tela. Um erro de escrita é ignorado, como no `print!`.
fn escreve(bytes: &[u8]) {
    if !bytes.is_empty() {
        let _ = write(bytes);
    }
}

/// Vai para `(linha, coluna)`. As posições usadas estão sempre dentro da tela.
fn vai_para(linha: usize, coluna: usize) {
    let _ = screen::move_to(linha, coluna);
}

/// Escolhe a cor (texto, fundo) do que for escrito a seguir.
fn cor((texto, fundo): (u8, u8)) {
    let _ = screen::set_color(texto, fundo);
}

/// Uma linha de 80 bytes só de espaços.
fn em_branco() -> [u8; LARGURA] {
    [b' '; LARGURA]
}

/// Copia `texto` para o começo de `destino`, cortando o que não cabe.
fn copia(destino: &mut [u8], texto: &[u8]) {
    let n = texto.len().min(destino.len());
    destino[..n].copy_from_slice(&texto[..n]);
}

/// A linha 22: a mensagem, ou o rótulo e o campo de um prompt.
fn desenha_mensagem(texto: &[u8]) {
    let mut linha = em_branco();
    copia(&mut linha, texto);
    cor(MENSAGEM);
    vai_para(LINHA_MENSAGEM, 0);
    escreve(&linha);
}

/// Um quadro de atalhos: cada atalho ocupa 13 colunas, a tecla em vídeo reverso
/// e o rótulo no corpo; o que sobra até a coluna 79 é espaço. A linha 24 sai
/// com exatamente 80 bytes.
fn desenha_quadro(linha: usize, atalhos: &[(&str, &str)]) {
    vai_para(linha, 0);
    for (tecla, rotulo) in atalhos {
        cor(REVERSO);
        escreve(tecla.as_bytes());
        let largura = LARGURA_ATALHO - tecla.len();
        let mut resto = [b' '; LARGURA_ATALHO];
        copia(&mut resto[1..largura], rotulo.as_bytes());
        cor(CORPO);
        escreve(&resto[..largura]);
    }
    let sobra = LARGURA - atalhos.len() * LARGURA_ATALHO;
    cor(CORPO);
    escreve(&em_branco()[..sobra]);
}

/// Os dois quadros de atalhos (linhas 23 e 24).
fn desenha_rodape(rodape: Rodape) {
    match rodape {
        Rodape::Normal => {
            desenha_quadro(LINHA_ATALHOS, &ATALHOS_1);
            desenha_quadro(LINHA_ATALHOS + 1, &ATALHOS_2);
        }
        Rodape::Campo => {
            desenha_quadro(LINHA_ATALHOS, &[("^C", "Cancelar")]);
            desenha_quadro(LINHA_ATALHOS + 1, &[("Enter", "Aceitar")]);
        }
        Rodape::Pergunta => {
            desenha_quadro(LINHA_ATALHOS, &[("^C", "Cancelar")]);
            desenha_quadro(LINHA_ATALHOS + 1, &[]);
        }
    }
}

/// Escreve uma linha de ajuda (atalho, e o que faz) na linha `numero`.
fn desenha_linha_de_ajuda(numero: usize, atalho: &str, efeito: &str) {
    let mut linha = em_branco();
    copia(&mut linha, atalho.as_bytes());
    copia(&mut linha[40..], efeito.as_bytes());
    cor(CORPO);
    vai_para(numero, 0);
    escreve(&linha);
}

/// A tela de `Ctrl+G`: o título, uma linha por atalho e o aviso no pé.
fn desenha_ajuda() {
    screen::clear();
    let mut titulo = em_branco();
    copia(&mut titulo[1..], b"Ajuda do mano");
    cor(REVERSO);
    vai_para(0, 0);
    escreve(&titulo);
    for (i, (atalho, efeito)) in AJUDA.iter().enumerate() {
        desenha_linha_de_ajuda(i + 1, atalho, efeito);
    }
    let mut pe = em_branco();
    copia(&mut pe[1..], b"Pressione qualquer tecla para voltar");
    cor(REVERSO);
    vai_para(LINHA_ATALHOS + 1, 0);
    escreve(&pe);
}

/// Escreve o texto de uma pergunta de erro de uso e devolve o código de saída.
fn uso(texto: &str) -> i32 {
    println!("mano: {}", texto);
    1
}

impl Editor {
    /// Um editor com o texto dado, o cursor no começo e nada modificado.
    fn novo(linhas: Vec<Vec<u8>>, nome: Option<String>) -> Editor {
        let soma = linhas.iter().map(|l| l.len()).sum();
        Editor {
            linhas,
            soma,
            cy: 0,
            cx: 0,
            desejada: 0,
            topo: 0,
            esquerda: 0,
            nome,
            modificado: false,
            mensagem: String::new(),
            ultimo: None,
            area: Vec::new(),
            corte_seguido: false,
            destaque: None,
        }
    }

    /// A última linha do texto é vazia?
    fn ultima_vazia(&self) -> bool {
        self.linhas.last().map_or(true, |l| l.is_empty())
    }

    /// Anda as janelas o mínimo para o cursor aparecer.
    fn rola(&mut self) {
        if self.cy < self.topo {
            self.topo = self.cy;
        } else if self.cy >= self.topo + LINHAS_TEXTO {
            self.topo = self.cy + 1 - LINHAS_TEXTO;
        }
        if self.cx < self.esquerda {
            self.esquerda = self.cx;
        } else if self.cx >= self.esquerda + LARGURA {
            self.esquerda = self.cx + 1 - LARGURA;
        }
    }

    // -- desenho -----------------------------------------------------------

    /// A linha 0: ` mano  `, o nome (ou `Novo buffer`) e `Modificado` à direita.
    fn desenha_titulo(&self) {
        let mut linha = em_branco();
        copia(&mut linha, b" mano  ");
        let nome = self.nome.as_deref().unwrap_or("Novo buffer");
        copia(&mut linha[7..], &nome.as_bytes()[..nome.len().min(MAX_CAMINHO)]);
        if self.modificado {
            copia(&mut linha[69..79], b"Modificado");
        }
        cor(REVERSO);
        vai_para(0, 0);
        escreve(&linha);
    }

    /// As linhas 1 a 21: a janela do texto, completada com espaços. O trecho em
    /// destaque sai em vídeo reverso, recortado na janela.
    fn desenha_texto(&self) {
        cor(CORPO);
        for i in 0..LINHAS_TEXTO {
            let numero = self.topo + i;
            let mut janela = em_branco();
            if let Some(linha) = self.linhas.get(numero) {
                if linha.len() > self.esquerda {
                    let fim = linha.len().min(self.esquerda + LARGURA);
                    copia(&mut janela, &linha[self.esquerda..fim]);
                }
            }
            vai_para(i + 1, 0);
            match self.destaque.filter(|d| d.linha == numero) {
                Some(d) => {
                    let ini = d.coluna.max(self.esquerda) - self.esquerda;
                    let fim = (d.coluna + d.tamanho).min(self.esquerda + LARGURA) - self.esquerda;
                    let (ini, fim) = (ini.min(LARGURA), fim.min(LARGURA).max(ini.min(LARGURA)));
                    escreve(&janela[..ini]);
                    cor(REVERSO);
                    escreve(&janela[ini..fim]);
                    cor(CORPO);
                    escreve(&janela[fim..]);
                }
                None => escreve(&janela),
            }
        }
    }

    /// Redesenha as 25 linhas, de cima para baixo, e leva o cursor ao lugar:
    /// no texto, ou no fim do campo da linha 22 (`no_campo`).
    fn desenha(&self, rodape: Rodape, linha22: &[u8], no_campo: bool) {
        self.desenha_titulo();
        self.desenha_texto();
        desenha_mensagem(linha22);
        desenha_rodape(rodape);
        if no_campo {
            vai_para(LINHA_MENSAGEM, linha22.len().min(LARGURA - 1));
        } else {
            let linha = self.cy.saturating_sub(self.topo).min(LINHAS_TEXTO - 1) + 1;
            let coluna = self.cx.saturating_sub(self.esquerda).min(LARGURA - 1);
            vai_para(linha, coluna);
        }
    }

    /// O desenho de sempre: a mensagem atual na linha 22.
    fn redesenha(&self) {
        self.desenha(Rodape::Normal, self.mensagem.as_bytes(), false);
    }

    // -- prompts -------------------------------------------------------------

    /// Um campo de texto na linha 22: o rótulo e o que se digita (até 40
    /// caracteres). `Enter` aceita; `Ctrl+C` e `Esc` cancelam (`None`).
    fn campo(&self, rotulo: &str, inicial: &str) -> Option<String> {
        let mut texto: Vec<u8> = inicial.bytes().collect();
        loop {
            let mut linha22: Vec<u8> = rotulo.bytes().collect();
            linha22.extend_from_slice(&texto);
            self.desenha(Rodape::Campo, &linha22, true);
            let evento = read_key();
            match (evento.key, evento.ctrl) {
                (Key::Enter, false) => return Some(String::from_utf8(texto).unwrap_or_default()),
                (Key::Esc, _) | (Key::Char('c'), true) => return None,
                (Key::Backspace, false) => {
                    texto.pop();
                }
                (Key::Char(c), false) if texto.len() < MAX_CAMPO => texto.push(c as u8),
                _ => {}
            }
        }
    }

    /// Uma pergunta de uma tecla. Só valem as letras de `aceitas` (em maiúscula
    /// ou minúscula) e `Ctrl+C`/`Esc`, que cancelam; qualquer outra tecla é
    /// ignorada e a pergunta continua.
    fn pergunta(&self, texto: &str, aceitas: &[u8]) -> Resposta {
        loop {
            self.desenha(Rodape::Pergunta, texto.as_bytes(), false);
            let evento = read_key();
            match (evento.key, evento.ctrl) {
                (Key::Esc, _) | (Key::Char('c'), true) => return Resposta::Cancela,
                (Key::Char(c), false) => {
                    let letra = c.to_ascii_lowercase() as u8;
                    if aceitas.contains(&letra) {
                        return match letra {
                            b's' => Resposta::Sim,
                            b'n' => Resposta::Nao,
                            _ => Resposta::Todos,
                        };
                    }
                }
                _ => {}
            }
        }
    }

    /// Pede o termo de uma busca ou substituição. O rótulo mostra o último
    /// termo entre colchetes; campo vazio usa o último (sem último, cancela).
    fn pede_termo(&mut self, base: &str) -> Option<Vec<u8>> {
        let rotulo = match &self.ultimo {
            Some(ultimo) => {
                let visto = &ultimo[..ultimo.len().min(ULTIMO_NO_ROTULO)];
                format!("{} [{}]: ", base, core::str::from_utf8(visto).unwrap_or(""))
            }
            None => format!("{}: ", base),
        };
        let digitado = self.campo(&rotulo, "")?;
        let termo = if digitado.is_empty() { self.ultimo.clone()? } else { digitado.into_bytes() };
        self.ultimo = Some(termo.clone());
        Some(termo)
    }

    // -- edição ----------------------------------------------------------------

    /// Mostra na linha 22 qual limite foi atingido; o texto não muda.
    fn recusa(&mut self, limite: Limite) {
        self.mensagem = texto_do_limite(limite);
    }

    /// Insere um caractere na posição do cursor.
    fn insere(&mut self, byte: u8) {
        let n = self.linhas.len();
        let ultima_vazia = if self.cy == n - 1 { false } else { self.ultima_vazia() };
        if let Err(limite) = cabe(self.soma + 1, n, ultima_vazia) {
            return self.recusa(limite);
        }
        self.linhas[self.cy].insert(self.cx, byte);
        self.soma += 1;
        self.cx += 1;
        self.desejada = self.cx;
        self.modificado = true;
    }

    /// `Tab`: espaços até a próxima coluna múltipla de 4 (nunca o byte `0x09`).
    fn tabula(&mut self) {
        let espacos = TAB - self.cx % TAB;
        let n = self.linhas.len();
        let ultima_vazia = if self.cy == n - 1 { false } else { self.ultima_vazia() };
        if let Err(limite) = cabe(self.soma + espacos, n, ultima_vazia) {
            return self.recusa(limite);
        }
        for _ in 0..espacos {
            self.linhas[self.cy].insert(self.cx, b' ');
            self.cx += 1;
        }
        self.soma += espacos;
        self.desejada = self.cx;
        self.modificado = true;
    }

    /// `Enter`: parte a linha no cursor; o resto vai para uma linha nova abaixo.
    fn parte_linha(&mut self) {
        let n = self.linhas.len();
        let resto_vazio = self.cx == self.linhas[self.cy].len();
        let ultima_vazia = if self.cy == n - 1 { resto_vazio } else { self.ultima_vazia() };
        if let Err(limite) = cabe(self.soma, n + 1, ultima_vazia) {
            return self.recusa(limite);
        }
        let resto = self.linhas[self.cy].split_off(self.cx);
        self.linhas.insert(self.cy + 1, resto);
        self.cy += 1;
        self.cx = 0;
        self.desejada = 0;
        self.modificado = true;
    }

    /// `Backspace`: apaga à esquerda; no começo da linha junta com a anterior.
    fn apaga_esquerda(&mut self) {
        if self.cx > 0 {
            self.linhas[self.cy].remove(self.cx - 1);
            self.soma -= 1;
            self.cx -= 1;
        } else if self.cy > 0 {
            let atual = self.linhas.remove(self.cy);
            self.cy -= 1;
            self.cx = self.linhas[self.cy].len();
            self.linhas[self.cy].extend_from_slice(&atual);
        } else {
            return;
        }
        self.desejada = self.cx;
        self.modificado = true;
    }

    /// `Delete`: apaga sob o cursor; no fim da linha junta com a seguinte.
    fn apaga_direita(&mut self) {
        if self.cx < self.linhas[self.cy].len() {
            self.linhas[self.cy].remove(self.cx);
            self.soma -= 1;
        } else if self.cy + 1 < self.linhas.len() {
            let proxima = self.linhas.remove(self.cy + 1);
            self.linhas[self.cy].extend_from_slice(&proxima);
        } else {
            return;
        }
        self.desejada = self.cx;
        self.modificado = true;
    }

    // -- movimento -----------------------------------------------------------

    fn esquerda(&mut self) {
        if self.cx > 0 {
            self.cx -= 1;
        } else if self.cy > 0 {
            self.cy -= 1;
            self.cx = self.linhas[self.cy].len();
        }
        self.desejada = self.cx;
    }

    fn direita(&mut self) {
        if self.cx < self.linhas[self.cy].len() {
            self.cx += 1;
        } else if self.cy + 1 < self.linhas.len() {
            self.cy += 1;
            self.cx = 0;
        }
        self.desejada = self.cx;
    }

    fn inicio(&mut self) {
        self.cx = 0;
        self.desejada = 0;
    }

    fn fim(&mut self) {
        self.cx = self.linhas[self.cy].len();
        self.desejada = self.cx;
    }

    /// Anda `delta` linhas (limitado ao primeiro e ao último). A coluna volta à
    /// desejada, limitada ao tamanho da linha de destino, **sem** mudá-la.
    fn pagina(&mut self, delta: isize) {
        let ultima = self.linhas.len() as isize - 1;
        let alvo = (self.cy as isize + delta).clamp(0, ultima) as usize;
        if alvo == self.cy {
            return;
        }
        self.cy = alvo;
        self.cx = self.desejada.min(self.linhas[alvo].len());
    }

    fn cima(&mut self) {
        self.pagina(-1);
    }

    fn baixo(&mut self) {
        self.pagina(1);
    }

    /// `Ctrl+C`: mostra a linha e a coluna do cursor (a partir de 1) e o total.
    fn mostra_posicao(&mut self) {
        self.mensagem = format!(
            "linha {}/{}, coluna {}/{}",
            self.cy + 1,
            self.linhas.len(),
            self.cx + 1,
            self.linhas[self.cy].len() + 1
        );
    }

    /// `Ctrl+_`: pergunta o número da linha e leva o cursor ao começo dela.
    fn vai_para_linha(&mut self) {
        let Some(digitado) = self.campo("Ir para a linha: ", "") else { return };
        let total = self.linhas.len();
        match digitado.parse::<usize>() {
            Ok(numero) if (1..=total).contains(&numero) => {
                self.cy = numero - 1;
                self.cx = 0;
                self.desejada = 0;
            }
            _ => self.mensagem = format!("Linha fora do intervalo (1 a {})", total),
        }
    }

    // -- cortar e colar ----------------------------------------------------------

    /// `Ctrl+K`: corta a linha do cursor. Cortes seguidos acumulam na área; num
    /// texto que já é uma única linha vazia a tecla é ignorada (e a sequência de
    /// cortes continua como estava).
    fn corta_linha(&mut self, corte_anterior: bool) {
        let n = self.linhas.len();
        if n == 1 && self.linhas[0].is_empty() {
            self.corte_seguido = corte_anterior;
            return;
        }
        if !corte_anterior {
            self.area.clear();
        }
        let linha = if n == 1 { core::mem::take(&mut self.linhas[0]) } else { self.linhas.remove(self.cy) };
        self.soma -= linha.len();
        self.area.push(linha);
        self.cy = self.cy.min(self.linhas.len() - 1);
        self.cx = 0;
        self.desejada = 0;
        self.modificado = true;
        self.corte_seguido = true;
    }

    /// `Ctrl+U`: cola as linhas cortadas acima da linha do cursor. A área não
    /// esvazia, então se pode colar várias vezes.
    fn cola(&mut self) {
        if self.area.is_empty() {
            self.mensagem = String::from("Nada para colar");
            return;
        }
        let k = self.area.len();
        let a_mais: usize = self.area.iter().map(|l| l.len()).sum();
        if let Err(limite) = cabe(self.soma + a_mais, self.linhas.len() + k, self.ultima_vazia()) {
            return self.recusa(limite);
        }
        let copias: Vec<Vec<u8>> = self.area.clone();
        let _ = self.linhas.splice(self.cy..self.cy, copias);
        self.soma += a_mais;
        self.cy += k;
        self.cx = 0;
        self.desejada = 0;
        self.modificado = true;
    }

    /// `Ctrl+R`: insere um arquivo na posição do cursor. O cursor termina no
    /// fim do texto inserido; qualquer falha deixa o texto intacto.
    fn insere_arquivo(&mut self) {
        let Some(nome) = self.campo("Arquivo a inserir: ", "") else { return };
        if nome.is_empty() {
            return;
        }
        if !nome_valido(&nome) {
            self.mensagem = String::from(SO_ABSOLUTO_NA_TELA);
            return;
        }
        let dados = match le_arquivo(&nome) {
            Ok(dados) => dados,
            Err(ErroAbrir::NaoExiste) => {
                self.mensagem = format!("Erro ao ler: {}", FsError::NotFound);
                return;
            }
            Err(ErroAbrir::Fs(erro)) => {
                self.mensagem = format!("Erro ao ler: {}", erro);
                return;
            }
            Err(ErroAbrir::Grande) => return self.recusa(Limite::Bytes),
        };
        let mut novas = divide(&dados);
        // O buffer de leitura sai antes de o texto novo ser montado.
        drop(dados);
        if novas.len() == 1 && novas[0].is_empty() {
            return;
        }
        let k = novas.len();
        let a_mais: usize = novas.iter().map(|l| l.len()).sum();
        let n = self.linhas.len();
        let resto_vazio = self.cx == self.linhas[self.cy].len();
        let ultima_vazia = if self.cy == n - 1 {
            if k == 1 { novas[0].is_empty() && resto_vazio && self.cx == 0 } else { novas[k - 1].is_empty() && resto_vazio }
        } else {
            self.ultima_vazia()
        };
        if let Err(limite) = cabe(self.soma + a_mais, n + k - 1, ultima_vazia) {
            return self.recusa(limite);
        }
        let resto = self.linhas[self.cy].split_off(self.cx);
        let primeira = novas.remove(0);
        self.linhas[self.cy].extend_from_slice(&primeira);
        if novas.is_empty() {
            self.cx += primeira.len();
            self.linhas[self.cy].extend_from_slice(&resto);
        } else {
            let ultima = novas.len() - 1;
            self.cx = novas[ultima].len();
            novas[ultima].extend_from_slice(&resto);
            let _ = self.linhas.splice(self.cy + 1..self.cy + 1, novas);
            self.cy += k - 1;
        }
        self.soma += a_mais;
        self.desejada = self.cx;
        self.modificado = true;
    }

    // -- busca e substituição ----------------------------------------------------

    /// `Ctrl+W`: procura o termo para a frente, a partir de **um byte depois do
    /// cursor**, no máximo uma volta no arquivo.
    fn busca(&mut self) {
        let Some(termo) = self.pede_termo("Buscar") else { return };
        let total = self.linhas.len();
        let mut achou = None;
        let mut reiniciou = false;
        if let Some(coluna) = acha(&self.linhas[self.cy], &termo, self.cx + 1) {
            achou = Some((self.cy, coluna));
        } else {
            for linha in self.cy + 1..total {
                if let Some(coluna) = acha(&self.linhas[linha], &termo, 0) {
                    achou = Some((linha, coluna));
                    break;
                }
            }
            if achou.is_none() {
                reiniciou = true;
                for linha in 0..=self.cy {
                    if let Some(coluna) = acha(&self.linhas[linha], &termo, 0) {
                        achou = Some((linha, coluna));
                        break;
                    }
                }
            }
        }
        match achou {
            Some((linha, coluna)) => {
                self.cy = linha;
                self.cx = coluna;
                self.desejada = coluna;
                self.destaque = Some(Destaque { linha, coluna, tamanho: termo.len() });
                if reiniciou {
                    self.mensagem = String::from("Busca reiniciada");
                }
            }
            None => {
                let visto = core::str::from_utf8(&termo).unwrap_or("");
                self.mensagem = format!("\"{}\" nao encontrado", visto);
            }
        }
    }

    /// `Ctrl+\` e `Ctrl+T`: troca o termo por um texto novo, percorrendo o
    /// arquivo **inteiro, do começo ao fim, uma vez**. O texto novo nunca é
    /// buscado de novo: depois de uma troca a busca segue depois dele.
    fn substitui(&mut self) {
        let Some(termo) = self.pede_termo("Substituir") else { return };
        let Some(novo) = self.campo("Substituir por: ", "") else { return };
        let novo = novo.into_bytes();
        let mut feitas = 0usize;
        let mut ocorreu = false;
        let mut todos = false;
        let mut cancelou = false;
        let mut estourou = false;
        let (mut linha, mut coluna) = (0usize, 0usize);
        while linha < self.linhas.len() {
            let Some(pos) = acha(&self.linhas[linha], &termo, coluna) else {
                linha += 1;
                coluna = 0;
                continue;
            };
            ocorreu = true;
            self.cy = linha;
            self.cx = pos;
            self.desejada = pos;
            let resposta = if todos {
                Resposta::Sim
            } else {
                self.destaque = Some(Destaque { linha, coluna: pos, tamanho: termo.len() });
                self.rola();
                self.pergunta("Substituir este? (S sim, N nao, A todos, Ctrl+C cancela)", b"sna")
            };
            match resposta {
                Resposta::Cancela => {
                    cancelou = true;
                    break;
                }
                Resposta::Nao => coluna = pos + termo.len(),
                Resposta::Sim | Resposta::Todos => {
                    if resposta == Resposta::Todos {
                        todos = true;
                    }
                    // O termo está no texto, então `soma >= termo.len()`: sem estouro.
                    let nova_soma = self.soma + novo.len() - termo.len();
                    let tamanho_depois = self.linhas[linha].len() + novo.len() - termo.len();
                    let n = self.linhas.len();
                    let ultima_vazia = if linha == n - 1 { tamanho_depois == 0 } else { self.ultima_vazia() };
                    if cabe(nova_soma, n, ultima_vazia).is_err() {
                        estourou = true;
                        break;
                    }
                    let _ = self.linhas[linha].splice(pos..pos + termo.len(), novo.iter().copied());
                    self.soma = nova_soma;
                    feitas += 1;
                    if termo != novo {
                        self.modificado = true;
                    }
                    coluna = pos + novo.len();
                }
            }
        }
        self.destaque = None;
        self.mensagem = if estourou {
            format!("{}. Substituicoes: {}", texto_do_limite(Limite::Bytes), feitas)
        } else if cancelou || ocorreu {
            format!("Substituicoes: {}", feitas)
        } else {
            String::from("Nenhuma ocorrencia")
        };
    }

    // -- gravar e sair ---------------------------------------------------------

    /// Grava o texto em `nome`. Devolve `true` se gravou. Se o nome é diferente
    /// do atual e o arquivo já existe, pergunta antes de sobrescrever.
    fn grava(&mut self, nome: &str) -> bool {
        if !nome_valido(nome) {
            self.mensagem = String::from(SO_ABSOLUTO_NA_TELA);
            return false;
        }
        if self.nome.as_deref() != Some(nome) {
            match File::open(nome) {
                Ok(arquivo) => {
                    drop(arquivo);
                    let resposta = self.pergunta("Arquivo existe. Sobrescrever? (S sim, N nao)", b"sn");
                    if resposta != Resposta::Sim {
                        return false;
                    }
                }
                Err(FsError::NotFound) => {}
                Err(erro) => {
                    self.mensagem = format!("Erro ao gravar: {}", erro);
                    return false;
                }
            }
        }
        let bytes = junta(&self.linhas);
        if let Err(erro) = grava_arquivo(nome, &bytes) {
            self.mensagem = format!("Erro ao gravar: {}", erro);
            return false;
        }
        let quebras = bytes.iter().filter(|&&b| b == b'\n').count();
        self.modificado = false;
        self.nome = Some(String::from(nome));
        self.mensagem = if quebras == 1 { String::from("Gravada 1 linha") } else { format!("Gravadas {} linhas", quebras) };
        true
    }

    /// `Ctrl+O`: pergunta o nome (já preenchido com o atual) e grava.
    fn grava_pedindo_nome(&mut self) -> bool {
        let inicial = self.nome.clone().unwrap_or_default();
        let Some(nome) = self.campo("Nome do arquivo para gravar: ", &inicial) else { return false };
        if nome.is_empty() {
            return false;
        }
        self.grava(&nome)
    }

    /// `Ctrl+S`: com nome, grava direto; sem nome, age como `Ctrl+O`.
    fn grava_direto(&mut self) -> bool {
        match self.nome.clone() {
            Some(nome) => self.grava(&nome),
            None => self.grava_pedindo_nome(),
        }
    }

    /// `Ctrl+X`: devolve `true` se o editor deve sair. Com alterações pergunta;
    /// `S` só sai se gravou.
    fn sai(&mut self) -> bool {
        if !self.modificado {
            return true;
        }
        match self.pergunta("Gravar o texto alterado? (S sim, N nao, Ctrl+C cancela)", b"sn") {
            Resposta::Sim => self.grava_direto(),
            Resposta::Nao => true,
            _ => false,
        }
    }

    /// `Ctrl+G`: a tela de ajuda. Qualquer tecla volta, sem mudar nada.
    fn ajuda(&self) {
        desenha_ajuda();
        let _ = read_key();
    }

    // -- despacho de teclas --------------------------------------------------------

    /// Trata uma tecla. O Ctrl chega como uma letra minúscula mais um bit
    /// (`ctrl`); `Backspace` e `Ctrl+H` são teclas diferentes. Tudo o que não
    /// está na tabela não faz nada.
    fn trata(&mut self, evento: KeyEvent) -> Fluxo {
        // A mensagem e o trecho em reverso duram até a próxima tecla, e qualquer
        // tecla encerra uma sequência de cortes (só o `Ctrl+K` a continua).
        self.mensagem.clear();
        self.destaque = None;
        let corte_anterior = self.corte_seguido;
        self.corte_seguido = false;
        match (evento.key, evento.ctrl) {
            // Texto.
            (Key::Char(c), false) => self.insere(c as u8),
            (Key::Enter, false) => self.parte_linha(),
            (Key::Tab, false) => self.tabula(),
            (Key::Backspace, false) | (Key::Char('h'), true) => self.apaga_esquerda(),
            (Key::Delete, false) | (Key::Char('d'), true) => self.apaga_direita(),
            // Movimento.
            (Key::Left, false) | (Key::Char('b'), true) => self.esquerda(),
            (Key::Right, false) | (Key::Char('f'), true) => self.direita(),
            (Key::Up, false) | (Key::Char('p'), true) => self.cima(),
            (Key::Down, false) | (Key::Char('n'), true) => self.baixo(),
            (Key::Home, false) | (Key::Char('a'), true) => self.inicio(),
            (Key::End, false) | (Key::Char('e'), true) => self.fim(),
            (Key::PageUp, false) | (Key::Char('y'), true) => self.pagina(-(PAGINA as isize)),
            (Key::PageDown, false) | (Key::Char('v'), true) => self.pagina(PAGINA as isize),
            (Key::Char('_'), true) | (Key::Char('/'), true) => self.vai_para_linha(),
            (Key::Char('c'), true) => self.mostra_posicao(),
            // Cortar, colar e inserir arquivo.
            (Key::Char('k'), true) => self.corta_linha(corte_anterior),
            (Key::Char('u'), true) => self.cola(),
            (Key::Char('r'), true) => self.insere_arquivo(),
            // Busca e substituição.
            (Key::Char('w'), true) => self.busca(),
            (Key::Char('\\'), true) | (Key::Char('t'), true) => self.substitui(),
            // Arquivo, ajuda e saída.
            (Key::Char('o'), true) => {
                self.grava_pedindo_nome();
            }
            (Key::Char('s'), true) => {
                self.grava_direto();
            }
            (Key::Char('g'), true) => self.ajuda(),
            (Key::Char('x'), true) => {
                if self.sai() {
                    return Fluxo::Sai;
                }
            }
            _ => {}
        }
        Fluxo::Continua
    }
}

fn main() -> i32 {
    // Os argumentos chegam como um texto só: nenhuma palavra é um texto novo,
    // uma é o arquivo, mais de uma é erro de uso. Tudo isso é decidido **antes**
    // de mexer na tela.
    let mut area = [0u8; args::MAX_ARGS_LEN];
    let texto = args::read(&mut area);
    let mut palavras = args::words(texto);
    let primeira = palavras.next();
    if palavras.next().is_some() {
        return uso("uso: exec mano [arquivo]");
    }
    let mut mensagem = "";
    let (linhas, nome) = match primeira {
        None => (vec![Vec::new()], None),
        Some(caminho) => {
            if !nome_valido(caminho) {
                return uso(SO_ABSOLUTO);
            }
            if caminho.len() > MAX_CAMINHO {
                return uso(&format!("caminho muito longo (maximo {} bytes)", MAX_CAMINHO));
            }
            let nome = Some(String::from(caminho));
            match abre(caminho) {
                Ok(linhas) => (linhas, nome),
                Err(ErroAbrir::NaoExiste) => {
                    mensagem = "Novo arquivo";
                    (vec![Vec::new()], nome)
                }
                Err(ErroAbrir::Fs(erro)) => return uso(&format!("{}", erro)),
                Err(ErroAbrir::Grande) => {
                    return uso(&format!(
                        "arquivo grande demais (maximo {} bytes e {} linhas)",
                        MAX_BYTES, MAX_LINHAS
                    ));
                }
            }
        }
    };

    let mut editor = Editor::novo(linhas, nome);
    editor.mensagem = String::from(mensagem);
    screen::clear();
    editor.redesenha();
    loop {
        let evento = read_key();
        if let Fluxo::Sai = editor.trata(evento) {
            break;
        }
        editor.rola();
        editor.redesenha();
    }
    // Deixa a tela limpa e na cor padrão: o prompt volta como depois de `clear`.
    screen::reset_color();
    screen::clear();
    0
}
