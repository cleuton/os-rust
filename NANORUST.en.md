[VERSÃO EM PORTUGUÊS](NANORUST.md)

# `nanorust`: the os-rust compiler

`nanorust` is a compiler for a **strict subset of Rust** that runs inside os-rust itself. It reads a source file, checks the types and
produces an ELF64 executable that the `run` command loads. Together with [`mano`](MANO.en.md) it closes the loop: **write** with `mano`,
**compile** with `nanorust`, **run** with `run`, all inside os-rust, without rebuilding the kernel and without a helper computer.

*Strict* means: everything `nanorust` accepts also compiles with `rustc` and behaves the same. The reverse does not hold: `nanorust` is much
smaller than Rust (see [Limits of the guarantee](#limits-of-the-guarantee)).

`nanorust` is an ordinary user program, built into the boot image (like `mano`). Its code is explained in the [walkthrough](WALKTHROUGH-NANORUST.en.md).

**A note on language:** the compiler's messages are the program's real output, so they stay in Portuguese, without accents, in this manual too.

## An end-to-end example

```text
os-rust> exec mano /ram/fibo.rs
```

Type the program below, save with `Ctrl+S` and quit with `Ctrl+X`:

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

The usage is always `exec nanorust <source> <output>`: **two** words, absolute paths. Any other number of words (including `run nanorust`, which passes no arguments) shows
`nanorust: uso: exec nanorust <fonte> <saida>`.

If there is an error, `nanorust` shows the line and column and **does not create or change any file**:

```text
os-rust> exec nanorust /ram/ruim.rs /ram/ruim
erro na linha 1, coluna 30: esperado expressao, encontrado ';'
[run] nanorust terminou com codigo 1
```

There are six ready-made examples in `programs/nano/exemplos/` (`ola`, `aritm`, `fibo`, `primos`, `logica`, `entrada`).

## Where the files live

- Paths are absolute and names follow the **8.3** format (up to 8 letters, a dot, up to 3). `/ram/fibo.rs` and `/ram/fibo` are fine.
- `/ram` **is cleared on every boot**: what you wrote and compiled there is gone when os-rust restarts.
- The `/disco` of `cargo run` uses `snapshot=on`: writes go to a temporary file and vanish when QEMU closes. To keep what you write, see
  [Keeping the disk between boots](README.en.md#keeping-the-disk-between-boots) in the README.
- The source and the output must be different files (upper and lower case do not count). `exec` limits the argument text to 119 bytes through the prompt, so the two paths together, with the space, cannot exceed that.
- `run` loads executables of up to 65536 bytes.

## The language

### Lexical rules

- **Whitespace:** space, tab, `\n` and `\r`. **Comment:** `//` to the end of the line (any byte is allowed inside). There is no `/* */`.
- **Identifier:** `[a-z_][a-z0-9_]*`, **lower case only**.
- **Reserved words** (cannot be names): a lone `_`; `as async await break const continue crate dyn else enum extern false fn for if
  impl in let loop match mod move mut pub ref return self static struct super trait true type unsafe use where while`; `abstract become box
  do final gen macro override priv try typeof unsized virtual yield`; `union macro_rules i64 bool print println`.
- **Integer:** `[0-9]+`, decimal, no `_`, no prefix and no suffix, at most `9223372036854775807`. An integer never has a sign: `-5` is the
  `-` operator applied to `5`. So the smallest `i64` is only obtained by calculation: `-9223372036854775807 - 1`.
- **Booleans:** `true` and `false`.
- **Text:** `"..."` with bytes from `0x20` to `0x7e` and the escapes `\n`, `\\` and `\"`. It only exists as the **first argument** of `print!` and `println!`.
- **Symbols:** `( ) { } , ; : -> = + - * / % == != < <= > >= && || !`.

### Grammar

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

- A statement that starts with `if` or `while` ends at the `}`, **takes no `;` after it** and does not continue as an operand. If it is the last item of the block, it is the block's value.
- The last expression without `;` before the `}` is the block's value. An assignment needs `;` and has no value.
- `return` without a value applies when the next token is `;` or `}`.
- `print!()` without text is an error; `println!()` without text prints just the line break.
- These do not exist: a bare block, a function inside a function, `pub`, attributes, `use`, `const`, `static`, `loop`, `break`, `continue`, `+=` and friends.

### Types

The types are `i64`, `bool`, `()` (that of functions without `->`, of `while` and of `println!`) and `!` (that of `return`). A value of type `!` fits wherever any type is expected.

1. `+ - * / %` and unary `-` work on `i64`. Unary `!` works on `bool`.
2. `< <= > >=` compare two `i64`; `==` and `!=` compare two `i64` or two `bool`. Comparisons **do not chain** (`a < b < c` is an error).
3. `&&` and `||` work on `bool` and short-circuit.
4. `if` and `while` need a `bool` condition. An `if` with `else` has both blocks of the same type; without `else`, the block is `()`. A `while` body is `()`.
5. The type of a block is that of its final value; with no value, it is `!` if some statement of it is `!`, otherwise `()`. A function body has the declared return type.
6. `let x: T = e;` needs `e` of type `T` (the annotation is mandatory). **Shadowing is allowed**: the scope of a `let` runs from the `;` to the end of the block,
   and the initializer sees the **previous** name (`let x: i64 = x + 1;` uses the `x` that already existed). Parameters are not mutable. Assignment needs `let mut`.
7. Functions can be recursive and called before being defined. `main` has no parameters and no return value and cannot be called.
8. Names: two functions cannot have the same name or that of a predefined one; a variable or parameter cannot have the name of any function
   (those in the source and `ler_i64`); two parameters of the same function cannot repeat.
9. **Loose integer.** An expression made only of integer literals, unary `-`, `+ - * / %`, parentheses and `if` with such blocks is *loose*: nothing in it fixes the
   type, and Rust would give `i32`. It is an error to use a loose expression (a) as an argument of `print!`/`println!`, (b) when **both sides** of a comparison
   are loose, or (c) as a statement `expr ;` (such as `2147483647 + 1;`). Everywhere else the context fixes `i64`. To print a calculation, store it first:
   `let x: i64 = 2 + 3; println!("{}", x);`.
10. Dividing or taking the remainder by an integer literal of value zero (`0` or `00`, with or without parentheses) is an error.

### Execution

- The order is left to right. In `print!`/`println!` **all** arguments are evaluated before any byte is written (as in Rust).
- `{}` prints an `i64` in decimal (with `-` if negative, including the smallest `i64`); it only accepts `i64` (a `bool` is an error). A stray `{` or `}`, or a number of `{}` different
  from the number of arguments, is an error. There is no `{{` or `}}`.
- Overflow in `+ - *`, in unary `-` and in `MIN / -1` and `MIN % -1`: `panic: estouro aritmetico`. A zero divisor in `/` and `%`: `panic: divisao por zero`.
  `/` truncates toward zero and `%` has the sign of the dividend, as in Rust.
- Exhausted stack (endless recursion): `panic: pilha esgotada`.
- Every panic writes `panic: <reason>` on the screen and ends with exit code **101**. `main` ending, or a `return` in `main`, ends with 0.

### The predefined function `ler_i64`

`ler_i64() -> i64` waits for a typed line. If the line (without the `\n`) matches `-?[0-9]{1,18}`, it returns the value; **otherwise it returns `0`**. There is no way to tell "typed 0" from "typed garbage":
that is the contract. Leading zeros count (`007` is 7). A line with 19 digits or more is invalid and gives `0`, **even if the value fits in `i64`**. The kernel delivers at most 23
characters per read and discards the rest of the line, so a long line gives `0` and the next read starts on the next line.

## Error table

`nanorust` stops at the **first** error. With a position: `erro na linha L, coluna C: <message>`; without one: `erro: <message>`. Line and column count from 1, in bytes.
Any error ends with exit code 1 and **no file is created or changed**. The messages are the ones the program shows (in Portuguese, without accents).

| Code | Message | Points to |
|---|---|---|
| M01 | `caractere inesperado '<c>'` or `byte inesperado 0x<hh>` | the byte |
| M02 | `numero invalido` | the first digit |
| M03 | `texto invalido` | the `"` that opens the text, or the misplaced text |
| M04 | `esperado <o que>, encontrado <token>` | the token found |
| M05 | `aninhamento demais (maximo 10)` | the token that would open level 11 |
| M06 | `'<nome>' nao existe` | the name |
| M07 | `nome '<nome>' invalido (reservado ou ja existe)` | the name in the declaration |
| M08 | `tipos incompativeis: esperado <T>, encontrado <U>` | the first token of the expression of the wrong type |
| M09 | `a funcao '<nome>' espera <n> argumentos, recebeu <m>` | the name in the call |
| M10 | `'<nome>' nao e mutavel (declare com let mut)` | the name in the assignment |
| M11 | `inteiro solto: use uma variavel ou chamada para fixar o tipo i64` | the first token of the loose expression |
| M12 | `divisao por zero literal` | the `/` or `%` operator |
| M13 | `texto de formato invalido` | the text |
| M14 | `funcao main nao encontrada` | line 1, column 1 |
| M15 | `main nao pode ter parametros nem retorno, nem ser chamada` | the name `main` |
| M16 | `limite excedido: fonte (maximo 16384 bytes)`, `... parametros (maximo 8)` or `... argumentos (maximo 8)` | line 1 column 1; the 9th parameter; the 9th argument |
| M17 | `programa grande demais (maximo 65536 bytes)` | no position |
| M18 | `nao consegui abrir '<caminho>': <erro>` | no position |
| M19 | `nao consegui gravar '<caminho>': <erro>` | no position |
| M20 | `fonte e saida sao o mesmo arquivo` | no position |

In M04, `<o que>` is `expressao`, `texto`, `identificador`, `tipo (i64 ou bool)`, `'fn'`, `';'`, `'{'`, `'}'`, `'('`, `')'`, `':'`, `'='` or `'!'`, and `<token>` is the token's text in single quotes,
`numero`, `texto` or `fim do fonte`. In M08, `<T>` and `<U>` are `i64`, `bool` or `()`.

Examples:

- `fn main() { let x: i64 = 1 + ; }` gives `erro na linha 1, coluna 30: esperado expressao, encontrado ';'`.
- `fn main() { let x: bool = 1; }` gives `erro na linha 1, coluna 27: tipos incompativeis: esperado bool, encontrado i64`.
- `fn main() { println!("{}"); }` gives `erro na linha 1, coluna 22: texto de formato invalido`.

A **write** error (M19) is different from a compile error. If `nanorust` cannot open the output (in use, invalid path, no space), nothing was changed. If it opened it and the write fails afterwards (the volume filled up),
the previous file **is lost**: `nanorust` tries to delete the broken file and shows the error.

## Limits

| What | Limit |
|---|---|
| Source | 16384 bytes |
| Nesting (`{`, `(`, unary operator, `return`, the condition of `if`/`while` while it is read) | 10 levels, and a function body is level 1 |
| Parameters per function | 8 |
| Arguments per call or per `print!`/`println!` (not counting the text) | 8 |
| Generated executable | 65536 bytes |

A chain `a + b + c`, a sequence of statements and an `else if` chain do **not** increase nesting. A sum of 1000 terms compiles (each `+ 1` produces about 21 bytes of code);
one of 8000 goes over 65536 bytes and gives M17.

## Limits of the guarantee

1. `rustc` rejects at compile time operations it can **prove** will panic (`arithmetic_overflow`, `unconditional_panic`), for example an overflow with visible constant operands. `nanorust` only rejects a divisor
   that is an integer literal of value zero, such as `0` or `00` (M12). A source accepted by `nanorust` may, in those cases, be rejected by `rustc`; that is why the panic examples get the dangerous value from `ler_i64` or from a parameter.
2. `rustc` warnings (unused variable, unnecessary `mut`, unreachable code) are not errors, and `nanorust` does not emit them.
3. Stack overflow: Rust on the computer ends by signal; the os-rust program ends with `panic: pilha esgotada` and exit code 101, and the possible recursion depth is different (the stack here is 16 KiB).
4. Rust's panic text goes to the error output, in English; in os-rust it goes to the screen, as `panic: <reason>`. The exit code (101) and the standard output before the panic are the same.
5. `nanorust` is **more restricted** than Rust: `rustc` accepting a source does not mean `nanorust` accepts it.

## Checking on the computer

To see that `rustc` accepts it and behaves the same, join the prelude (which defines `ler_i64`) to the source:

```sh
cat programs/nano/prelude.rs fonte.rs > x.rs && rustc x.rs
```

The prelude (`programs/nano/prelude.rs`) has only `#![allow(dead_code)]` and the `ler_i64` function, everything inside it, so no function of your source collides with a name of its own. A source checked this way can also go to os-rust by FTP
(see the [programmer's guide](PROGRAMMERS_GUIDE.md), section **Two ways to create programs**) and be compiled there.

## The examples

| File | What it shows |
|---|---|
| `ola.rs` | just `println!` with text and escapes |
| `aritm.rs` | every operator, precedence and the smallest `i64` obtained by calculation |
| `fibo.rs` | recursion and `while` |
| `primos.rs` | nested `while` and `%` |
| `logica.rs` | `&&`, `||` and `!` with functions that print (short-circuit) and `println!` arguments that print (everything is evaluated before writing) |
| `entrada.rs` | `ler_i64` with a valid number, a negative one, garbage, 19 digits and a 30-digit line |
