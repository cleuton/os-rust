[ENGLISH VERSION](NANORUST.en.md)

# `nanorust`: o compilador do os-rust

O `nanorust` é um compilador de um **subconjunto estrito de Rust** que roda dentro do próprio os-rust.
Ele lê um arquivo-fonte, confere os tipos e gera um executável ELF64 que o comando `run` carrega. Com o
[`mano`](MANO.md) fecha-se o ciclo: **escrever** com o `mano`, **compilar** com o `nanorust`, **rodar** com o
`run`, tudo dentro do os-rust, sem recompilar o kernel e sem computador de apoio.

*Estrito* quer dizer: tudo o que o `nanorust` aceita também compila no `rustc` e se comporta igual. O
contrário não vale: o `nanorust` é bem menor que o Rust (veja [Limites da garantia](#limites-da-garantia)).

O `nanorust` é um programa de usuário comum, embutido na imagem de boot (como o `mano`). O código está
explicado no [walkthrough](WALKTHROUGH-NANORUST.md).

## Um exemplo de ponta a ponta

```text
os-rust> exec mano /ram/fibo.rs
```

Digite o programa abaixo, grave com `Ctrl+S` e saia com `Ctrl+X`:

```rust
fn fib(n: i64) -> i64 {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn main() {
    let mut i: i64 = 0;
    while i <= 10 {
        println!("fib({}) = {}", i, fib(i));
        i = i + 1;
    }
}
```

```text
os-rust> exec nanorust /ram/fibo.rs /ram/fibo
nanorust: ok, 741 bytes
os-rust> run /ram/fibo
fib(0) = 0
fib(1) = 1
...
fib(10) = 55
```

O uso é sempre `exec nanorust <fonte> <saida>`: **duas** palavras, caminhos absolutos. Qualquer outra quantidade
(inclusive `run nanorust`, que não passa argumentos) mostra `nanorust: uso: exec nanorust <fonte> <saida>`.

Se há um erro, o `nanorust` mostra a linha e a coluna e **não cria nem altera nenhum arquivo**:

```text
os-rust> exec nanorust /ram/ruim.rs /ram/ruim
erro na linha 1, coluna 30: esperado expressao, encontrado ';'
[run] nanorust terminou com codigo 1
```

Há seis exemplos prontos em `programs/nano/exemplos/` (`ola`, `aritm`, `fibo`, `primos`, `logica`, `entrada`).

## Onde ficam os arquivos

- Os caminhos são absolutos e os nomes seguem o formato **8.3** (até 8 letras, ponto, até 3). `/ram/fibo.rs` e `/ram/fibo` servem.
- O `/ram` **zera a cada boot**: o que você escreveu e compilou nele some quando o os-rust reinicia.
- O `/disco` do `cargo run` usa `snapshot=on`: as gravações vão para um arquivo temporário e somem quando o QEMU fecha. Para guardar o que
  você grava, veja [Preservando o disco entre boots](README.md#preservando-o-disco-entre-boots), no README.
- O fonte e a saída precisam ser arquivos diferentes (maiúsculas e minúsculas não contam). O `exec` limita o texto de argumentos
  a 119 bytes pelo prompt, então os dois caminhos juntos, com o espaço, não podem passar disso.
- O `run` carrega executáveis de até 65536 bytes.

## A linguagem

### Léxico

- **Espaço:** espaço, tab, `\n` e `\r`. **Comentário:** `//` até o fim da linha (dentro dele vale qualquer byte). Não existe `/* */`.
- **Identificador:** `[a-z_][a-z0-9_]*`, **só minúsculas**.
- **Palavras reservadas** (não servem de nome): `_` sozinho; `as async await break const continue crate dyn else enum extern false fn for if
  impl in let loop match mod move mut pub ref return self static struct super trait true type unsafe use where while`; `abstract become box
  do final gen macro override priv try typeof unsized virtual yield`; `union macro_rules i64 bool print println`.
- **Inteiro:** `[0-9]+`, decimal, sem `_`, sem prefixo e sem sufixo, no máximo `9223372036854775807`. O inteiro nunca tem sinal: `-5` é o
  operador `-` aplicado a `5`. Por isso o menor `i64` só se obtém por conta: `-9223372036854775807 - 1`.
- **Booleanos:** `true` e `false`.
- **Texto:** `"..."` com bytes de `0x20` a `0x7e` e os escapes `\n`, `\\` e `\"`. Só existe como **primeiro argumento** de `print!` e `println!`.
- **Símbolos:** `( ) { } , ; : -> = + - * / % == != < <= > >= && || !`.

### Gramática

```
programa   = { funcao } ;
funcao     = "fn" IDENT "(" [ param { "," param } [ "," ] ] ")" [ "->" tipo ] bloco ;
param      = IDENT ":" tipo ;
tipo       = "i64" | "bool" ;

bloco      = "{" { instrucao } [ expr ] "}" ;
instrucao  = "let" [ "mut" ] IDENT ":" tipo "=" expr ";"
           | IDENT "=" expr ";"
           | expr_bloco
           | expr ";" ;
expr_bloco = se | "while" expr bloco ;

expr       = ou ;
ou         = e { "||" e } ;
e          = comparacao { "&&" comparacao } ;
comparacao = soma [ ( "==" | "!=" | "<" | "<=" | ">" | ">=" ) soma ] ;
soma       = termo { ( "+" | "-" ) termo } ;
termo      = unario { ( "*" | "/" | "%" ) unario } ;
unario     = ( "-" | "!" ) unario | primario ;
primario   = INT | "true" | "false" | IDENT | chamada | "(" expr ")"
           | se | macro | "return" [ expr ] ;
chamada    = IDENT "(" [ expr { "," expr } [ "," ] ] ")" ;
se         = "if" expr bloco [ "else" ( bloco | se ) ] ;
macro      = ( "print" | "println" ) "!" "(" [ TEXTO { "," expr } [ "," ] ] ")" ;
```

- Uma instrução que começa com `if` ou `while` termina no `}`, **não leva `;` depois** e não continua como operando. Se for o último item do
  bloco, é o valor dele.
- A última expressão sem `;` antes do `}` é o valor do bloco. A atribuição exige `;` e não tem valor.
- `return` sem valor vale quando o próximo token é `;` ou `}`.
- `print!()` sem texto é erro; `println!()` sem texto imprime só a quebra de linha.
- Não existem: bloco avulso, função dentro de função, `pub`, atributos, `use`, `const`, `static`, `loop`, `break`, `continue`, `+=` e companheiros.

### Tipos

Os tipos são `i64`, `bool`, `()` (o de funções sem `->`, de `while` e de `println!`) e `!` (o de `return`). Um valor de tipo `!` serve onde qualquer tipo é esperado.

1. `+ - * / %` e o `-` unário trabalham com `i64`. O `!` unário trabalha com `bool`.
2. `< <= > >=` comparam dois `i64`; `==` e `!=` comparam dois `i64` ou dois `bool`. Comparações **não se encadeiam** (`a < b < c` é erro).
3. `&&` e `||` trabalham com `bool` e têm curto-circuito.
4. `if` e `while` exigem condição `bool`. Um `if` com `else` tem os dois blocos do mesmo tipo; sem `else`, o bloco é `()`. O corpo do `while` é `()`.
5. O tipo de um bloco é o do valor final; sem valor, é `!` se alguma instrução dele é `!`, senão `()`. O corpo da função tem o tipo do retorno declarado.
6. `let x: T = e;` exige `e` do tipo `T` (a anotação é obrigatória). **Sombreamento é permitido**: o escopo de um `let` vai do `;` até o fim do bloco,
   e o inicializador enxerga o nome **anterior** (`let x: i64 = x + 1;` usa o `x` que já existia). Parâmetros não são mutáveis. A atribuição exige `let mut`.
7. Funções podem ser recursivas e chamadas antes de definidas. `main` não tem parâmetros nem retorno e não pode ser chamada.
8. Nomes: duas funções não podem ter o mesmo nome nem o de uma predefinida; uma variável ou parâmetro não pode ter o nome de nenhuma função
   (as do fonte e `ler_i64`); dois parâmetros da mesma função não podem se repetir.
9. **Inteiro solto.** Uma expressão feita só de literais inteiros, `-` unário, `+ - * / %`, parênteses e `if` com blocos assim é *solta*: nada nela fixa o
   tipo, e o Rust daria `i32`. É erro usar uma expressão solta (a) como argumento de `print!`/`println!`, (b) quando **os dois lados** de uma comparação
   são soltos, ou (c) como instrução `expr ;` (como `2147483647 + 1;`). Nos demais lugares o contexto fixa `i64`. Para imprimir uma conta, guarde-a antes:
   `let x: i64 = 2 + 3; println!("{}", x);`.
10. Dividir ou tirar o resto por um literal inteiro de valor zero (`0` ou `00`, com ou sem parênteses) é erro.

### Execução

- A ordem é da esquerda para a direita. Em `print!`/`println!` **todos** os argumentos são avaliados antes de qualquer byte ser escrito (como no Rust).
- `{}` imprime um `i64` em decimal (com `-` se negativo, inclusive o menor `i64`); só aceita `i64` (um `bool` é erro). Um `{` ou `}` solto, ou um número de `{}` diferente
  do de argumentos, é erro. Não existem `{{` nem `}}`.
- Estouro em `+ - *`, no `-` unário e em `MIN / -1` e `MIN % -1`: `panic: estouro aritmetico`. Divisor zero em `/` e `%`: `panic: divisao por zero`.
  `/` trunca em direção a zero e `%` tem o sinal do dividendo, como no Rust.
- Pilha esgotada (recursão sem fim): `panic: pilha esgotada`.
- Todo panic escreve `panic: <motivo>` na tela e termina com o código **101**. `main` terminar, ou um `return` em `main`, termina com 0.

### A função predefinida `ler_i64`

`ler_i64() -> i64` espera uma linha digitada. Se a linha (sem o `\n`) casa com `-?[0-9]{1,18}`, devolve o valor; **senão devolve `0`**. Não há como distinguir "digitou 0" de "digitou lixo":
esse é o contrato. Zeros à esquerda valem (`007` é 7). Uma linha com 19 dígitos ou mais é inválida e dá `0`, **mesmo que o valor caiba em `i64`**. O kernel entrega no máximo 23
caracteres por leitura e descarta o resto da linha, então uma linha longa dá `0` e a leitura seguinte começa na linha seguinte.

## Tabela de erros

O `nanorust` para no **primeiro** erro. Com posição: `erro na linha L, coluna C: <mensagem>`; sem posição: `erro: <mensagem>`. Linha e coluna contam a partir de 1, em bytes.
Qualquer erro termina com o código 1 e **nenhum arquivo é criado ou alterado**. As mensagens são as que o programa mostra (em português, sem acentos, mesmo neste manual em inglês).

| Código | Mensagem | Aponta para |
|---|---|---|
| M01 | `caractere inesperado '<c>'` ou `byte inesperado 0x<hh>` | o byte |
| M02 | `numero invalido` | o primeiro dígito |
| M03 | `texto invalido` | o `"` que abre o texto, ou o texto fora de lugar |
| M04 | `esperado <o que>, encontrado <token>` | o token encontrado |
| M05 | `aninhamento demais (maximo 10)` | o token que abriria o nível 11 |
| M06 | `'<nome>' nao existe` | o nome |
| M07 | `nome '<nome>' invalido (reservado ou ja existe)` | o nome na declaração |
| M08 | `tipos incompativeis: esperado <T>, encontrado <U>` | o primeiro token da expressão de tipo errado |
| M09 | `a funcao '<nome>' espera <n> argumentos, recebeu <m>` | o nome na chamada |
| M10 | `'<nome>' nao e mutavel (declare com let mut)` | o nome na atribuição |
| M11 | `inteiro solto: use uma variavel ou chamada para fixar o tipo i64` | o primeiro token da expressão solta |
| M12 | `divisao por zero literal` | o operador `/` ou `%` |
| M13 | `texto de formato invalido` | o texto |
| M14 | `funcao main nao encontrada` | linha 1, coluna 1 |
| M15 | `main nao pode ter parametros nem retorno, nem ser chamada` | o nome `main` |
| M16 | `limite excedido: fonte (maximo 16384 bytes)`, `... parametros (maximo 8)` ou `... argumentos (maximo 8)` | linha 1 coluna 1; o 9º parâmetro; o 9º argumento |
| M17 | `programa grande demais (maximo 65536 bytes)` | sem posição |
| M18 | `nao consegui abrir '<caminho>': <erro>` | sem posição |
| M19 | `nao consegui gravar '<caminho>': <erro>` | sem posição |
| M20 | `fonte e saida sao o mesmo arquivo` | sem posição |

Em M04, `<o que>` é `expressao`, `texto`, `identificador`, `tipo (i64 ou bool)`, `'fn'`, `';'`, `'{'`, `'}'`, `'('`, `')'`, `':'`, `'='` ou `'!'`, e `<token>` é o texto do token entre aspas simples,
`numero`, `texto` ou `fim do fonte`. Em M08, `<T>` e `<U>` são `i64`, `bool` ou `()`.

Exemplos:

- `fn main() { let x: i64 = 1 + ; }` dá `erro na linha 1, coluna 30: esperado expressao, encontrado ';'`.
- `fn main() { let x: bool = 1; }` dá `erro na linha 1, coluna 27: tipos incompativeis: esperado bool, encontrado i64`.
- `fn main() { println!("{}"); }` dá `erro na linha 1, coluna 22: texto de formato invalido`.

Um erro de **gravação** (M19) é diferente de um erro de compilação. Se o `nanorust` não consegue abrir a saída (em uso, caminho inválido, sem espaço), nada foi alterado. Se abriu e a gravação falha depois (o volume encheu),
o arquivo anterior **se perdeu**: o `nanorust` tenta apagar o arquivo quebrado e mostra o erro.

## Limites

| O que | Limite |
|---|---|
| Fonte | 16384 bytes |
| Aninhamento (`{`, `(`, operador unário, `return`, condição de `if`/`while` enquanto é lida) | 10 níveis, e o corpo de uma função é o nível 1 |
| Parâmetros por função | 8 |
| Argumentos por chamada ou por `print!`/`println!` (sem contar o texto) | 8 |
| Executável gerado | 65536 bytes |

Uma cadeia `a + b + c`, uma sequência de instruções e uma cadeia `else if` **não** aumentam o aninhamento. Uma soma de 1000 termos compila (cada `+ 1` gera cerca de 21 bytes de código);
uma de 8000 passa de 65536 bytes e dá M17.

## Limites da garantia

1. O `rustc` recusa na compilação operações que ele consegue **provar** que dão panic (`arithmetic_overflow`, `unconditional_panic`), por exemplo um estouro com operandos constantes visíveis. O `nanorust` só recusa o divisor
   que é literal inteiro de valor zero, como `0` ou `00` (M12). Um fonte aceito pelo `nanorust` pode, nesses casos, ser recusado pelo `rustc`; por isso os exemplos de panic obtêm o valor perigoso de `ler_i64` ou de um parâmetro.
2. Avisos do `rustc` (variável não usada, `mut` desnecessário, código inalcançável) não são erros, e o `nanorust` não os emite.
3. Estouro de pilha: o Rust no computador termina por sinal; o programa do os-rust termina com `panic: pilha esgotada` e código 101, e a profundidade de recursão possível é diferente (a pilha aqui é de 16 KiB).
4. O texto do panic no Rust vai para a saída de erro, em inglês; no os-rust vai para a tela, como `panic: <motivo>`. O código de término (101) e a saída padrão antes do panic são iguais.
5. O `nanorust` é **mais restrito** que o Rust: o `rustc` aceitar um fonte não quer dizer que o `nanorust` aceita.

## Conferindo no computador

Para ver que o `rustc` aceita e se comporta igual, junte o prelúdio (que define `ler_i64`) ao fonte:

```sh
cat programs/nano/prelude.rs fonte.rs > x.rs && rustc x.rs
```

O prelúdio (`programs/nano/prelude.rs`) tem só `#![allow(dead_code)]` e a função `ler_i64`, tudo dentro dela, para nenhuma função do seu fonte colidir com um nome dele. Um fonte conferido assim também pode ir para o os-rust por FTP
(veja o [guia do programador](GUIA_DO_PROGRAMADOR.md), seção **Duas formas de criar programas**) e ser compilado lá.

## Os exemplos

| Arquivo | O que mostra |
|---|---|
| `ola.rs` | só `println!` com texto e escapes |
| `aritm.rs` | todos os operadores, a precedência e o menor `i64` obtido por conta |
| `fibo.rs` | recursão e `while` |
| `primos.rs` | `while` aninhado e `%` |
| `logica.rs` | `&&`, `||` e `!` com funções que imprimem (o curto-circuito) e argumentos de `println!` que imprimem (tudo é avaliado antes de escrever) |
| `entrada.rs` | `ler_i64` com número válido, negativo, lixo, 19 dígitos e uma linha de 30 dígitos |
