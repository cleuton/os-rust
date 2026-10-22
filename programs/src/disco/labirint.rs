//! `labirint`: gerador e resolvedor de labirinto em ASCII que existe **só no
//! disco**.
//!
//! É o port de um programa de Rust comum (`labirinto_demo`, que usava `std` e
//! a crate `rand`) para rodar em ring 3 no os-rust. O algoritmo é o mesmo do
//! original: o labirinto é gerado por backtracking com pilha explícita e
//! resolvido por busca em profundidade. O que mudou está marcado, ponto a
//! ponto, nos comentários "No original ..." (mudanças A a E):
//!
//! - A: `rand` virou `Aleatorio` (xorshift64), semeado pelo relógio do kernel;
//! - B: `std::env::args()` virou duas perguntas com `read_line`;
//! - C: `std` virou `no_std` + `alloc` + a biblioteca de runtime;
//! - D: o tamanho é limitado, para o desenho caber na tela VGA de 80x25;
//! - E: o programa espera um Enter entre o labirinto e a solução.
//!
//! O nome no disco tem 8 letras porque o FAT do projeto só aceita nomes 8.3 e
//! `labirinto` tem 9. Fica fora de `programs/src/bin/` de propósito: o
//! `build.rs` do kernel só embute os programas dessa pasta, então este só
//! existe em `/disco/bin/labirint`. Rode `run /disco/bin/labirint`.

#![no_std]
#![no_main]

// C. No original não havia nada disto: o `std` trazia `Vec` e `vec!` sozinho.
// Aqui o alocador vem da biblioteca de runtime e `extern crate alloc` dá
// acesso a `Vec` e a `vec!`.
extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
// C. No original: `use std::fmt;`. `core::fmt` é o mesmo módulo, sem `std`.
use core::fmt;
// C. No original: `println!` do `std`. Aqui `print!`/`println!` escrevem pelo
// kernel (syscall `write`) e `entry!` define o ponto de entrada do programa.
use runtime::{entry, print, println, read_line, time};

entry!(main);

// A. No original: `use rand::Rng;`, `rand::thread_rng()` e `gen_range`.
// Não existe `std` nem a crate `rand` em ring 3, e o projeto não ganha
// dependências, então o sorteio é este gerador pequeno, escrito aqui mesmo.
//
// O algoritmo é o xorshift64: guarda um número de 64 bits e, a cada sorteio,
// mistura os bits dele com três deslocamentos e xors (13, 7 e 17). Foi
// escolhido porque cabe em poucas linhas, não precisa de tabela e é fácil de
// explicar; a qualidade sobra para decidir que parede quebrar num desenho.
// O estado nunca pode ser zero (o xorshift ficaria preso em zero).
//
// A semente vem do relógio do kernel (ver `semente`): duas execuções em
// segundos diferentes geram labirintos diferentes.
struct Aleatorio {
    estado: u64,
}

/// Semente usada quando o relógio falha ou a mistura dá zero (a constante de
/// ouro do SplitMix64).
const SEMENTE_RESERVA: u64 = 0x9E37_79B9_7F4A_7C15;

impl Aleatorio {
    // Cria o gerador. A semente passa por um passo do SplitMix64 antes de
    // virar estado: o relógio muda pouco de uma execução para outra (um
    // segundo a mais mexe em poucos bits), e a mistura espalha essa diferença.
    fn novo(semente: u64) -> Self {
        let mut z = semente.wrapping_add(SEMENTE_RESERVA);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        // O xorshift não aceita estado zero.
        if z == 0 {
            z = SEMENTE_RESERVA;
        }
        Aleatorio { estado: z }
    }

    // Um passo do xorshift64; devolve o novo estado.
    fn proximo(&mut self) -> u64 {
        let mut x = self.estado;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.estado = x;
        x
    }

    // Número em `0..max` (`max` nunca é zero aqui: é 4 ou o tamanho do
    // labirinto, que tem no mínimo 2). O resto da divisão tem um viés
    // desprezível para `max` pequeno sobre 64 bits.
    fn intervalo(&mut self, max: usize) -> usize {
        (self.proximo() % max as u64) as usize
    }
}

// A. A semente: a data e a hora do kernel (UTC), empacotadas em um `u64`
// (cada campo na sua faixa de bits). Se o relógio falhar, usa a constante de
// reserva e o programa continua. A resolução é de um segundo.
fn semente() -> u64 {
    match time::now() {
        Ok(agora) => {
            (u64::from(agora.year) << 40)
                | (u64::from(agora.month) << 32)
                | (u64::from(agora.day) << 24)
                | (u64::from(agora.hour) << 16)
                | (u64::from(agora.minute) << 8)
                | u64::from(agora.second)
        }
        Err(_) => SEMENTE_RESERVA,
    }
}

// Definição da struct Celula (Célula do labirinto)
struct Celula {
    paredes: [bool; 4], // Paredes: Norte, Sul, Leste, Oeste
    visitada: bool,
    inicio: bool,
    fim: bool,
    x: usize,
    y: usize,
}

// Constantes para as direções
const NORTE: usize = 0;
const SUL: usize = 1;
const LESTE: usize = 2;
const OESTE: usize = 3;

// Definição da struct Pilha (Stack)
struct Pilha<T> {
    data: Vec<T>,
}

impl<T> Pilha<T> {
    fn new() -> Self {
        Pilha { data: Vec::new() }
    }

    fn push(&mut self, value: T) {
        self.data.push(value);
    }

    fn pop(&mut self) -> Option<T> {
        self.data.pop()
    }

    fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    fn top(&self) -> Option<&T> {
        self.data.last()
    }
}

// Definição da struct Labirinto
struct Labirinto {
    linhas: usize,
    colunas: usize,
    celulas: Vec<Vec<Celula>>,
    valido: bool,
    _corrente: Option<(usize, usize)>,
    _proxima: Option<(usize, usize)>,
    _qtd_total: usize,
    _qtd_visitadas: usize,
    pilha: Pilha<(usize, usize)>,
    caminho: Option<Vec<(usize, usize)>>,
}

impl Labirinto {
    // Método para criar um novo labirinto
    // A. No original: `fn new(linhas, colunas)`. O sorteio agora vem do
    // `Aleatorio` único criado em `main`, que precisa chegar até `criar` e
    // `pegar_vizinha` passando por `new`, `inicializar` e `processa_celula`.
    fn new(linhas: usize, colunas: usize, aleatorio: &mut Aleatorio) -> Self {
        let mut labirinto = Labirinto {
            linhas,
            colunas,
            celulas: Vec::new(),
            valido: false,
            _corrente: None,
            _proxima: None,
            _qtd_total: 0,
            _qtd_visitadas: 0,
            pilha: Pilha::new(),
            caminho: None,
        };
        labirinto.inicializar(aleatorio);
        labirinto
    }

    // Método para inicializar o labirinto
    fn inicializar(&mut self, aleatorio: &mut Aleatorio) {
        let mut contador = 0;
        while contador < 4 {
            self.celulas = Vec::new();
            for i in 0..self.linhas {
                let mut linha = Vec::new();
                for j in 0..self.colunas {
                    let celula = Celula {
                        paredes: [true, true, true, true],
                        visitada: false,
                        inicio: false,
                        fim: false,
                        x: j,
                        y: i,
                    };
                    linha.push(celula);
                }
                self.celulas.push(linha);
            }
            self.celulas[0][0].inicio = true;
            self.celulas[self.linhas - 1][self.colunas - 1].fim = true;
            contador += 1;
            self.pilha = Pilha::new();
            self._qtd_visitadas = 0;
            self.criar(aleatorio);
            // D. Este acesso a `celulas[1][1]` e a `celulas[linhas - 2][colunas - 2]`
            // é do original e falha com 1 linha ou 1 coluna: por isso o
            // tamanho mínimo é 2 (ver `main`).
            if !self.fechada(&self.celulas[1][1]) && !self.fechada(&self.celulas[self.linhas - 2][self.colunas - 2]) {
                break;
            }
        }
        if contador < 4 {
            self.valido = true;
        }
    }

    // Método para verificar se uma célula está fechada (todas as paredes intactas)
    fn fechada(&self, celula: &Celula) -> bool {
        celula.paredes[NORTE] && celula.paredes[SUL] && celula.paredes[LESTE] && celula.paredes[OESTE]
    }

    // Método para criar o labirinto usando backtracking
    // A. No original: `let mut rng = rand::thread_rng();` e `rng.gen_range(0..n)`.
    fn criar(&mut self, aleatorio: &mut Aleatorio) {
        self._qtd_total = self.linhas * self.colunas;
        let linha = aleatorio.intervalo(self.linhas);
        let coluna = aleatorio.intervalo(self.colunas);
        self._corrente = Some((linha, coluna));
        self.celulas[linha][coluna].visitada = true;
        if let Some(proxima) = self.pegar_vizinha(linha, coluna, aleatorio) {
            self.celulas[proxima.0][proxima.1].visitada = true;
            self.quebrar_paredes((linha, coluna), proxima);
            self.pilha.push((linha, coluna));
            self._qtd_visitadas += 1;
            self._corrente = Some(proxima);
            self.processa_celula(aleatorio);
        }
    }

    // Método para processar células durante a criação do labirinto
    fn processa_celula(&mut self, aleatorio: &mut Aleatorio) {
        loop {
            if !self.pilha.is_empty() {
                let corrente = self._corrente.unwrap();
                if self.is_dead_end(corrente) || self.celulas[corrente.0][corrente.1].fim || self.celulas[corrente.0][corrente.1].inicio {
                    if let Some(proxima) = self.pilha.pop() {
                        self._corrente = Some(proxima);
                    } else {
                        self._corrente = None;
                    }
                } else {
                    if let Some(proxima) = self.pegar_vizinha(corrente.0, corrente.1, aleatorio) {
                        self.quebrar_paredes(corrente, proxima);
                        self.pilha.push(corrente);
                        self.celulas[proxima.0][proxima.1].visitada = true;
                        self._qtd_visitadas += 1;
                        self._corrente = Some(proxima);
                    }
                }
            } else {
                self.celulas[0][0].paredes[NORTE] = false;
                self.celulas[self.linhas - 1][self.colunas - 1].paredes[SUL] = false;
                return;
            }
        }
    }

    // Método para verificar se uma célula é um beco sem saída
    fn is_dead_end(&self, celula: (usize, usize)) -> bool {
        let (y, x) = celula;
        if y > 0 && !self.celulas[y - 1][x].visitada {
            return false;
        }
        if y + 1 < self.linhas && !self.celulas[y + 1][x].visitada {
            return false;
        }
        if x > 0 && !self.celulas[y][x - 1].visitada {
            return false;
        }
        if x + 1 < self.colunas && !self.celulas[y][x + 1].visitada {
            return false;
        }
        true
    }

    // Método para quebrar as paredes entre duas células
    fn quebrar_paredes(&mut self, c1: (usize, usize), c2: (usize, usize)) {
        let (y1, x1) = c1;
        let (y2, x2) = c2;
        if x1 > x2 {
            self.celulas[y1][x1].paredes[OESTE] = false;
            self.celulas[y2][x2].paredes[LESTE] = false;
        } else if x1 < x2 {
            self.celulas[y1][x1].paredes[LESTE] = false;
            self.celulas[y2][x2].paredes[OESTE] = false;
        } else if y1 > y2 {
            self.celulas[y1][x1].paredes[NORTE] = false;
            self.celulas[y2][x2].paredes[SUL] = false;
        } else if y1 < y2 {
            self.celulas[y1][x1].paredes[SUL] = false;
            self.celulas[y2][x2].paredes[NORTE] = false;
        }
    }

    // Método para pegar uma célula vizinha não visitada
    // A. No original: `let mut rng = rand::thread_rng();` e `rng.gen_range(0..4)`.
    fn pegar_vizinha(&self, y: usize, x: usize, aleatorio: &mut Aleatorio) -> Option<(usize, usize)> {
        let mut procurar = true;
        let mut cel = None;
        while procurar {
            let vizinha = aleatorio.intervalo(4);
            match vizinha {
                NORTE => {
                    if y > 0 && !self.celulas[y - 1][x].visitada {
                        cel = Some((y - 1, x));
                        procurar = false;
                    }
                }
                SUL => {
                    if y + 1 < self.linhas && !self.celulas[y + 1][x].visitada {
                        cel = Some((y + 1, x));
                        procurar = false;
                    }
                }
                LESTE => {
                    if x + 1 < self.colunas && !self.celulas[y][x + 1].visitada {
                        cel = Some((y, x + 1));
                        procurar = false;
                    }
                }
                OESTE => {
                    if x > 0 && !self.celulas[y][x - 1].visitada {
                        cel = Some((y, x - 1));
                        procurar = false;
                    }
                }
                _ => {}
            }
        }
        cel
    }
}

// Implementação do Display para imprimir o labirinto
impl fmt::Display for Labirinto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut linhas = vec![vec![' '; (self.colunas * 3) + 1]; self.linhas * 3];
        for z in linhas.iter_mut() {
            z[self.colunas * 3] = '\n';
        }
        for i in 0..self.linhas {
            for j in 0..self.colunas {
                let matriz = self.get_celula(&self.celulas[i][j]);
                self.insert(&mut linhas, &matriz, i, j);
            }
        }
        for linha in linhas {
            for c in linha {
                write!(f, "{}", c)?;
            }
        }
        Ok(())
    }
}

impl Labirinto {
    // Método auxiliar para obter a representação de uma célula
    fn get_celula(&self, cel: &Celula) -> Vec<Vec<char>> {
        let mut linha1 = vec![' ', ' ', ' '];
        let mut linha2 = vec![' ', ' ', ' '];
        let mut linha3 = vec![' ', ' ', ' '];
        if cel.paredes[NORTE] {
            linha1 = vec!['-', '-', '-'];
        }
        if cel.paredes[SUL] {
            linha3 = vec!['-', '-', '-'];
        }
        if cel.paredes[OESTE] {
            linha1[0] = if linha1[0] == '-' { '+' } else { '|' };
            linha2[0] = '|';
            linha3[0] = if linha3[0] == '-' { '+' } else { '|' };
        }
        if cel.paredes[LESTE] {
            linha1[2] = if linha1[2] == '-' { '+' } else { '|' };
            linha2[2] = '|';
            linha3[2] = if linha3[2] == '-' { '+' } else { '|' };
        }
        // Marca o caminho se existir
        if let Some(ref caminho) = self.caminho {
            if caminho.contains(&(cel.y, cel.x)) {
                linha2[1] = '*';
            }
        }
        vec![linha1, linha2, linha3]
    }

    // Método auxiliar para inserir a representação de uma célula na matriz de caracteres
    fn insert(&self, linhas: &mut Vec<Vec<char>>, matriz: &Vec<Vec<char>>, i: usize, j: usize) {
        let linha = i * 2;
        let coluna = j * 2;
        for l in 0..3 {
            for c in 0..3 {
                linhas[linha + l][coluna + c] = matriz[l][c];
            }
        }
    }
}

// Definição da struct Solver (Resolvedor)
struct Solver {
    pilha: Pilha<(usize, usize)>,
    corrente: Option<(usize, usize)>,
    caminho: Vec<(usize, usize)>,
    incrementos: Vec<(isize, isize)>,
    visitadas: Vec<Vec<bool>>,
}

impl Solver {
    // Método para criar um novo Solver
    fn new() -> Self {
        Solver {
            pilha: Pilha::new(),
            corrente: None,
            caminho: Vec::new(),
            incrementos: vec![(0, -1), (0, 1), (1, 0), (-1, 0)], // Norte, Sul, Leste, Oeste
            visitadas: Vec::new(),
        }
    }

    // Método para resolver o labirinto
    fn solve(&mut self, labirinto: &mut Labirinto) {
        self.visitadas = vec![vec![false; labirinto.colunas]; labirinto.linhas];
        self.corrente = Some((0, 0));
        self.pilha.push((0, 0));
        self.procurar(labirinto);
        while let Some(pos) = self.pilha.pop() {
            self.caminho.push(pos);
        }
        labirinto.caminho = Some(self.caminho.clone());
    }

    // Método para procurar o caminho através do labirinto
    fn procurar(&mut self, labirinto: &Labirinto) {
        let mut buffer = None;
        while !self.pilha.is_empty() {
            let (y, x) = *self.pilha.top().unwrap();
            self.visitadas[y][x] = true;
            if labirinto.celulas[y][x].fim {
                return;
            }
            let mut proxima = None;
            for parede in 0..4 {
                if !labirinto.celulas[y][x].paredes[parede] {
                    let dy = self.incrementos[parede].1;
                    let dx = self.incrementos[parede].0;
                    let ny = y as isize + dy;
                    let nx = x as isize + dx;
                    if ny < 0 || nx < 0 || ny as usize >= labirinto.linhas || nx as usize >= labirinto.colunas {
                        continue;
                    }
                    let ny = ny as usize;
                    let nx = nx as usize;
                    if self.visitadas[ny][nx] {
                        continue;
                    }
                    proxima = Some((ny, nx));
                    if let Some(pos) = buffer {
                        self.pilha.push(pos);
                        buffer = None;
                    }
                    self.pilha.push((ny, nx));
                    self.corrente = Some((ny, nx));
                    break;
                }
            }
            if proxima.is_none() {
                let pos = self.pilha.pop().unwrap();
                self.corrente = Some(pos);
                buffer = Some(pos);
            }
        }
    }
}

// B e D. Pergunta um tamanho e devolve o valor escolhido.
// No original (B): `std::env::args()` trazia linhas e colunas, e um argumento
// que não fosse número virava 10. Aqui o `run` não passa argumentos a
// programas, então o tamanho é perguntado com `read_line`.
// D: o original aceitava qualquer tamanho. Na tela VGA de 80x25 o desenho
// ocupa `3 * linhas + 1` linhas e `3 * colunas` colunas, e ainda precisa de
// espaço para as perguntas, por isso os limites (2 a 7 linhas, 2 a 26
// colunas). O mínimo 2 evita o acesso a `celulas[1][1]` de `inicializar`.
// Resposta vazia assume o padrão em silêncio; qualquer outra coisa que não
// seja um número inteiro sem sinal dentro da faixa (letras, sinal, número
// gigante, linha longa demais) avisa e assume o padrão: nunca derruba o
// programa.
fn perguntar(texto: &str, minimo: usize, maximo: usize, padrao: usize) -> usize {
    print!("{}", texto);
    // 16 bytes bastam: o maior valor válido tem 2 dígitos, e uma linha maior
    // que 15 caracteres chega cortada e cai no caso "valor invalido".
    let mut buffer = [0u8; 16];
    let resposta = read_line(&mut buffer).trim();
    if resposta.is_empty() {
        return padrao;
    }
    // `parse` aceitaria "+3"; o enunciado pede inteiro sem sinal.
    let so_digitos = resposta.bytes().all(|b| b.is_ascii_digit());
    if so_digitos {
        if let Ok(valor) = resposta.parse::<usize>() {
            if (minimo..=maximo).contains(&valor) {
                return valor;
            }
        }
    }
    println!("valor invalido, usando {}", padrao);
    padrao
}

// Função principal
// C. No original: `fn main()` com `println!` do `std`. Aqui `main` devolve o
// código de saída (0 = sucesso) e é registrada por `entry!`.
fn main() -> i32 {
    let mut aleatorio = Aleatorio::novo(semente());

    // B. No original: `std::env::args()`.
    let linhas = perguntar("linhas (2 a 7, Enter = 5): ", 2, 7, 5);
    let colunas = perguntar("colunas (2 a 26, Enter = 10): ", 2, 26, 10);

    let mut labirinto = Labirinto::new(linhas, colunas, &mut aleatorio);
    println!("{}", labirinto);

    // E. No original o labirinto e a solução eram impressos em seguida. Dois
    // desenhos de até 22 linhas não cabem juntos na tela de 25 linhas, então
    // o programa espera um Enter (o texto digitado é ignorado) e só então
    // resolve e desenha de novo.
    print!("Enter para ver a solucao: ");
    let mut espera = [0u8; 16];
    let _ = read_line(&mut espera);

    let mut solver = Solver::new();
    solver.solve(&mut labirinto);
    println!("{}", labirinto);
    0
}
