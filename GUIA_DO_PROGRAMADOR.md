# Guia do Programador - os-rust

Este guia é para quem quer escrever um programa que roda dentro do os-rust, sem precisar entender o kernel por dentro. Se você quer saber como o boot, a memória ou as interrupções funcionam, o documento certo é o `WALKTHROUGH.md`. Este aqui assume que você só quer escrever código que rode em modo usuário, usando a biblioteca de runtime do projeto.

## Pré-requisitos

Os mesmos três do `README.md`: Rust com a toolchain nightly pinada pelo projeto (`rust-toolchain.toml`), `bootimage` e QEMU. Se você já roda `cargo run` com sucesso na raiz do repositório, está pronto para seguir este guia.

## Como um programa de usuário é organizado

Um programa de usuário não é compilado junto com o kernel. Ele é um arquivo Rust `no_std`, compilado para um target de modo usuário (diferente do target bare-metal do kernel), e o executável resultante é embutido na imagem de boot em tempo de compilação. Isso acontece automaticamente dentro de `cargo run`/`cargo test`: você não precisa rodar nenhum comando de compilação separado.

Cada programa é **um arquivo** em `programs/src/bin/`, por exemplo `programs/src/bin/hello.rs`. O nome do arquivo, sem `.rs`, é o nome que você digita depois de `run`. Não existe `Cargo.toml` por programa: todos compartilham o de `programs/`, que já depende da biblioteca de runtime.

## A biblioteca de runtime

A biblioteca de runtime (`runtime/`, na raiz do repositório) existe para que você não precise chamar syscalls na mão. Ela oferece:

- `entry!(main)`: liga a sua função `main` ao ponto de entrada (`_start`) do programa. Quando `main` retorna, a biblioteca chama `exit` com o valor devolvido.
- `print!` e `println!`: escrevem texto na tela, através da syscall de escrita.
- `read_line(&mut buffer)`: espera a pessoa digitar uma linha e pressionar Enter, e devolve o que foi digitado (sem o Enter) como texto, através da syscall de leitura de teclado. O sistema mostra na tela o que é digitado e apaga com Backspace; a linha tem no máximo 127 caracteres.
- Um alocador global: permite usar `Box`, `Vec`, `String` e o restante da crate `alloc` (com `extern crate alloc;` no seu programa), através da syscall de memória. O heap do programa cresce até 1 MiB.
- `exit(codigo)`: encerra o programa mais cedo, se precisar.
- Um tratador de `panic!`: escreve `[panic] <mensagem>` na tela e encerra o programa com o código 101.

Com isso, o único código de baixo nível que você precisa escrever é a sua lógica: ler, processar, escrever.

## Estrutura mínima de um programa

```rust
#![no_std]
#![no_main]

use runtime::{entry, println};

entry!(main);

fn main() -> i32 {
    println!("ola, os-rust!");
    0
}
```

Repare em quatro coisas:

1. `#![no_std]` e `#![no_main]`: como no kernel, um programa de usuário não tem a biblioteca padrão nem um `main` convencional por baixo.
2. `entry!(main);` gera o ponto de entrada e o liga à sua função. O compilador confere que `main` é `fn() -> i32`.
3. O valor que `main` devolve vira o código de saída do programa: `0` significa sucesso e não mostra nada; qualquer outro valor faz o sistema escrever `[run] <nome> terminou com codigo <n>`.
4. `println!` vem da biblioteca de runtime (`runtime::println`), não da biblioteca padrão.

## O contrato de syscalls, resumido

O documento oficial e versionado do contrato de syscalls é o [`SYSCALLS.md`](SYSCALLS.md), na raiz do repositório. Ele é a fonte da verdade para números, convenção de registradores, códigos de erro e limites. Este guia só resume o que cada syscall faz, porque na prática você vai usar a biblioteca de runtime, não a syscall diretamente:

| Nº | Syscall | O que faz | Como você usa |
|---|---|---|---|
| 1 | `SYS_WRITE` | Escreve texto na tela | `print!`/`println!` |
| 2 | `SYS_EXIT` | Encerra o programa e devolve um código de saída | o retorno de `main`, ou `exit(codigo)` |
| 3 | `SYS_READ_LINE` | Espera uma linha digitada, até o Enter | `read_line(&mut buffer)` |
| 4 | `SYS_ALLOC` | Aumenta o heap do programa | o alocador global, usado por `Box`/`Vec`/`String` |

Se seu programa precisar chamar uma syscall diretamente (fora do que a biblioteca de runtime já cobre), consulte o `SYSCALLS.md` para o número e a convenção exatos.

## Programa de exemplo 1: eco

Lê uma linha do teclado, escreve o mesmo texto de volta e conta quantas palavras ele tem. Demonstra o ciclo completo de entrada e saída de um programa de usuário, e o uso de `Vec` pelo alocador da biblioteca. Arquivo: `programs/src/bin/eco.rs`.

```rust
//! `eco`: lê uma linha do teclado e responde.
//!
//! Mostra o ciclo completo de um programa de usuário: escrever (`print!`),
//! ler do teclado (`read_line`) e usar memória dinâmica (`Vec`), tudo por
//! syscalls do kernel, através da biblioteca de runtime. O programa nunca
//! toca no hardware do teclado nem na tela: só pede ao kernel.

#![no_std]
#![no_main]

// O alocador global vem da biblioteca de runtime; `extern crate alloc`
// dá acesso a `Vec`, `Box` e `String`.
extern crate alloc;

use alloc::vec::Vec;
use runtime::{entry, print, println, read_line};

entry!(main);

fn main() -> i32 {
    print!("digite algo: ");

    // O buffer é do programa. `read_line` escreve nele a linha digitada e
    // devolve o texto, sem o Enter. Cabem até 127 caracteres.
    let mut buffer = [0u8; 128];
    let texto = read_line(&mut buffer);

    // A resposta: exatamente o que foi digitado.
    println!("voce digitou: {}", texto);

    // `collect` cria um `Vec`: a primeira alocação faz a biblioteca pedir
    // memória ao kernel (syscall `SYS_ALLOC`).
    let palavras: Vec<&str> = texto.split_whitespace().collect();
    println!("palavras: {}", palavras.len());

    0
}
```

Rodando:

```text
os-rust> run eco
digite algo: Ola mundo
voce digitou: Ola mundo
palavras: 2
os-rust>
```

## Programa de exemplo 2: falha_memoria

Acessa de propósito um endereço de memória que não pertence ao programa, para demonstrar que o kernel isola a falha: só o programa é encerrado, o kernel continua de pé. Arquivo: `programs/src/bin/falha_memoria.rs`.

```rust
//! `falha_memoria`: um programa que erra de propósito.
//!
//! Escreve num endereço de memória que não é dele. O processador levanta
//! uma exceção de falha de página (`#PF`); o kernel a trata, encerra só
//! este programa, mostra uma mensagem legível e devolve o controle ao
//! prompt. Uma falha em programa de usuário nunca derruba o kernel
//! (`SYSCALLS.md`, seção 7).

#![no_std]
#![no_main]

use runtime::{entry, println};

entry!(main);

fn main() -> i32 {
    println!("prestes a acessar memoria invalida...");

    // SAFETY: isto é **intencionalmente** inseguro. `0xdead_beef` está fora
    // da memória deste programa, então a escrita vira uma exceção de CPU
    // (`#PF`), tratada pelo kernel: é exatamente o comportamento que este
    // programa existe para mostrar.
    unsafe {
        core::ptr::write_volatile(0xdead_beef as *mut u8, 42);
    }

    // Esta linha nunca é alcançada.
    println!("se voce esta vendo isso, o isolamento falhou");
    0
}
```

Rodando:

```text
os-rust> run falha_memoria
prestes a acessar memoria invalida...
[run] falha_memoria encerrado por erro de memoria: #PF (Page Fault) em 0x40000015
codigo de erro: 0x6
endereco de falha: 0xdeadbeef
os-rust>
```

O endereço depois de `em` muda de uma compilação para outra; o `endereco de falha` é o que o programa tentou acessar. Logo depois, o prompt continua funcionando: `run hello` e `run eco` rodam normalmente.

## Escrevendo e compilando o seu próprio programa

1. Crie um arquivo novo em `programs/src/bin/`, com o nome do seu programa (por exemplo, `programs/src/bin/meuprograma.rs`).
2. Comece pela estrutura mínima da seção acima: `#![no_std]`, `#![no_main]`, `entry!(main);` e `fn main() -> i32`. Não crie `Cargo.toml`: os programas compartilham o de `programs/`, que já depende da biblioteca de runtime.
3. Escreva a lógica do seu programa usando `print!`/`println!`, `read_line`, e `Box`/`Vec` se precisar (com `extern crate alloc;`).
4. Rode `cargo run` na raiz do repositório. Seu programa é compilado automaticamente junto com os demais, e embutido na imagem de boot.
5. No prompt do os-rust, digite `run meuprograma`. Se o nome não bater com nenhum programa embutido, o prompt lista os nomes disponíveis, então confira o nome do seu arquivo.

Não existe nenhum passo de compilação manual separado: se `cargo run` funciona, seu programa está embutido.

## Testando

A forma mais simples de testar é manual: `cargo run`, depois `run <nome do seu programa>`, observando a tela.

Para verificação automatizada, `cargo test` roda a suíte do kernel, que inclui testes cobrindo o carregamento e a execução de programas de usuário (usando `hello`, `eco` e `falha_memoria` como referência). Se você quiser um teste dedicado ao seu próprio programa, o padrão usado pelo projeto é um arquivo em `tests/`, dando boot no kernel, "digitando" a entrada com `interrupts::push_scancode`, e verificando o resultado na tela; `tests/user_runtime.rs` é o modelo.

## Erros comuns

- **Meu programa não aparece em `run`**: confira se o nome do arquivo em `programs/src/bin/` (sem `.rs`) é exatamente o que você digitou depois de `run`, e se `cargo run` terminou sem erro de compilação.
- **`cargo run` falha ao compilar meu programa**: o erro apontado é do seu código, não do kernel; corrija e rode `cargo run` de novo. O kernel não sobe com um programa que falhou ao compilar.
- **Erro de ligação dizendo que falta `_start`**: faltou `entry!(main);` no seu programa.
- **Meu programa trava e nunca volta ao prompt**: confira se todo caminho de código do seu `main` eventualmente retorna (ou chama `exit`); um programa que nunca sai continua ocupando o prompt. Se ele está em `read_line`, está esperando você apertar Enter.
- **`memory allocation of N bytes failed` e `terminou com codigo 101`**: o programa pediu mais memória do que o heap de 1 MiB comporta (a mensagem vem depois de uma linha `[panic] panicked at ...`).
- **Meu programa usa uma syscall que não existe**: o kernel encerra o programa com uma mensagem de erro, do mesmo jeito que trata um acesso inválido à memória; confira o número da syscall no `SYSCALLS.md`.
