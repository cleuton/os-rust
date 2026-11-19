[VERSÃO EM PORTUGUÊS](WALKTHROUGH-NANORUST.md)

# How `nanorust` works, inside

This document explains `programs/src/bin/nanorust.rs`, the os-rust compiler, for people who can program but have never written a compiler. The usage manual is [`NANORUST.en.md`](NANORUST.en.md); here the subject is the code. The file follows the order of this text, in numbered sections. Every function name quoted in backticks with parentheses, such as `lexer()`, exists in the file.

## 1. Why it is a user program

A compiler needs no privilege: it reads a file, computes and writes another. The kernel already offers that to any program, so `nanorust` **did not change the kernel** (nor the syscall contract, version 9, nor the runtime library). It runs in ring 3, like `hello`, and an error in it only ends it.

From the runtime the program uses `args::read` and `args::words` (the `exec` arguments), `File::open` and `read` (the source), `FileWriter::overwrite` and `write_all` (the executable), `remove` and `println!`. It uses `extern crate alloc` to have `Vec`, and `entry!(main)` links `main` to the entry point.

The **generated program** is another matter: it does not use the runtime. It talks to the kernel through just three contract syscalls, `SYS_WRITE`, `SYS_EXIT` and `SYS_READ_LINE`, written directly in machine code.

## 2. The phases

`Comp::compila()` runs four stages, in this order, and **stops at the first error**:

1. `lexer()` splits the whole source into tokens (errors M01, M02 and M03 for malformed text);
2. `Comp::passada1()` records the name, parameters and return type of each function, **skipping the bodies** by matching `{` and `}` (so a function can be called before it is defined);
3. `Comp::passada2()` checks the types **and generates the code in the same step**, with no syntax tree: the machine code comes out while the source is read;
4. `Comp::fecha()` joins the support routines and the texts, resolves the missing addresses and writes the ELF header.

Only afterwards, if nothing failed, `main()` calls `grava()`. That is why a compile error never creates or changes a file: everything happens in memory.

## 3. Errors are values

An `Erro` is a small struct of numbers (the code from M01 to M17, the byte offset, and up to three data fields). Every function in the compiler returns `R<T>`, which is `Result<T, Erro>`, and propagates with `?`. The **text** is only built in `mensagem()`, called by `main()` with an almost empty stack: no `format!` runs inside the recursion, because the program's stack is 16 KiB. Line and column are only computed there, by `linha_coluna()`, counting the `\n` up to the offset.

## 4. The lexer

A `Token` is 24 bytes (a `const _` checks it): the type (`Tk`), the offset and length in the source, and the value, if it is an integer. It stores no line or column. The reserved words are the first values of `Tk`, from `Fn` to `Res`, so `Tk::eh_reservada()` is a single comparison; `reservada()` decides whether a word is reserved. `lexer()` is a loop over the bytes: whitespace, `//` comments, identifiers, integers (checking the limit and a glued letter, M02), text (checking the escapes, M03) and the symbols, longest first (`->` before `-`).

The token vector is reserved with the source size plus 1 (the end token): there are never more tokens than bytes.

## 5. Types and what each expression returns

`Ty` has `I64`, `Bool`, `Unit` and `Nunca` (the `!`, which fits any type). Compiling an expression returns a `Val`: the type, the index of the expression's **first token** (where M08 and M11 point), and two flags: `solto` (only literals and operations: Rust would give `i32`) and `zero` (it is a literal 0, for M12). `exige_ty()` does the type check and builds M08.

## 6. Fixed memory

The heap is 1 MiB, and no table grows: `Comp::novo()` reserves everything at once, with `Vec::with_capacity`, from largest to smallest (tokens, executable, texts, patches, functions, parameters and variables). The capacities come from the limits: `fn f(){}` has 6 tokens, so at most `MAX_FONTE / 6` functions; each patch belongs to an instruction of 5 bytes or more, so at most `MAX_EXE / 5` patches. The total is near 690 KB.

## 7. Pass 1

`passada1()` walks the source looking only at headers. `cabecalho()` reads `fn name(params) [-> type]`, checks the function name (M07, M15) and **accepts any word as a parameter name** (pass 2 checks it, M07), and then skips the body counting braces. At the end, `main` must exist (M14). The function table keeps the index of the body's `{`, so pass 2 can restart from there.

## 8. Pass 2: stack-machine code

The convention is simple and is what keeps the generator small: **the result of every expression is in `rax`**; an operand waiting its turn is **on the stack**, never in a register (`syscall` destroys `rcx` and `r11`, and the code never depends on them).

`expr_prec()` handles all binary operators, in one function, with a precedence table (`Comp::prec()`: `||` 1, `&&` 2, comparisons 3, `+ -` 4, `* / %` 5). For `a + b`: it compiles `a` (in `rax`), emits `push rax`, compiles `b`, and emits `mov rcx, rax; pop rax; add rax, rcx; jo overflow`. A chain `a + b + c` is a **loop**, not recursion. One function for all levels is what keeps few stack frames per nesting level. `unario()` handles `-` and `!`; `primario()` handles integers, `true`/`false`, variables, parentheses, `if`, `print!`/`println!`, `return` and calls.

Arithmetic uses `jo` after `add`, `sub`, `imul` and `neg` for the overflow panic; division checks the zero divisor and `MIN / -1` before `idiv`. The `solto` and `zero` of each `Val` are updated here, and this is where M08, M11 and M12 come from.

### Nesting and the 16 KiB stack

`entra()` and `sai()` count nesting: `{}` blocks, parentheses, unary operators, `return` and the condition of an `if`/`while` while it is read. Past 10, M05. This limit is what guarantees the compiler's recursion fits in the stack. Measured in the release build: `expr_prec` and `bloco` use 128 bytes of frame, `unario` and `se` 112 (the rest is inlined into them); the worst level, with all five precedence levels full, costs about 1 KiB, so 10 levels plus the base come to about 11 KiB, within the 12 KiB budget.

## 9. Variables, blocks and functions

`funcao()` validates the parameter names, emits the prologue `push rbp; mov rbp, rsp; sub rsp, 8*L` (`L`, the number of locals, is only known at the end, so the `sub` is filled in later), the stack test, the body and the epilogue. The **stack test** compares `rsp` with `0x7FFFC800` and, if lower, **jumps** (does not call: the stack may be at its end) to the `pilha esgotada` panic. Parameter `i` of `n` is at `[rbp+16+8*(n-1-i)]` and local `k` at `[rbp-8*(k+1)]`.

`bloco()` is a loop of statements. Each `let` (`let_()`) gets a new slot, even when it shadows another. The initializer is compiled **before** the variable enters the table, which is why `let x: i64 = x + 1;` uses the old `x`. When the block closes, the variable table is cut back to the size it had. `atribuicao()` checks `let mut` (M10).

## 10. Control flow

`se()` emits `test rax, rax; jz` to the `else`. Forward jumps are **patched** when the destination appears: `salto()` emits the empty `rel32` and returns its position, and `aqui()` fixes it. The `else if` chain is a loop in `se()`; the jumps to the end form a **linked list inside the code itself** (each `rel32` holds the position of the previous one), so there is no table and the chain does not grow the stack. `junta_ramo()` joins the type of each branch. `enquanto()` makes the backward jump with `salto_volta()`. `ret()` emits the epilogue directly.

## 11. Calls and `print!`

`chamada()` evaluates the arguments from left to right, pushes each one, emits `call`, and the caller removes the arguments. Since the called function may be defined later, the `call` goes with a **patch** (`rem()`), resolved in `fecha()`.

`macro_()` evaluates **all** the arguments and pushes them, and only then `emite_escrita()` writes: each piece of text is a `SYS_WRITE` (`escreve_trecho()`, in chunks of up to 4096 bytes), and each `{}` loads the value from the stack and calls the `imprime` routine. `confere_formato()` validates the braces (M13). Texts live at the end of the file and are referenced by relative address (`lea rdi, [rip+...]`).

## 12. The end: support routines, patches and ELF

The support routines (`ROT_IMPRIME`, `ROT_LE_I64` and the three panics, through `emite_panic()`) are **hand-written byte tables**, each instruction commented, and only included if used. `fecha()` adds up the sizes, checks them against 65536 (M17), emits the routines and the texts, resolves all patches (`rel32 = target - (position + 4)`) and writes the header: an ELF64 `ET_EXEC` with **one** read-and-execute `PT_LOAD` at `0x4000_0000`, entry at `0x4000_0078` (right after the 120 header bytes). Every emitted byte goes through `Comp::b()`, which counts the size and gives M17 when it passes 65536.

The output is **deterministic**: no hash table, no heap address, no date. The same source always produces the same bytes.

## 13. Writing

`le_fonte()` reads the source in chunks and **closes** it before any write. `grava()` opens the output with `FileWriter::overwrite` and writes everything: if it cannot open it, nothing was changed; if the write fails afterwards (the file was already emptied), it deletes the broken file and shows the error (M19).

## 14. How it is tested

The compiler runs inside QEMU, and `tests/nanorust.rs` uses it for real through the prompt: it compiles, checks the ELF with the kernel's `elf::parse`, runs and compares the screen. The six examples in `programs/nano/exemplos/` were also checked with `rustc` (`cat programs/nano/prelude.rs example.rs`), and the tests keep the expected output.
