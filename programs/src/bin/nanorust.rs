//! `nanorust`: um compilador de um subconjunto estrito de Rust, que roda dentro
//! do próprio os-rust e gera um executável ELF64 que o `run` carrega.
//!
//! ```text
//! os-rust> exec nanorust /ram/fibo.rs /ram/fibo
//! nanorust: ok, 1234 bytes
//! os-rust> run /ram/fibo
//! ```
//!
//! O programa lê o fonte inteiro, compila tudo na memória e só grava a saída
//! se não houve erro (um erro de compilação nunca cria nem altera arquivo).
//! A compilação tem quatro etapas, nesta ordem, e para no **primeiro** erro:
//!
//! 1. o léxico separa o fonte em tokens;
//! 2. a passada 1 registra o nome, os parâmetros e o retorno de cada função,
//!    pulando os corpos (assim uma função pode ser chamada antes de definida);
//! 3. a passada 2 confere os tipos **e gera o código no mesmo passo**, sem
//!    árvore sintática: o código de máquina sai enquanto o fonte é lido;
//! 4. `fecha` junta as rotinas de apoio, os textos, resolve os remendos e
//!    escreve o cabeçalho ELF.
//!
//! A pilha do programa é de 16 KiB, então a recursão do compilador acompanha
//! só o aninhamento (no máximo 10 níveis), nunca o tamanho do fonte: cadeias de
//! operadores, sequências de instruções e cadeias `else if` são laços, e os
//! níveis de precedência são tratados por uma única função (`expr_prec`).
//!
//! O código gerado é de máquina de pilha, sem otimização: o resultado de cada
//! expressão fica em `rax` e os operandos em espera ficam na pilha.
//!
//! Compilado à parte do kernel e embutido na imagem de boot.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use runtime::{args, entry, println, remove, File, FileWriter};

entry!(main);

// ---------------------------------------------------------------------------
// 1. Constantes
// ---------------------------------------------------------------------------

/// Maior fonte aceito, em bytes.
const MAX_FONTE: usize = 16384;
/// Maior aninhamento (o corpo de uma função é o nível 1).
const MAX_NIVEIS: u8 = 10;
/// Maior quantidade de parâmetros por função.
const MAX_PARAMS: usize = 8;
/// Maior quantidade de argumentos por chamada ou por `print!`/`println!`.
const MAX_ARGS: usize = 8;
/// Maior executável gerado, em bytes (o que o `run` aceita carregar).
const MAX_EXE: usize = 65536;
/// Endereço virtual do segmento único.
const BASE: u64 = 0x4000_0000;
/// Cabeçalho ELF (64) mais um program header (56).
const CABECALHO: usize = 120;
/// Endereço de `_start`, logo depois do cabeçalho.
const ENTRADA: u64 = BASE + CABECALHO as u64;
/// Se `rsp` ficar abaixo disto, a pilha do programa gerado esgotou (o fundo da
/// pilha é `0x7FFF_C000`; a margem de 2048 bytes cobre o pior caso de
/// empilhamentos temporários).
const FUNDO_PILHA: u32 = 0x7FFF_C800;
/// Um `SYS_WRITE` aceita no máximo isto.
const PEDACO_TEXTO: usize = 4096;

// Capacidades fixas (nenhuma tabela cresce): ver o cálculo em `research.md`.
/// `fn f(){}` tem 6 tokens.
const CAP_FUNCS: usize = MAX_FONTE / 6 + 1;
/// `a:i64` tem 3 tokens, mais a vírgula.
const CAP_PARAMS: usize = MAX_FONTE / 3 + 1;
/// `let x:i64=0;` tem 7 tokens; mais 8 parâmetros.
const CAP_VARS: usize = MAX_FONTE / 7 + MAX_PARAMS + 1;
/// Cada remendo é de uma instrução de 5 bytes ou mais.
const CAP_REMENDOS: usize = MAX_EXE / 5 + 1;
/// Os textos decodificados nunca passam do tamanho do fonte.
const CAP_TEXTOS: usize = MAX_FONTE;

/// Marca de "sem posição" em um erro.
const SEM_POS: u32 = u32::MAX;
/// Fim de uma cadeia de remendos de salto.
const FIM_CADEIA: u32 = u32::MAX;

// ---------------------------------------------------------------------------
// 2. Erros
// ---------------------------------------------------------------------------

/// Um erro de compilação. Só números: o texto é montado em `mensagem`, fora
/// da recursão (nenhum `format!` roda dentro do compilador).
#[derive(Clone, Copy)]
struct Erro {
    /// O número do erro (1 a 17; M18 a M20 são tratados em `main`).
    cod: u8,
    /// Deslocamento do byte a que o erro aponta, ou `SEM_POS`.
    off: u32,
    /// Dados da mensagem (ver `mensagem`).
    a: u32,
    b: u32,
    c: u32,
}

type R<T> = Result<T, Erro>;

fn erro(cod: u8, off: u32) -> Erro {
    Erro { cod, off, a: 0, b: 0, c: 0 }
}

/// O que falta, em M04 (`esperado <o que>`).
const O_EXPR: u32 = 0;
const O_TEXTO: u32 = 1;
const O_IDENT: u32 = 2;
const O_TIPO: u32 = 3;
const O_FN: u32 = 4;
const O_PONTO_VIRGULA: u32 = 5;
const O_ABRE_CHAVE: u32 = 6;
const O_FECHA_CHAVE: u32 = 7;
const O_ABRE_PAREN: u32 = 8;
const O_FECHA_PAREN: u32 = 9;
const O_DOIS_PONTOS: u32 = 10;
const O_IGUAL: u32 = 11;
const O_EXCLAMACAO: u32 = 12;

const TEXTO_O_QUE: [&str; 13] = [
    "expressao",
    "texto",
    "identificador",
    "tipo (i64 ou bool)",
    "'fn'",
    "';'",
    "'{'",
    "'}'",
    "'('",
    "')'",
    "':'",
    "'='",
    "'!'",
];

/// Linha e coluna (a partir de 1, em bytes) do deslocamento `off`.
fn linha_coluna(fonte: &[u8], off: u32) -> (u32, u32) {
    let off = (off as usize).min(fonte.len());
    let mut linha = 1u32;
    let mut inicio = 0usize;
    for (i, &b) in fonte[..off].iter().enumerate() {
        if b == b'\n' {
            linha += 1;
            inicio = i + 1;
        }
    }
    (linha, (off - inicio) as u32 + 1)
}

/// O texto de um token, como aparece em M04 (`'x'`, `numero`, `texto`,
/// `fim do fonte`).
fn texto_do_token(fonte: &[u8], toks: &[Token], i: u32) -> String {
    let Some(t) = toks.get(i as usize) else { return String::from("fim do fonte") };
    match t.tipo {
        Tk::Fim => String::from("fim do fonte"),
        Tk::Int => String::from("numero"),
        Tk::Texto => String::from("texto"),
        _ => {
            let bytes = &fonte[t.off as usize..(t.off + t.len) as usize];
            format!("'{}'", core::str::from_utf8(bytes).unwrap_or("?"))
        }
    }
}

/// O nome (texto) de um token de identificador ou de palavra.
fn nome_do_token(fonte: &[u8], toks: &[Token], i: u32) -> String {
    match toks.get(i as usize) {
        Some(t) => {
            let bytes = &fonte[t.off as usize..(t.off + t.len) as usize];
            String::from(core::str::from_utf8(bytes).unwrap_or("?"))
        }
        None => String::new(),
    }
}

/// A linha completa de um erro de compilação, como a tabela de erros manda.
fn mensagem(fonte: &[u8], toks: &[Token], e: &Erro) -> String {
    let texto = match e.cod {
        1 => {
            if (0x20..=0x7e).contains(&e.a) {
                format!("caractere inesperado '{}'", e.a as u8 as char)
            } else {
                format!("byte inesperado 0x{:02x}", e.a)
            }
        }
        2 => String::from("numero invalido"),
        3 => String::from("texto invalido"),
        4 => format!(
            "esperado {}, encontrado {}",
            TEXTO_O_QUE[(e.a as usize).min(TEXTO_O_QUE.len() - 1)],
            texto_do_token(fonte, toks, e.b)
        ),
        5 => format!("aninhamento demais (maximo {})", MAX_NIVEIS),
        6 => format!("'{}' nao existe", nome_do_token(fonte, toks, e.a)),
        7 => format!("nome '{}' invalido (reservado ou ja existe)", nome_do_token(fonte, toks, e.a)),
        8 => format!(
            "tipos incompativeis: esperado {}, encontrado {}",
            Ty::de(e.a).nome(),
            Ty::de(e.b).nome()
        ),
        9 => format!(
            "a funcao '{}' espera {} argumentos, recebeu {}",
            nome_do_token(fonte, toks, e.a),
            e.b,
            e.c
        ),
        10 => format!("'{}' nao e mutavel (declare com let mut)", nome_do_token(fonte, toks, e.a)),
        11 => String::from("inteiro solto: use uma variavel ou chamada para fixar o tipo i64"),
        12 => String::from("divisao por zero literal"),
        13 => String::from("texto de formato invalido"),
        14 => String::from("funcao main nao encontrada"),
        15 => String::from("main nao pode ter parametros nem retorno, nem ser chamada"),
        16 => match e.a {
            0 => format!("limite excedido: fonte (maximo {} bytes)", MAX_FONTE),
            1 => format!("limite excedido: parametros (maximo {})", MAX_PARAMS),
            _ => format!("limite excedido: argumentos (maximo {})", MAX_ARGS),
        },
        _ => format!("programa grande demais (maximo {} bytes)", MAX_EXE),
    };
    if e.off == SEM_POS {
        format!("erro: {}", texto)
    } else {
        let (l, c) = linha_coluna(fonte, e.off);
        format!("erro na linha {}, coluna {}: {}", l, c, texto)
    }
}

// ---------------------------------------------------------------------------
// 3. Léxico
// ---------------------------------------------------------------------------

/// O tipo de um token. As palavras reservadas vêm primeiro (de `Fn` a `Res`),
/// para `eh_reservada` ser uma comparação só.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Tk {
    Fn,
    Let,
    Mut,
    If,
    Else,
    While,
    Return,
    True,
    False,
    TI64,
    TBool,
    Print,
    Println,
    /// Qualquer outra palavra reservada (`loop`, `match`, `_`...).
    Res,
    Ident,
    Int,
    Texto,
    Fim,
    AbreParen,
    FechaParen,
    AbreChave,
    FechaChave,
    Virgula,
    PontoVirgula,
    DoisPontos,
    Seta,
    Igual,
    Mais,
    Menos,
    Estrela,
    Barra,
    Percento,
    IgualIgual,
    Diferente,
    Menor,
    MenorIgual,
    Maior,
    MaiorIgual,
    EE,
    OuOu,
    Nao,
}

impl Tk {
    fn eh_reservada(self) -> bool {
        (self as u8) <= (Tk::Res as u8)
    }
}

/// Um token: 24 bytes. Linha e coluna só se calculam quando há erro.
#[derive(Clone, Copy)]
struct Token {
    tipo: Tk,
    /// Deslocamento no fonte (para um texto, o do `"` que o abre).
    off: u32,
    len: u32,
    /// O valor de um inteiro.
    valor: u64,
}

const _: () = assert!(core::mem::size_of::<Token>() == 24);

/// Classifica uma palavra: reservada (com o tipo próprio ou `Res`) ou `None`.
fn reservada(p: &[u8]) -> Option<Tk> {
    Some(match p {
        b"fn" => Tk::Fn,
        b"let" => Tk::Let,
        b"mut" => Tk::Mut,
        b"if" => Tk::If,
        b"else" => Tk::Else,
        b"while" => Tk::While,
        b"return" => Tk::Return,
        b"true" => Tk::True,
        b"false" => Tk::False,
        b"i64" => Tk::TI64,
        b"bool" => Tk::TBool,
        b"print" => Tk::Print,
        b"println" => Tk::Println,
        b"_" | b"as" | b"async" | b"await" | b"break" | b"const" | b"continue" | b"crate"
        | b"dyn" | b"enum" | b"extern" | b"for" | b"impl" | b"in" | b"loop" | b"match"
        | b"mod" | b"move" | b"pub" | b"ref" | b"self" | b"static" | b"struct" | b"super"
        | b"trait" | b"type" | b"unsafe" | b"use" | b"where" | b"abstract" | b"become"
        | b"box" | b"do" | b"final" | b"gen" | b"macro" | b"override" | b"priv" | b"try"
        | b"typeof" | b"unsized" | b"virtual" | b"yield" | b"union" | b"macro_rules" => Tk::Res,
        _ => return None,
    })
}

/// Separa o fonte em tokens. Erros: M01, M02 e M03 (de texto mal formado).
fn lexer(f: &[u8], toks: &mut Vec<Token>) -> R<()> {
    let n = f.len();
    let mut i = 0usize;
    while i < n {
        let c = f[i];
        match c {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'/' if i + 1 < n && f[i + 1] == b'/' => {
                // Comentário: até o fim da linha, qualquer byte vale.
                while i < n && f[i] != b'\n' {
                    i += 1;
                }
            }
            b'a'..=b'z' | b'_' => {
                let ini = i;
                while i < n && (f[i].is_ascii_lowercase() || f[i].is_ascii_digit() || f[i] == b'_') {
                    i += 1;
                }
                let tipo = reservada(&f[ini..i]).unwrap_or(Tk::Ident);
                toks.push(Token { tipo, off: ini as u32, len: (i - ini) as u32, valor: 0 });
            }
            b'0'..=b'9' => {
                let ini = i;
                let mut v: u64 = 0;
                let mut estouro = false;
                while i < n && f[i].is_ascii_digit() {
                    let d = u64::from(f[i] - b'0');
                    match v.checked_mul(10).and_then(|x| x.checked_add(d)) {
                        Some(x) if x <= i64::MAX as u64 => v = x,
                        _ => estouro = true,
                    }
                    i += 1;
                }
                // Um inteiro seguido de letra ou `_`, ou grande demais, é M02.
                if estouro || (i < n && (f[i].is_ascii_alphabetic() || f[i] == b'_')) {
                    return Err(erro(2, ini as u32));
                }
                toks.push(Token { tipo: Tk::Int, off: ini as u32, len: (i - ini) as u32, valor: v });
            }
            b'"' => {
                let ini = i;
                i += 1;
                loop {
                    if i >= n {
                        return Err(erro(3, ini as u32));
                    }
                    let b = f[i];
                    if b == b'"' {
                        i += 1;
                        break;
                    } else if b == b'\\' {
                        if i + 1 < n && matches!(f[i + 1], b'n' | b'\\' | b'"') {
                            i += 2;
                        } else {
                            return Err(erro(3, ini as u32));
                        }
                    } else if (0x20..=0x7e).contains(&b) {
                        i += 1;
                    } else {
                        return Err(erro(3, ini as u32));
                    }
                }
                toks.push(Token { tipo: Tk::Texto, off: ini as u32, len: (i - ini) as u32, valor: 0 });
            }
            _ => {
                let prox = if i + 1 < n { f[i + 1] } else { 0 };
                let (tipo, len) = match (c, prox) {
                    (b'-', b'>') => (Tk::Seta, 2),
                    (b'=', b'=') => (Tk::IgualIgual, 2),
                    (b'!', b'=') => (Tk::Diferente, 2),
                    (b'<', b'=') => (Tk::MenorIgual, 2),
                    (b'>', b'=') => (Tk::MaiorIgual, 2),
                    (b'&', b'&') => (Tk::EE, 2),
                    (b'|', b'|') => (Tk::OuOu, 2),
                    (b'(', _) => (Tk::AbreParen, 1),
                    (b')', _) => (Tk::FechaParen, 1),
                    (b'{', _) => (Tk::AbreChave, 1),
                    (b'}', _) => (Tk::FechaChave, 1),
                    (b',', _) => (Tk::Virgula, 1),
                    (b';', _) => (Tk::PontoVirgula, 1),
                    (b':', _) => (Tk::DoisPontos, 1),
                    (b'=', _) => (Tk::Igual, 1),
                    (b'+', _) => (Tk::Mais, 1),
                    (b'-', _) => (Tk::Menos, 1),
                    (b'*', _) => (Tk::Estrela, 1),
                    (b'/', _) => (Tk::Barra, 1),
                    (b'%', _) => (Tk::Percento, 1),
                    (b'<', _) => (Tk::Menor, 1),
                    (b'>', _) => (Tk::Maior, 1),
                    (b'!', _) => (Tk::Nao, 1),
                    _ => {
                        let mut e = erro(1, i as u32);
                        e.a = u32::from(c);
                        return Err(e);
                    }
                };
                toks.push(Token { tipo, off: i as u32, len, valor: 0 });
                i += len as usize;
            }
        }
    }
    toks.push(Token { tipo: Tk::Fim, off: n as u32, len: 0, valor: 0 });
    Ok(())
}

// ---------------------------------------------------------------------------
// 4. Tipos
// ---------------------------------------------------------------------------

/// Os tipos da linguagem. `Nunca` é o `!` (o de `return`): serve onde
/// qualquer tipo é esperado.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Ty {
    I64,
    Bool,
    Unit,
    Nunca,
}

impl Ty {
    fn nome(self) -> &'static str {
        match self {
            Ty::I64 => "i64",
            Ty::Bool => "bool",
            _ => "()",
        }
    }

    fn de(n: u32) -> Ty {
        match n {
            0 => Ty::I64,
            1 => Ty::Bool,
            2 => Ty::Unit,
            _ => Ty::Nunca,
        }
    }
}

/// O resultado de compilar uma expressão: o tipo e o que as conferências
/// seguintes precisam saber.
#[derive(Clone, Copy)]
struct Val {
    ty: Ty,
    /// Índice do primeiro token da expressão (onde M08 e M11 apontam). Para um
    /// bloco, o da expressão final dele (ou o `}` que o fecha, se não há).
    ini: u32,
    /// A expressão é *solta*: só literais inteiros e operações (o Rust daria
    /// `i32`), então nada nela fixa o tipo.
    solto: bool,
    /// É um literal inteiro de valor zero (com ou sem parênteses).
    zero: bool,
}

// ---------------------------------------------------------------------------
// 5. Tabelas
// ---------------------------------------------------------------------------

/// Uma função declarada (passada 1).
#[derive(Clone, Copy)]
struct Func {
    /// Índice do token do nome.
    nome: u16,
    /// Primeiro parâmetro na tabela de parâmetros, e quantos são.
    pfirst: u16,
    np: u8,
    ret: Ty,
    /// Índice do token `{` do corpo.
    corpo: u16,
    /// Posição do código no arquivo (0 até ser gerada).
    addr: u32,
}

/// Um parâmetro: o nome (índice do token) e o tipo.
#[derive(Clone, Copy)]
struct Param {
    nome: u16,
    ty: Ty,
}

/// Uma variável ou parâmetro visível (passada 2).
#[derive(Clone, Copy)]
struct Var {
    nome: u16,
    ty: Ty,
    mutavel: bool,
    /// Deslocamento a partir de `rbp`.
    disp: i32,
}

/// Um `rel32` a resolver no fim da passada 2. O alvo leva o tipo nos 4 bits
/// altos e o número (função ou posição do texto) nos 28 de baixo.
#[derive(Clone, Copy)]
struct Remendo {
    pos: u32,
    alvo: u32,
}

const ALVO_FUNCAO: u32 = 0;
const ALVO_ESTOURO: u32 = 1;
const ALVO_DIV0: u32 = 2;
const ALVO_PILHA: u32 = 3;
const ALVO_IMPRIME: u32 = 4;
const ALVO_LE: u32 = 5;
const ALVO_TEXTO: u32 = 6;

// ---------------------------------------------------------------------------
// 7. Rotinas de apoio (código de máquina escrito à mão)
// ---------------------------------------------------------------------------

const MSG_ESTOURO: &[u8] = b"panic: estouro aritmetico\n";
const MSG_DIV0: &[u8] = b"panic: divisao por zero\n";
const MSG_PILHA: &[u8] = b"panic: pilha esgotada\n";

/// Tamanho do código de um panic, sem o texto.
const TAM_PANIC: usize = 31;

/// `imprime`: escreve o `i64` de `rax` em decimal (um `SYS_WRITE`). O número
/// é convertido como sem sinal depois de um `neg`, então `i64::MIN` sai certo.
/// O buffer de 32 bytes fica na pilha, abaixo de `rbp`.
#[rustfmt::skip]
const ROT_IMPRIME: &[u8] = &[
    0x55,                                     //  0: push rbp
    0x48, 0x89, 0xE5,                         //  1: mov rbp, rsp
    0x48, 0x83, 0xEC, 0x20,                   //  4: sub rsp, 32
    0x31, 0xDB,                               //  8: xor ebx, ebx        ; sinal = 0
    0x48, 0x85, 0xC0,                         // 10: test rax, rax
    0x79, 0x08,                               // 13: jns L1
    0x48, 0xF7, 0xD8,                         // 15: neg rax
    0xBB, 0x01, 0x00, 0x00, 0x00,             // 18: mov ebx, 1         ; sinal = 1
    0x48, 0x8D, 0x7D, 0xFF,                   // 23: L1: lea rdi, [rbp-1]  ; fim do buffer
    0xB9, 0x0A, 0x00, 0x00, 0x00,             // 27: mov ecx, 10
    0x31, 0xD2,                               // 32: L2: xor edx, edx
    0x48, 0xF7, 0xF1,                         // 34: div rcx            ; rax /= 10, rdx = resto
    0x80, 0xC2, 0x30,                         // 37: add dl, '0'
    0x88, 0x17,                               // 40: mov [rdi], dl
    0x48, 0xFF, 0xCF,                         // 42: dec rdi
    0x48, 0x85, 0xC0,                         // 45: test rax, rax
    0x75, 0xEE,                               // 48: jnz L2
    0x85, 0xDB,                               // 50: test ebx, ebx
    0x74, 0x06,                               // 52: jz L3
    0xC6, 0x07, 0x2D,                         // 54: mov byte [rdi], '-'
    0x48, 0xFF, 0xCF,                         // 57: dec rdi
    0x48, 0x89, 0xEE,                         // 60: L3: mov rsi, rbp
    0x48, 0x29, 0xFE,                         // 63: sub rsi, rdi
    0x48, 0xFF, 0xCE,                         // 66: dec rsi            ; rsi = tamanho
    0x48, 0xFF, 0xC7,                         // 69: inc rdi            ; rdi = início
    0xB8, 0x01, 0x00, 0x00, 0x00,             // 72: mov eax, 1         ; SYS_WRITE
    0x0F, 0x05,                               // 77: syscall
    0x48, 0x89, 0xEC,                         // 79: mov rsp, rbp
    0x5D,                                     // 82: pop rbp
    0xC3,                                     // 83: ret
];

/// `le_i64`: lê uma linha (`SYS_READ_LINE`, tamanho 24) e devolve em `rax` o
/// valor se ela casa com `-?[0-9]{1,18}` (sem o `\n`), senão `0`. Com no máximo
/// 18 dígitos o acúmulo nunca estoura, então não há `jo`.
#[rustfmt::skip]
const ROT_LE_I64: &[u8] = &[
    0x55,                                     //   0: push rbp
    0x48, 0x89, 0xE5,                         //   1: mov rbp, rsp
    0x48, 0x83, 0xEC, 0x20,                   //   4: sub rsp, 32
    0x48, 0x8D, 0x7D, 0xE0,                   //   8: lea rdi, [rbp-32]
    0xBE, 0x18, 0x00, 0x00, 0x00,             //  12: mov esi, 24
    0xB8, 0x03, 0x00, 0x00, 0x00,             //  17: mov eax, 3         ; SYS_READ_LINE
    0x0F, 0x05,                               //  22: syscall
    0x48, 0x89, 0xC1,                         //  24: mov rcx, rax       ; n (com o \n)
    0x48, 0x85, 0xC9,                         //  27: test rcx, rcx
    0x7E, 0x58,                               //  30: jle INV
    0x48, 0x8D, 0x75, 0xE0,                   //  32: lea rsi, [rbp-32]
    0x80, 0x7C, 0x0E, 0xFF, 0x0A,             //  36: cmp byte [rsi+rcx-1], 10
    0x75, 0x03,                               //  41: jne +3
    0x48, 0xFF, 0xC9,                         //  43: dec rcx            ; tira o \n
    0x31, 0xDB,                               //  46: xor ebx, ebx       ; sinal = 0
    0x31, 0xC0,                               //  48: xor eax, eax       ; acumulador
    0x48, 0x85, 0xC9,                         //  50: test rcx, rcx
    0x74, 0x41,                               //  53: jz INV
    0x80, 0x3E, 0x2D,                         //  55: cmp byte [rsi], '-'
    0x75, 0x0B,                               //  58: jne SKIPNEG
    0xBB, 0x01, 0x00, 0x00, 0x00,             //  60: mov ebx, 1
    0x48, 0xFF, 0xC6,                         //  65: inc rsi
    0x48, 0xFF, 0xC9,                         //  68: dec rcx
    0x48, 0x85, 0xC9,                         //  71: SKIPNEG: test rcx, rcx
    0x74, 0x2C,                               //  74: jz INV
    0x48, 0x83, 0xF9, 0x12,                   //  76: cmp rcx, 18
    0x77, 0x26,                               //  80: ja INV
    0x0F, 0xB6, 0x16,                         //  82: LOOP: movzx edx, byte [rsi]
    0x83, 0xEA, 0x30,                         //  85: sub edx, '0'
    0x83, 0xFA, 0x09,                         //  88: cmp edx, 9
    0x77, 0x1B,                               //  91: ja INV
    0x48, 0x6B, 0xC0, 0x0A,                   //  93: imul rax, rax, 10
    0x48, 0x01, 0xD0,                         //  97: add rax, rdx
    0x48, 0xFF, 0xC6,                         // 100: inc rsi
    0x48, 0xFF, 0xC9,                         // 103: dec rcx
    0x75, 0xE6,                               // 106: jnz LOOP
    0x85, 0xDB,                               // 108: test ebx, ebx
    0x74, 0x03,                               // 110: jz DONE
    0x48, 0xF7, 0xD8,                         // 112: neg rax
    0x48, 0x89, 0xEC,                         // 115: DONE: mov rsp, rbp
    0x5D,                                     // 118: pop rbp
    0xC3,                                     // 119: ret
    0x31, 0xC0,                               // 120: INV: xor eax, eax
    0x48, 0x89, 0xEC,                         // 122: mov rsp, rbp
    0x5D,                                     // 125: pop rbp
    0xC3,                                     // 126: ret
];

// ---------------------------------------------------------------------------
// 6. O compilador: emissor, passada 1 e passada 2
// ---------------------------------------------------------------------------

struct Comp {
    fonte: Vec<u8>,
    toks: Vec<Token>,
    funcs: Vec<Func>,
    params: Vec<Param>,
    vars: Vec<Var>,
    /// O executável: cabeçalho, código, rotinas de apoio e textos.
    exe: Vec<u8>,
    textos: Vec<u8>,
    remendos: Vec<Remendo>,
    /// Índice do token atual.
    pos: usize,
    /// Aninhamento atual.
    prof: u8,
    /// Quantas locais a função atual declarou (cada `let` conta).
    nlocais: u32,
    ret_atual: Ty,
    main_idx: usize,
    usa_imprime: bool,
    usa_le: bool,
    usa_estouro: bool,
    usa_div0: bool,
}

impl Comp {
    fn novo(fonte: Vec<u8>) -> Comp {
        // O vetor de tokens é o maior: reservado primeiro, de uma vez, com o
        // tamanho do fonte mais 1 (o token de fim). Nenhum vetor cresce.
        let toks = Vec::with_capacity(fonte.len() + 1);
        Comp {
            fonte,
            toks,
            funcs: Vec::with_capacity(CAP_FUNCS),
            params: Vec::with_capacity(CAP_PARAMS),
            vars: Vec::with_capacity(CAP_VARS),
            exe: Vec::with_capacity(MAX_EXE),
            textos: Vec::with_capacity(CAP_TEXTOS),
            remendos: Vec::with_capacity(CAP_REMENDOS),
            pos: 0,
            prof: 0,
            nlocais: 0,
            ret_atual: Ty::Unit,
            main_idx: 0,
            usa_imprime: false,
            usa_le: false,
            usa_estouro: false,
            usa_div0: false,
        }
    }

    // ---- acesso aos tokens -------------------------------------------------

    fn tk(&self) -> Tk {
        self.toks[self.pos].tipo
    }

    /// O tipo do token seguinte (o de fim se não há).
    fn tk_seguinte(&self) -> Tk {
        self.toks.get(self.pos + 1).map_or(Tk::Fim, |t| t.tipo)
    }

    fn avanca(&mut self) {
        if self.toks[self.pos].tipo != Tk::Fim {
            self.pos += 1;
        }
    }

    fn e(&self, cod: u8, tok: usize) -> Erro {
        erro(cod, self.toks[tok].off)
    }

    /// M04 no token atual.
    fn esperado(&self, o_que: u32) -> Erro {
        let mut e = self.e(4, self.pos);
        e.a = o_que;
        e.b = self.pos as u32;
        e
    }

    /// M07, M06 e M10: erros que citam o nome de um token.
    fn e_nome(&self, cod: u8, tok: usize) -> Erro {
        let mut e = self.e(cod, tok);
        e.a = tok as u32;
        e
    }

    fn exige(&mut self, t: Tk, o_que: u32) -> R<()> {
        if self.tk() == t {
            self.avanca();
            Ok(())
        } else {
            Err(self.esperado(o_que))
        }
    }

    fn nome(&self, tok: usize) -> &[u8] {
        let t = &self.toks[tok];
        &self.fonte[t.off as usize..(t.off + t.len) as usize]
    }

    fn nomes_iguais(&self, a: usize, b: usize) -> bool {
        self.nome(a) == self.nome(b)
    }

    fn eh_ler_i64(&self, tok: usize) -> bool {
        self.nome(tok) == b"ler_i64"
    }

    /// O nome de `tok` é o de alguma função do fonte, ou `ler_i64`.
    fn eh_nome_de_funcao(&self, tok: usize) -> bool {
        self.eh_ler_i64(tok) || self.funcs.iter().any(|f| self.nomes_iguais(f.nome as usize, tok))
    }

    fn acha_func(&self, tok: usize) -> Option<usize> {
        self.funcs.iter().position(|f| self.nomes_iguais(f.nome as usize, tok))
    }

    fn acha_var(&self, tok: usize) -> Option<Var> {
        self.vars.iter().rev().find(|v| self.nomes_iguais(v.nome as usize, tok)).copied()
    }

    /// Lê um tipo (`i64` ou `bool`).
    fn tipo(&mut self) -> R<Ty> {
        let t = match self.tk() {
            Tk::TI64 => Ty::I64,
            Tk::TBool => Ty::Bool,
            _ => return Err(self.esperado(O_TIPO)),
        };
        self.avanca();
        Ok(t)
    }

    // ---- aninhamento -------------------------------------------------------

    /// Abre um nível de aninhamento no token atual (M05 se passar do limite).
    fn entra(&mut self) -> R<()> {
        if self.prof >= MAX_NIVEIS {
            return Err(self.e(5, self.pos));
        }
        self.prof += 1;
        Ok(())
    }

    fn sai(&mut self) {
        self.prof -= 1;
    }

    // ---- tipos -------------------------------------------------------------

    /// M08 se `v` não tem o tipo `esperado` (`Nunca` serve para todos).
    fn exige_ty(&self, v: &Val, esperado: Ty) -> R<()> {
        if v.ty == esperado || v.ty == Ty::Nunca {
            return Ok(());
        }
        Err(self.e_tipos(v.ini, esperado, v.ty))
    }

    fn e_tipos(&self, tok: u32, esperado: Ty, achado: Ty) -> Erro {
        let mut e = self.e(8, tok as usize);
        e.a = esperado as u32;
        e.b = achado as u32;
        e
    }

    // ---- emissor -----------------------------------------------------------

    /// Emite um byte. Ao passar de 65536 bytes (código e texto somados) é M17.
    fn b(&mut self, x: u8) -> R<()> {
        if self.exe.len() + self.textos.len() >= MAX_EXE {
            return Err(erro(17, SEM_POS));
        }
        self.exe.push(x);
        Ok(())
    }

    fn bs(&mut self, xs: &[u8]) -> R<()> {
        for &x in xs {
            self.b(x)?;
        }
        Ok(())
    }

    fn i32le(&mut self, v: i32) -> R<()> {
        self.bs(&v.to_le_bytes())
    }

    /// Emite `op` e um `rel32` a resolver no fim, para o alvo `kind`/`val`.
    fn rem(&mut self, op: &[u8], kind: u32, val: u32) -> R<()> {
        self.bs(op)?;
        if self.remendos.len() >= CAP_REMENDOS {
            return Err(erro(17, SEM_POS));
        }
        let pos = self.exe.len() as u32;
        self.remendos.push(Remendo { pos, alvo: (kind << 28) | val });
        match kind {
            ALVO_ESTOURO => self.usa_estouro = true,
            ALVO_DIV0 => self.usa_div0 = true,
            ALVO_IMPRIME => self.usa_imprime = true,
            ALVO_LE => self.usa_le = true,
            _ => {}
        }
        self.bs(&[0, 0, 0, 0])
    }

    /// Emite `op` e um `rel32` vazio, para ser corrigido com `aqui`. Devolve a
    /// posição do `rel32`.
    fn salto(&mut self, op: &[u8]) -> R<usize> {
        self.bs(op)?;
        let at = self.exe.len();
        self.bs(&[0, 0, 0, 0])?;
        Ok(at)
    }

    /// Faz o salto emitido por `salto` (em `at`) cair aqui.
    fn aqui(&mut self, at: usize) {
        let rel = self.exe.len() as i64 - (at as i64 + 4);
        self.exe[at..at + 4].copy_from_slice(&(rel as i32).to_le_bytes());
    }

    /// Emite um salto para trás, para `alvo`.
    fn salto_volta(&mut self, op: &[u8], alvo: usize) -> R<()> {
        self.bs(op)?;
        let fim = self.exe.len() as i64 + 4;
        self.i32le((alvo as i64 - fim) as i32)
    }

    /// Um inteiro em `rax`: `mov rax, imm32` (sinal estendido) ou `movabs`.
    fn emite_int(&mut self, v: u64) -> R<()> {
        if v <= i32::MAX as u64 {
            self.bs(&[0x48, 0xC7, 0xC0])?;
            self.bs(&(v as u32).to_le_bytes())
        } else {
            self.bs(&[0x48, 0xB8])?;
            self.bs(&v.to_le_bytes())
        }
    }

    /// `mov rsp, rbp; pop rbp; ret`.
    fn epilogo(&mut self) -> R<()> {
        self.bs(&[0x48, 0x89, 0xEC, 0x5D, 0xC3])
    }

    // ---- passada 1 ---------------------------------------------------------

    /// Registra as funções: nome, parâmetros e retorno. Os corpos são pulados
    /// pelo casamento de `{` e `}`.
    fn passada1(&mut self) -> R<()> {
        self.pos = 0;
        loop {
            match self.tk() {
                Tk::Fim => break,
                Tk::Fn => self.cabecalho()?,
                _ => return Err(self.esperado(O_FN)),
            }
        }
        match self.funcs.iter().position(|f| self.nome(f.nome as usize) == b"main") {
            Some(i) => {
                self.main_idx = i;
                Ok(())
            }
            None => Err(erro(14, 0)),
        }
    }

    fn cabecalho(&mut self) -> R<()> {
        self.avanca(); // fn
        let nome = self.pos;
        let t = self.tk();
        if t.eh_reservada() {
            return Err(self.e_nome(7, nome));
        }
        if t != Tk::Ident {
            return Err(self.esperado(O_IDENT));
        }
        if self.eh_ler_i64(nome) || self.acha_func(nome).is_some() {
            return Err(self.e_nome(7, nome));
        }
        self.avanca();
        self.exige(Tk::AbreParen, O_ABRE_PAREN)?;
        let pfirst = self.params.len();
        let mut np = 0usize;
        if self.tk() != Tk::FechaParen {
            loop {
                // Qualquer palavra serve como nome de parâmetro aqui; a
                // passada 2 confere (M07).
                let t = self.tk();
                if t != Tk::Ident && !t.eh_reservada() {
                    return Err(self.esperado(O_IDENT));
                }
                if np == MAX_PARAMS {
                    let mut e = self.e(16, self.pos);
                    e.a = 1;
                    return Err(e);
                }
                let pn = self.pos;
                self.avanca();
                self.exige(Tk::DoisPontos, O_DOIS_PONTOS)?;
                let ty = self.tipo()?;
                if self.params.len() >= CAP_PARAMS {
                    return Err(erro(17, SEM_POS));
                }
                self.params.push(Param { nome: pn as u16, ty });
                np += 1;
                if self.tk() == Tk::Virgula {
                    self.avanca();
                    if self.tk() == Tk::FechaParen {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
        self.exige(Tk::FechaParen, O_FECHA_PAREN)?;
        let mut ret = Ty::Unit;
        let mut tem_ret = false;
        if self.tk() == Tk::Seta {
            self.avanca();
            ret = self.tipo()?;
            tem_ret = true;
        }
        if self.nome(nome) == b"main" && (np > 0 || tem_ret) {
            return Err(self.e(15, nome));
        }
        if self.tk() != Tk::AbreChave {
            return Err(self.esperado(O_ABRE_CHAVE));
        }
        let corpo = self.pos;
        // Pula o corpo contando chaves.
        let mut nivel = 0u32;
        loop {
            match self.tk() {
                Tk::AbreChave => nivel += 1,
                Tk::FechaChave => {
                    nivel -= 1;
                    if nivel == 0 {
                        self.avanca();
                        break;
                    }
                }
                Tk::Fim => return Err(self.esperado(O_FECHA_CHAVE)),
                _ => {}
            }
            self.avanca();
        }
        if self.funcs.len() >= CAP_FUNCS {
            return Err(erro(17, SEM_POS));
        }
        self.funcs.push(Func {
            nome: nome as u16,
            pfirst: pfirst as u16,
            np: np as u8,
            ret,
            corpo: corpo as u16,
            addr: 0,
        });
        Ok(())
    }

    // ---- passada 2 ---------------------------------------------------------

    /// Confere os tipos e gera o código de todas as funções, na ordem do fonte.
    fn passada2(&mut self) -> R<()> {
        // O cabeçalho é reservado agora e preenchido no fim.
        for _ in 0..CABECALHO {
            self.b(0)?;
        }
        // `_start`: call main; mov edi, 0; mov eax, 2; syscall.
        let main = self.main_idx as u32;
        self.rem(&[0xE8], ALVO_FUNCAO, main)?;
        self.bs(&[0xBF, 0, 0, 0, 0, 0xB8, 2, 0, 0, 0, 0x0F, 0x05])?;
        for i in 0..self.funcs.len() {
            self.funcao(i)?;
        }
        Ok(())
    }

    fn funcao(&mut self, idx: usize) -> R<()> {
        let f = self.funcs[idx];
        let pf = f.pfirst as usize;
        let n = f.np as usize;
        // Os nomes dos parâmetros são conferidos aqui, na ordem e antes do corpo.
        for i in 0..n {
            let t = self.params[pf + i].nome as usize;
            let repetido = (0..i).any(|j| self.nomes_iguais(self.params[pf + j].nome as usize, t));
            if self.toks[t].tipo.eh_reservada() || self.eh_nome_de_funcao(t) || repetido {
                return Err(self.e_nome(7, t));
            }
        }
        self.funcs[idx].addr = self.exe.len() as u32;
        self.vars.clear();
        self.nlocais = 0;
        self.ret_atual = f.ret;
        for i in 0..n {
            let p = self.params[pf + i];
            let disp = 16 + 8 * (n - 1 - i) as i32;
            self.vars.push(Var { nome: p.nome, ty: p.ty, mutavel: false, disp });
        }
        // Prólogo: quadro, espaço das locais (preenchido no fim) e teste de pilha.
        self.bs(&[0x55, 0x48, 0x89, 0xE5, 0x48, 0x81, 0xEC])?;
        let sub = self.exe.len();
        self.bs(&[0, 0, 0, 0])?;
        self.bs(&[0x48, 0x81, 0xFC])?;
        self.bs(&FUNDO_PILHA.to_le_bytes())?;
        self.rem(&[0x0F, 0x82], ALVO_PILHA, 0)?;
        self.pos = f.corpo as usize;
        let v = self.bloco()?;
        if v.ty != Ty::Nunca && v.ty != f.ret {
            return Err(self.e_tipos(v.ini, f.ret, v.ty));
        }
        self.epilogo()?;
        let espaco = (8 * self.nlocais).to_le_bytes();
        self.exe[sub..sub + 4].copy_from_slice(&espaco);
        Ok(())
    }

    /// `{ instruções [expr] }`. O resultado é o tipo do valor final, se há; senão
    /// `Nunca` se alguma instrução do bloco tem esse tipo; senão `()`.
    fn bloco(&mut self) -> R<Val> {
        if self.tk() != Tk::AbreChave {
            return Err(self.esperado(O_ABRE_CHAVE));
        }
        self.entra()?;
        self.avanca();
        let vars0 = self.vars.len();
        let mut nunca = false;
        let mut cauda: Option<Val> = None;
        loop {
            match self.tk() {
                Tk::FechaChave => break,
                Tk::Fim => return Err(self.esperado(O_FECHA_CHAVE)),
                Tk::Let => nunca |= self.let_()?,
                Tk::Ident if self.tk_seguinte() == Tk::Igual => nunca |= self.atribuicao()?,
                Tk::If | Tk::While => {
                    let v = if self.tk() == Tk::If { self.se()? } else { self.enquanto()? };
                    if self.tk() == Tk::FechaChave {
                        cauda = Some(v);
                        break;
                    }
                    // Fora da última posição, precisa ser `()` ou `!`.
                    self.exige_ty(&v, Ty::Unit)?;
                    nunca |= v.ty == Ty::Nunca;
                }
                _ => {
                    let v = self.expr()?;
                    match self.tk() {
                        Tk::PontoVirgula => {
                            self.avanca();
                            if v.solto {
                                return Err(self.e(11, v.ini as usize));
                            }
                            nunca |= v.ty == Ty::Nunca;
                        }
                        Tk::FechaChave => {
                            cauda = Some(v);
                            break;
                        }
                        _ => return Err(self.esperado(O_PONTO_VIRGULA)),
                    }
                }
            }
        }
        let fecha = self.pos as u32;
        self.avanca();
        self.sai();
        self.vars.truncate(vars0);
        Ok(match cauda {
            Some(v) => Val { ty: v.ty, ini: v.ini, solto: v.solto, zero: false },
            None => Val { ty: if nunca { Ty::Nunca } else { Ty::Unit }, ini: fecha, solto: false, zero: false },
        })
    }

    /// `let [mut] nome: tipo = expr;`. Devolve se o inicializador é do tipo `!`.
    fn let_(&mut self) -> R<bool> {
        self.avanca(); // let
        let mutavel = if self.tk() == Tk::Mut {
            self.avanca();
            true
        } else {
            false
        };
        let nome = self.pos;
        let t = self.tk();
        if t.eh_reservada() {
            return Err(self.e_nome(7, nome));
        }
        if t != Tk::Ident {
            return Err(self.esperado(O_IDENT));
        }
        if self.eh_nome_de_funcao(nome) {
            return Err(self.e_nome(7, nome));
        }
        self.avanca();
        self.exige(Tk::DoisPontos, O_DOIS_PONTOS)?;
        let ty = self.tipo()?;
        self.exige(Tk::Igual, O_IGUAL)?;
        // O inicializador enxerga o nome anterior: a variável nova só entra na
        // tabela depois dele.
        let v = self.expr()?;
        self.exige_ty(&v, ty)?;
        self.exige(Tk::PontoVirgula, O_PONTO_VIRGULA)?;
        if self.vars.len() >= CAP_VARS {
            return Err(erro(17, SEM_POS));
        }
        let disp = -8 * (self.nlocais as i32 + 1);
        self.nlocais += 1;
        self.vars.push(Var { nome: nome as u16, ty, mutavel, disp });
        self.bs(&[0x48, 0x89, 0x85])?;
        self.i32le(disp)?;
        Ok(v.ty == Ty::Nunca)
    }

    /// `nome = expr;`. Devolve se o valor é do tipo `!`.
    fn atribuicao(&mut self) -> R<bool> {
        let nome = self.pos;
        let Some(var) = self.acha_var(nome) else {
            return Err(self.e_nome(6, nome));
        };
        if !var.mutavel {
            return Err(self.e_nome(10, nome));
        }
        self.avanca();
        self.avanca(); // =
        let v = self.expr()?;
        self.exige_ty(&v, var.ty)?;
        self.exige(Tk::PontoVirgula, O_PONTO_VIRGULA)?;
        self.bs(&[0x48, 0x89, 0x85])?;
        self.i32le(var.disp)?;
        Ok(v.ty == Ty::Nunca)
    }

    // ---- expressões --------------------------------------------------------

    fn expr(&mut self) -> R<Val> {
        self.expr_prec(1)
    }

    /// A precedência dos operadores binários: `||` 1, `&&` 2, comparações 3,
    /// `+ -` 4, `* / %` 5.
    fn prec(t: Tk) -> u8 {
        match t {
            Tk::OuOu => 1,
            Tk::EE => 2,
            Tk::IgualIgual | Tk::Diferente | Tk::Menor | Tk::MenorIgual | Tk::Maior | Tk::MaiorIgual => 3,
            Tk::Mais | Tk::Menos => 4,
            Tk::Estrela | Tk::Barra | Tk::Percento => 5,
            _ => 0,
        }
    }

    /// Operadores binários de precedência `>= min`, numa função só (precedência
    /// por tabela): é isso que mantém poucos quadros de pilha por nível de
    /// aninhamento. A cadeia `a + b + c` é um laço, não recursão.
    fn expr_prec(&mut self, min: u8) -> R<Val> {
        let mut esq = self.unario()?;
        let mut veio_de_comparacao = false;
        loop {
            let op = self.tk();
            let prec = Self::prec(op);
            if prec == 0 || prec < min {
                break;
            }
            // Comparações não se encadeiam: a seguinte fica para quem chamou
            // (que dá M04).
            if prec == 3 && veio_de_comparacao {
                break;
            }
            let optok = self.pos;
            self.avanca();
            match op {
                Tk::EE | Tk::OuOu => {
                    self.exige_ty(&esq, Ty::Bool)?;
                    self.bs(&[0x48, 0x85, 0xC0])?; // test rax, rax
                    let fim = self.salto(if op == Tk::EE { &[0x0F, 0x84] } else { &[0x0F, 0x85] })?;
                    let dir = self.expr_prec(prec + 1)?;
                    self.exige_ty(&dir, Ty::Bool)?;
                    self.aqui(fim);
                    esq = Val { ty: Ty::Bool, ini: esq.ini, solto: false, zero: false };
                    veio_de_comparacao = false;
                }
                Tk::IgualIgual | Tk::Diferente | Tk::Menor | Tk::MenorIgual | Tk::Maior | Tk::MaiorIgual => {
                    let igualdade = matches!(op, Tk::IgualIgual | Tk::Diferente);
                    if igualdade {
                        if esq.ty == Ty::Unit {
                            return Err(self.e_tipos(esq.ini, Ty::I64, Ty::Unit));
                        }
                    } else {
                        self.exige_ty(&esq, Ty::I64)?;
                    }
                    self.b(0x50)?; // push rax
                    let dir = self.expr_prec(prec + 1)?;
                    if igualdade {
                        if esq.ty != Ty::Nunca {
                            self.exige_ty(&dir, esq.ty)?;
                        }
                    } else {
                        self.exige_ty(&dir, Ty::I64)?;
                    }
                    if esq.solto && dir.solto {
                        return Err(self.e(11, esq.ini as usize));
                    }
                    self.bs(&[0x48, 0x89, 0xC1, 0x58])?; // mov rcx, rax; pop rax
                    self.bs(&[0x48, 0x39, 0xC8])?; // cmp rax, rcx
                    let cc = match op {
                        Tk::IgualIgual => 0x94,
                        Tk::Diferente => 0x95,
                        Tk::Menor => 0x9C,
                        Tk::MenorIgual => 0x9E,
                        Tk::Maior => 0x9F,
                        _ => 0x9D,
                    };
                    self.bs(&[0x0F, cc, 0xC0, 0x0F, 0xB6, 0xC0])?; // setcc al; movzx eax, al
                    esq = Val { ty: Ty::Bool, ini: esq.ini, solto: false, zero: false };
                    veio_de_comparacao = true;
                }
                _ => {
                    self.exige_ty(&esq, Ty::I64)?;
                    self.b(0x50)?; // push rax
                    let dir = self.expr_prec(prec + 1)?;
                    self.exige_ty(&dir, Ty::I64)?;
                    if (op == Tk::Barra || op == Tk::Percento) && dir.zero {
                        return Err(self.e(12, optok));
                    }
                    self.bs(&[0x48, 0x89, 0xC1, 0x58])?; // mov rcx, rax; pop rax
                    match op {
                        Tk::Mais => self.bs(&[0x48, 0x01, 0xC8])?,
                        Tk::Menos => self.bs(&[0x48, 0x29, 0xC8])?,
                        Tk::Estrela => self.bs(&[0x48, 0x0F, 0xAF, 0xC1])?,
                        _ => {
                            // `/` e `%`: divisor zero e `MIN / -1` dão panic.
                            self.bs(&[0x48, 0x85, 0xC9])?; // test rcx, rcx
                            self.rem(&[0x0F, 0x84], ALVO_DIV0, 0)?;
                            self.bs(&[0x48, 0x83, 0xF9, 0xFF, 0x75, 0x13])?; // cmp rcx, -1; jne ok
                            self.bs(&[0x48, 0xBA, 0, 0, 0, 0, 0, 0, 0, 0x80])?; // movabs rdx, i64::MIN
                            self.bs(&[0x48, 0x39, 0xD0])?; // cmp rax, rdx
                            self.rem(&[0x0F, 0x84], ALVO_ESTOURO, 0)?;
                            self.bs(&[0x48, 0x99, 0x48, 0xF7, 0xF9])?; // cqo; idiv rcx
                            if op == Tk::Percento {
                                self.bs(&[0x48, 0x89, 0xD0])?; // mov rax, rdx
                            }
                        }
                    }
                    if matches!(op, Tk::Mais | Tk::Menos | Tk::Estrela) {
                        self.rem(&[0x0F, 0x80], ALVO_ESTOURO, 0)?; // jo
                    }
                    esq = Val { ty: Ty::I64, ini: esq.ini, solto: esq.solto && dir.solto, zero: false };
                    veio_de_comparacao = false;
                }
            }
        }
        Ok(esq)
    }

    fn unario(&mut self) -> R<Val> {
        let ini = self.pos as u32;
        match self.tk() {
            Tk::Menos => {
                self.entra()?;
                self.avanca();
                let v = self.unario()?;
                self.exige_ty(&v, Ty::I64)?;
                self.bs(&[0x48, 0xF7, 0xD8])?; // neg rax
                self.rem(&[0x0F, 0x80], ALVO_ESTOURO, 0)?; // jo
                self.sai();
                Ok(Val { ty: Ty::I64, ini, solto: v.solto, zero: false })
            }
            Tk::Nao => {
                self.entra()?;
                self.avanca();
                let v = self.unario()?;
                self.exige_ty(&v, Ty::Bool)?;
                self.bs(&[0x83, 0xF0, 0x01])?; // xor eax, 1
                self.sai();
                Ok(Val { ty: Ty::Bool, ini, solto: false, zero: false })
            }
            _ => self.primario(),
        }
    }

    fn primario(&mut self) -> R<Val> {
        let ini = self.pos as u32;
        match self.tk() {
            Tk::Int => {
                let v = self.toks[self.pos].valor;
                self.avanca();
                self.emite_int(v)?;
                Ok(Val { ty: Ty::I64, ini, solto: true, zero: v == 0 })
            }
            Tk::True | Tk::False => {
                let v = u8::from(self.tk() == Tk::True);
                self.avanca();
                self.bs(&[0xB8, v, 0, 0, 0])?; // mov eax, 0/1
                Ok(Val { ty: Ty::Bool, ini, solto: false, zero: false })
            }
            Tk::Ident => {
                if self.tk_seguinte() == Tk::AbreParen {
                    return self.chamada();
                }
                let Some(var) = self.acha_var(self.pos) else {
                    return Err(self.e_nome(6, self.pos));
                };
                self.avanca();
                self.bs(&[0x48, 0x8B, 0x85])?; // mov rax, [rbp+disp32]
                self.i32le(var.disp)?;
                Ok(Val { ty: var.ty, ini, solto: false, zero: false })
            }
            Tk::AbreParen => {
                self.entra()?;
                self.avanca();
                let v = self.expr()?;
                self.exige(Tk::FechaParen, O_FECHA_PAREN)?;
                self.sai();
                Ok(Val { ty: v.ty, ini, solto: v.solto, zero: v.zero })
            }
            Tk::If => self.se(),
            Tk::Print | Tk::Println => self.macro_(),
            Tk::Return => self.ret(),
            // Um texto só existe como primeiro argumento de `print!`/`println!`.
            Tk::Texto => Err(self.e(3, self.pos)),
            _ => Err(self.esperado(O_EXPR)),
        }
    }

    /// `if cond { } [else if ... ] [else { }]`. A cadeia `else if` é um laço:
    /// os saltos para o fim formam uma lista encadeada dentro do próprio código
    /// (cada `rel32` guarda a posição do anterior), sem tabela.
    fn se(&mut self) -> R<Val> {
        let ini = self.pos as u32;
        let mut cadeia = FIM_CADEIA;
        let mut acc = Ty::Nunca;
        let mut solto = true;
        let mut tem_else = false;
        let mut ramo_ini = self.pos;
        let mut ultima_ini;
        loop {
            // A condição conta um nível enquanto é lida (M05 aponta para o `if`).
            self.entra()?;
            self.avanca();
            let c = self.expr()?;
            self.exige_ty(&c, Ty::Bool)?;
            self.sai();
            self.bs(&[0x48, 0x85, 0xC0])?; // test rax, rax
            let jz = self.salto(&[0x0F, 0x84])?;
            let v = self.bloco()?;
            self.junta_ramo(&mut acc, &v, ramo_ini)?;
            solto &= v.solto;
            ultima_ini = v.ini;
            if self.tk() == Tk::Else {
                self.avanca();
                let j = self.salto(&[0xE9])?;
                self.exe[j..j + 4].copy_from_slice(&cadeia.to_le_bytes());
                cadeia = j as u32;
                self.aqui(jz);
                ramo_ini = self.pos;
                if self.tk() == Tk::If {
                    continue;
                }
                let v2 = self.bloco()?;
                self.junta_ramo(&mut acc, &v2, ramo_ini)?;
                solto &= v2.solto;
                tem_else = true;
                break;
            }
            self.aqui(jz);
            break;
        }
        while cadeia != FIM_CADEIA {
            let at = cadeia as usize;
            let mut anterior = [0u8; 4];
            anterior.copy_from_slice(&self.exe[at..at + 4]);
            self.aqui(at);
            cadeia = u32::from_le_bytes(anterior);
        }
        if !tem_else {
            // Sem `else` o `if` tem tipo `()`.
            if acc != Ty::Nunca && acc != Ty::Unit {
                return Err(self.e_tipos(ultima_ini, Ty::Unit, acc));
            }
            acc = Ty::Unit;
        }
        Ok(Val { ty: acc, ini, solto: solto && tem_else, zero: false })
    }

    /// Junta o tipo de um ramo ao tipo dos anteriores (`Nunca` serve a todos).
    /// O erro aponta para o primeiro token do ramo.
    fn junta_ramo(&self, acc: &mut Ty, v: &Val, ramo_ini: usize) -> R<()> {
        if v.ty == Ty::Nunca {
            return Ok(());
        }
        if *acc == Ty::Nunca {
            *acc = v.ty;
            return Ok(());
        }
        if v.ty != *acc {
            return Err(self.e_tipos(ramo_ini as u32, *acc, v.ty));
        }
        Ok(())
    }

    /// `while cond { corpo }`.
    fn enquanto(&mut self) -> R<Val> {
        let ini = self.pos as u32;
        let topo = self.exe.len();
        self.entra()?;
        self.avanca();
        let c = self.expr()?;
        self.exige_ty(&c, Ty::Bool)?;
        self.sai();
        self.bs(&[0x48, 0x85, 0xC0])?; // test rax, rax
        let fim = self.salto(&[0x0F, 0x84])?;
        let v = self.bloco()?;
        self.exige_ty(&v, Ty::Unit)?;
        self.salto_volta(&[0xE9], topo)?;
        self.aqui(fim);
        Ok(Val { ty: Ty::Unit, ini, solto: false, zero: false })
    }

    /// `return [expr]`.
    fn ret(&mut self) -> R<Val> {
        let ini = self.pos as u32;
        self.entra()?;
        self.avanca();
        if matches!(self.tk(), Tk::PontoVirgula | Tk::FechaChave) {
            if self.ret_atual != Ty::Unit {
                return Err(self.e_tipos(ini, self.ret_atual, Ty::Unit));
            }
        } else {
            let v = self.expr()?;
            self.exige_ty(&v, self.ret_atual)?;
        }
        self.epilogo()?;
        self.sai();
        Ok(Val { ty: Ty::Nunca, ini, solto: false, zero: false })
    }

    /// `nome(args)`: argumentos da esquerda para a direita, empilhados; quem
    /// chama os remove.
    fn chamada(&mut self) -> R<Val> {
        let nome = self.pos;
        let embutida = self.eh_ler_i64(nome);
        let idx = if embutida {
            None
        } else {
            match self.acha_func(nome) {
                Some(i) => Some(i),
                None => return Err(self.e_nome(6, nome)),
            }
        };
        if idx == Some(self.main_idx) {
            return Err(self.e(15, nome));
        }
        let (np, ret, pf) = match idx {
            Some(i) => (self.funcs[i].np as usize, self.funcs[i].ret, self.funcs[i].pfirst as usize),
            None => (0, Ty::I64, 0),
        };
        self.avanca(); // nome
        self.entra()?;
        self.avanca(); // (
        let mut n = 0usize;
        if self.tk() != Tk::FechaParen {
            loop {
                if n == MAX_ARGS {
                    let mut e = self.e(16, self.pos);
                    e.a = 2;
                    return Err(e);
                }
                let v = self.expr()?;
                if n < np {
                    self.exige_ty(&v, self.params[pf + n].ty)?;
                }
                self.b(0x50)?; // push rax
                n += 1;
                if self.tk() == Tk::Virgula {
                    self.avanca();
                    if self.tk() == Tk::FechaParen {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
        self.exige(Tk::FechaParen, O_FECHA_PAREN)?;
        self.sai();
        if n != np {
            let mut e = self.e_nome(9, nome);
            e.b = np as u32;
            e.c = n as u32;
            return Err(e);
        }
        match idx {
            Some(i) => self.rem(&[0xE8], ALVO_FUNCAO, i as u32)?,
            None => self.rem(&[0xE8], ALVO_LE, 0)?,
        }
        if n > 0 {
            self.bs(&[0x48, 0x83, 0xC4, (8 * n) as u8])?; // add rsp, 8*n
        }
        Ok(Val { ty: ret, ini: nome as u32, solto: false, zero: false })
    }

    /// `print!("texto {}", args)` e `println!(...)`. Todos os argumentos são
    /// avaliados e empilhados antes de qualquer byte ser escrito.
    fn macro_(&mut self) -> R<Val> {
        let ini = self.pos as u32;
        let ln = self.tk() == Tk::Println;
        self.avanca();
        self.exige(Tk::Nao, O_EXCLAMACAO)?;
        if self.tk() != Tk::AbreParen {
            return Err(self.esperado(O_ABRE_PAREN));
        }
        self.entra()?;
        self.avanca();
        let mut ftok: Option<usize> = None;
        let mut marcas = 0u32;
        if !(ln && self.tk() == Tk::FechaParen) {
            if self.tk() != Tk::Texto {
                return Err(self.esperado(O_TEXTO));
            }
            let t = self.pos;
            marcas = self.confere_formato(t)?;
            ftok = Some(t);
            self.avanca();
        }
        let mut n = 0usize;
        while self.tk() == Tk::Virgula {
            self.avanca();
            if self.tk() == Tk::FechaParen {
                break;
            }
            if n == MAX_ARGS {
                let mut e = self.e(16, self.pos);
                e.a = 2;
                return Err(e);
            }
            let v = self.expr()?;
            if v.solto {
                return Err(self.e(11, v.ini as usize));
            }
            self.exige_ty(&v, Ty::I64)?;
            self.b(0x50)?; // push rax
            n += 1;
        }
        self.exige(Tk::FechaParen, O_FECHA_PAREN)?;
        self.sai();
        if marcas as usize != n {
            return Err(self.e(13, ftok.unwrap_or(self.pos)));
        }
        self.emite_escrita(ftok, ln, n)?;
        Ok(Val { ty: Ty::Unit, ini, solto: false, zero: false })
    }

    /// Confere as chaves de um texto de formato e conta os `{}` (M13 se há `{` ou
    /// `}` soltos).
    fn confere_formato(&self, tok: usize) -> R<u32> {
        let t = self.toks[tok];
        let mut i = t.off as usize + 1;
        let fim = (t.off + t.len) as usize - 1;
        let mut marcas = 0u32;
        while i < fim {
            match self.fonte[i] {
                b'\\' => i += 2,
                b'{' => {
                    if i + 1 < fim && self.fonte[i + 1] == b'}' {
                        marcas += 1;
                        i += 2;
                    } else {
                        return Err(self.e(13, tok));
                    }
                }
                b'}' => return Err(self.e(13, tok)),
                _ => i += 1,
            }
        }
        Ok(marcas)
    }

    /// Registra um byte de texto (conta para o tamanho, M17).
    fn texto_byte(&mut self, x: u8) -> R<()> {
        if self.exe.len() + self.textos.len() >= MAX_EXE || self.textos.len() >= CAP_TEXTOS {
            return Err(erro(17, SEM_POS));
        }
        self.textos.push(x);
        Ok(())
    }

    /// Escreve o trecho de texto que começa em `ini` (em `textos`) e vai até o
    /// fim: um `SYS_WRITE` por pedaço de até 4096 bytes.
    fn escreve_trecho(&mut self, ini: usize) -> R<()> {
        let total = self.textos.len() - ini;
        let mut feito = 0usize;
        while feito < total {
            let parte = (total - feito).min(PEDACO_TEXTO);
            self.rem(&[0x48, 0x8D, 0x3D], ALVO_TEXTO, (ini + feito) as u32)?; // lea rdi, [rip+texto]
            self.bs(&[0xBE])?; // mov esi, parte
            self.bs(&(parte as u32).to_le_bytes())?;
            self.bs(&[0xB8, 1, 0, 0, 0, 0x0F, 0x05])?; // mov eax, 1; syscall
            feito += parte;
        }
        Ok(())
    }

    /// Os `SYS_WRITE` de um `print!`/`println!`, depois que os `n` argumentos
    /// estão empilhados (o primeiro é o mais fundo).
    fn emite_escrita(&mut self, ftok: Option<usize>, ln: bool, n: usize) -> R<()> {
        let mut k = 0usize;
        let mut ini = self.textos.len();
        if let Some(tok) = ftok {
            let t = self.toks[tok];
            let mut i = t.off as usize + 1;
            let fim = (t.off + t.len) as usize - 1;
            while i < fim {
                let c = self.fonte[i];
                if c == b'\\' {
                    let d = match self.fonte[i + 1] {
                        b'n' => b'\n',
                        outro => outro,
                    };
                    self.texto_byte(d)?;
                    i += 2;
                } else if c == b'{' {
                    self.escreve_trecho(ini)?;
                    // mov rax, [rsp + 8*(n-1-k)]; call imprime
                    self.bs(&[0x48, 0x8B, 0x84, 0x24])?;
                    self.i32le(8 * (n - 1 - k) as i32)?;
                    self.rem(&[0xE8], ALVO_IMPRIME, 0)?;
                    k += 1;
                    ini = self.textos.len();
                    i += 2;
                } else {
                    self.texto_byte(c)?;
                    i += 1;
                }
            }
        }
        // O `\n` do `println!` vai colado ao último trecho de texto.
        if ln {
            self.texto_byte(b'\n')?;
        }
        self.escreve_trecho(ini)?;
        if n > 0 {
            self.bs(&[0x48, 0x83, 0xC4, (8 * n) as u8])?; // add rsp, 8*n
        }
        Ok(())
    }

    // ---- fim da passada 2 --------------------------------------------------

    /// Um panic: escreve o texto fixo e sai com 101. Alcançado por salto, sem
    /// usar a pilha.
    fn emite_panic(&mut self, msg: &[u8]) -> R<u32> {
        let off = self.exe.len() as u32;
        self.bs(&[0x48, 0x8D, 0x3D, 24, 0, 0, 0])?; // lea rdi, [rip+24] (o texto)
        self.bs(&[0xBE])?;
        self.bs(&(msg.len() as u32).to_le_bytes())?;
        self.bs(&[0xB8, 1, 0, 0, 0, 0x0F, 0x05])?; // SYS_WRITE
        self.bs(&[0xBF, 101, 0, 0, 0, 0xB8, 2, 0, 0, 0, 0x0F, 0x05])?; // SYS_EXIT(101)
        self.bs(msg)?;
        Ok(off)
    }

    /// Junta as rotinas de apoio usadas e os textos, resolve os remendos e
    /// escreve o cabeçalho.
    fn fecha(&mut self) -> R<()> {
        let mut total = self.exe.len() + self.textos.len() + TAM_PANIC + MSG_PILHA.len();
        if self.usa_estouro {
            total += TAM_PANIC + MSG_ESTOURO.len();
        }
        if self.usa_div0 {
            total += TAM_PANIC + MSG_DIV0.len();
        }
        if self.usa_imprime {
            total += ROT_IMPRIME.len();
        }
        if self.usa_le {
            total += ROT_LE_I64.len();
        }
        if total > MAX_EXE {
            return Err(erro(17, SEM_POS));
        }
        let off_pilha = self.emite_panic(MSG_PILHA)?;
        let off_estouro = if self.usa_estouro { self.emite_panic(MSG_ESTOURO)? } else { 0 };
        let off_div0 = if self.usa_div0 { self.emite_panic(MSG_DIV0)? } else { 0 };
        let off_imprime = self.exe.len() as u32;
        if self.usa_imprime {
            self.bs(ROT_IMPRIME)?;
        }
        let off_le = self.exe.len() as u32;
        if self.usa_le {
            self.bs(ROT_LE_I64)?;
        }
        let base_textos = self.exe.len() as u32;
        for i in 0..self.textos.len() {
            let x = self.textos[i];
            self.b(x)?;
        }
        for i in 0..self.remendos.len() {
            let r = self.remendos[i];
            let val = r.alvo & 0x0FFF_FFFF;
            let alvo = match r.alvo >> 28 {
                ALVO_FUNCAO => self.funcs[val as usize].addr,
                ALVO_ESTOURO => off_estouro,
                ALVO_DIV0 => off_div0,
                ALVO_PILHA => off_pilha,
                ALVO_IMPRIME => off_imprime,
                ALVO_LE => off_le,
                _ => base_textos + val,
            };
            let rel = i64::from(alvo) - (i64::from(r.pos) + 4);
            let p = r.pos as usize;
            self.exe[p..p + 4].copy_from_slice(&(rel as i32).to_le_bytes());
        }
        // Cabeçalho ELF64 e um program header (um `PT_LOAD`, leitura e execução).
        let n = self.exe.len() as u64;
        let mut h = [0u8; CABECALHO];
        h[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        h[4] = 2; // 64 bits
        h[5] = 1; // little-endian
        h[6] = 1; // versão
        h[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
        h[18..20].copy_from_slice(&0x3Eu16.to_le_bytes()); // x86-64
        h[20..24].copy_from_slice(&1u32.to_le_bytes());
        h[24..32].copy_from_slice(&ENTRADA.to_le_bytes());
        h[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
        h[52..54].copy_from_slice(&64u16.to_le_bytes()); // e_ehsize
        h[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
        h[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
        h[64..68].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
        h[68..72].copy_from_slice(&5u32.to_le_bytes()); // R + X
        h[80..88].copy_from_slice(&BASE.to_le_bytes()); // p_vaddr
        h[88..96].copy_from_slice(&BASE.to_le_bytes()); // p_paddr
        h[96..104].copy_from_slice(&n.to_le_bytes()); // p_filesz
        h[104..112].copy_from_slice(&n.to_le_bytes()); // p_memsz
        h[112..120].copy_from_slice(&0x1000u64.to_le_bytes()); // p_align
        self.exe[..CABECALHO].copy_from_slice(&h);
        Ok(())
    }

    /// Todas as etapas, na ordem de D6.
    fn compila(&mut self) -> R<()> {
        // Os tokens são o maior vetor, então o léxico vem antes de tudo.
        let mut toks = core::mem::take(&mut self.toks);
        let r = lexer(&self.fonte, &mut toks);
        self.toks = toks;
        r?;
        self.passada1()?;
        self.passada2()?;
        self.fecha()
    }
}

// ---------------------------------------------------------------------------
// 11. Saída
// ---------------------------------------------------------------------------

/// Grava o executável. Se não abre o arquivo, nada foi alterado. Se abriu e a
/// gravação falha depois, o arquivo já foi zerado: fecha, tenta apagar para não
/// deixar um executável quebrado, e devolve o erro.
fn grava(saida: &str, bytes: &[u8]) -> Result<(), String> {
    let mut w = match FileWriter::overwrite(saida) {
        Ok(w) => w,
        Err(e) => return Err(format!("nao consegui gravar '{}': {}", saida, e)),
    };
    if let Err(e) = w.write_all(bytes) {
        drop(w);
        let _ = remove(saida);
        return Err(format!("nao consegui gravar '{}': {}", saida, e));
    }
    Ok(())
}

/// Lê o fonte inteiro e fecha o arquivo. `Ok(None)` se passa de 16384 bytes.
fn le_fonte(caminho: &str) -> Result<Option<Vec<u8>>, String> {
    let mut arq = File::open(caminho).map_err(|e| format!("nao consegui abrir '{}': {}", caminho, e))?;
    let mut buf = vec![0u8; MAX_FONTE + 1];
    let mut len = 0usize;
    loop {
        if len > MAX_FONTE {
            return Ok(None);
        }
        let fim = (len + 4096).min(buf.len());
        match arq.read(&mut buf[len..fim]) {
            Ok(0) => break,
            Ok(n) => len += n,
            Err(e) => return Err(format!("nao consegui abrir '{}': {}", caminho, e)),
        }
    }
    if len > MAX_FONTE {
        return Ok(None);
    }
    buf.truncate(len);
    Ok(Some(buf))
}

// ---------------------------------------------------------------------------
// 12. main
// ---------------------------------------------------------------------------

const USO: &str = "nanorust: uso: exec nanorust <fonte> <saida>";

fn main() -> i32 {
    let mut area = [0u8; args::MAX_ARGS_LEN];
    let texto = args::read(&mut area);
    let mut palavras = args::words(texto);
    let (Some(fonte), Some(saida), None) = (palavras.next(), palavras.next(), palavras.next()) else {
        println!("{}", USO);
        return 1;
    };
    if fonte.eq_ignore_ascii_case(saida) {
        println!("erro: fonte e saida sao o mesmo arquivo");
        return 1;
    }
    let bytes = match le_fonte(fonte) {
        Ok(Some(b)) => b,
        Ok(None) => {
            let mut e = erro(16, 0);
            e.a = 0;
            println!("{}", mensagem(&[], &[], &e));
            return 1;
        }
        Err(msg) => {
            println!("erro: {}", msg);
            return 1;
        }
    };
    let mut comp = Comp::novo(bytes);
    if let Err(e) = comp.compila() {
        println!("{}", mensagem(&comp.fonte, &comp.toks, &e));
        return 1;
    }
    if let Err(msg) = grava(saida, &comp.exe) {
        println!("erro: {}", msg);
        return 1;
    }
    println!("nanorust: ok, {} bytes", comp.exe.len());
    0
}
