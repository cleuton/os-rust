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
- `print!` e `println!`: escrevem texto na tela, através da syscall de escrita. Cada chamada vira **uma** escrita (para textos de até 256 bytes), então uma linha nunca é partida pela saída de outro programa.
- `read_line(&mut buffer)`: espera a pessoa digitar uma linha e pressionar Enter, e devolve o que foi digitado (sem o Enter) como texto, através da syscall de leitura de teclado. O sistema mostra na tela o que é digitado e apaga com Backspace; a linha tem no máximo 127 caracteres.
- Um alocador global: permite usar `Box`, `Vec`, `String` e o restante da crate `alloc` (com `extern crate alloc;` no seu programa), através da syscall de memória. O heap do programa cresce até 1 MiB.
- `yield_now()`: cede a CPU a outro programa que esteja rodando ao mesmo tempo (veja a seção **Multitarefa**).
- `File`, `Dir`, `DirEntry` e `FsError`: abrir e ler arquivos e listar diretórios, somente leitura (veja a seção **Arquivos**).
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
| 5 | `SYS_YIELD` | Cede a CPU a outro programa | `yield_now()` |
| 6 | `SYS_OPEN` | Abre um arquivo ou diretório (somente leitura) | `File::open`, `Dir::open` |
| 7 | `SYS_READ` | Lê bytes de um arquivo aberto | `File::read` |
| 8 | `SYS_CLOSE` | Fecha um arquivo ou diretório | automático, quando o `File`/`Dir` sai de escopo |
| 9 | `SYS_READ_DIR` | Lê a próxima entrada de um diretório aberto | `Dir::next` |

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

## Multitarefa

Até agora os exemplos rodaram um de cada vez. O os-rust também roda **vários programas ao mesmo tempo**: `run <nome> [<nome>...]` carrega todos os programas pedidos (até 4) antes de começar qualquer um, e só devolve o prompt quando o último termina. O mesmo nome pode se repetir (`run ping ping` roda duas cópias independentes). Se um nome não existe, ou se pedir mais de 4, nenhum programa é iniciado: o prompt diz o problema e lista os programas disponíveis.

### O que muda para o seu programa

- **A memória é só sua.** Cada programa vê o mesmo mapa de endereços do `SYSCALLS.md`, mas ninguém mais enxerga nem altera as páginas dele, e ele não enxerga as dos outros. Duas cópias do mesmo programa não compartilham nem as variáveis.
- **Você não percebe as trocas.** O kernel guarda o estado inteiro do seu programa (registradores, pilha, memória) e o retoma exatamente de onde parou. Mas ele pode ser interrompido em **qualquer** instrução: não assuma nenhuma ordem de execução entre programas, nem tempo.
- **A CPU é compartilhada.** Um timer interrompe o programa que calcula por tempo demais (40 a 50 ms seguidos) e passa a CPU ao próximo. Por isso um programa que só calcula, sem pedir nada, não trava os outros.
- **Cada `print!`/`println!` sai inteiro.** Uma linha escrita por uma chamada não é partida pela saída de outro programa (até 256 bytes por chamada).
- **Esperar o teclado não gasta CPU.** Enquanto um programa espera em `read_line`, os outros seguem rodando. Se mais de um espera, a linha digitada vai ao que pediu **primeiro**; nenhuma tecla vai a dois programas nem se perde.
- **Uma falha só derruba quem falhou.** Se um programa acessa memória inválida, só ele é encerrado, com a mensagem de sempre, na hora em que isso acontece; os outros continuam, e o prompt volta quando o último termina.

### `yield_now`: ceder a CPU de propósito

`yield_now()` diz ao kernel: "pode passar a vez". O kernel guarda o estado do programa, roda o próximo programa pronto, e só volta ao seu quando chegar a vez dele, na linha seguinte. Se nenhum outro programa está pronto, volta na hora. Não é preciso chamar `yield_now` para os outros programas rodarem (o timer cuida disso); chamar serve para dois programas se **alternarem** de forma previsível. A chamada nunca falha e não devolve nada.

### Programa de exemplo 3: ping

Escreve uma linha e cede a CPU ao outro programa, quatro vezes. Arquivo: `programs/src/bin/ping.rs`.

```rust
//! `ping`: o primeiro de dois programas que se alternam na tela.
//!
//! Escreve uma linha e **cede a CPU** (`yield_now`) ao outro programa; quando
//! chega a vez dele de novo, escreve a próxima. Rode `run ping pong` e as
//! linhas de `ping` e de `pong` aparecem intercaladas, na ordem. Cada
//! programa tem a memória só dele: o contador `i` de um não é visto pelo outro.

#![no_std]
#![no_main]

use runtime::{entry, println, yield_now};

entry!(main);

fn main() -> i32 {
    for i in 1..=4 {
        // Pede ao kernel (syscall `write`) que escreva a linha na tela. A
        // linha chega inteira, mesmo com outro programa rodando.
        println!("ping {}", i);

        // Pede ao kernel (syscall `yield`) que passe a CPU ao próximo
        // programa pronto. Só volta aqui quando chegar a vez deste de novo.
        yield_now();
    }
    0
}
```

### Programa de exemplo 4: pong

O segundo da dupla: igual ao `ping`, com outro texto. Arquivo: `programs/src/bin/pong.rs`.

```rust
//! `pong`: o segundo de dois programas que se alternam na tela.
//!
//! É igual ao `ping`, com outro texto. Rode `run ping pong` e as linhas dos
//! dois aparecem intercaladas, na ordem: cada um escreve uma linha e **cede a
//! CPU** (`yield_now`) ao outro. Cada programa tem a memória só dele: o
//! contador `i` de um não é visto pelo outro.

#![no_std]
#![no_main]

use runtime::{entry, println, yield_now};

entry!(main);

fn main() -> i32 {
    for i in 1..=4 {
        // Pede ao kernel (syscall `write`) que escreva a linha na tela. A
        // linha chega inteira, mesmo com outro programa rodando.
        println!("pong {}", i);

        // Pede ao kernel (syscall `yield`) que passe a CPU ao próximo
        // programa pronto. Só volta aqui quando chegar a vez deste de novo.
        yield_now();
    }
    0
}
```

Rode os dois juntos:

```text
os-rust> run ping pong
ping 1
pong 1
ping 2
pong 2
ping 3
pong 3
ping 4
pong 4
os-rust>
```

Cada um escreve uma linha e cede a CPU ao outro: as linhas aparecem alternadas.

### Programa de exemplo 5: contador_a

Conta em um laço longo e **nunca cede a CPU**. Arquivo: `programs/src/bin/contador_a.rs`.

```rust
//! `contador_a`: conta em laço longo, **sem nunca ceder a CPU**.
//!
//! Este programa não pede a vez a ninguém: ele só conta, como se fosse o único
//! programa do sistema. Mesmo assim, rodando ao lado do `contador_b`
//! (`run contador_a contador_b`), as linhas dos dois aparecem intercaladas na
//! tela, porque o timer do kernel interrompe cada programa depois de uma
//! fatia de tempo e passa a CPU ao outro. Quem manda na CPU é o kernel, não o
//! programa.

#![no_std]
#![no_main]

use core::hint::black_box;
use runtime::{entry, println};

entry!(main);

/// Quantas iterações o programa conta entre uma linha e a seguinte. É grande
/// de propósito: leva várias fatias de tempo do timer, então as linhas dos
/// dois programas só se alternam se o kernel os interromper.
const PASSO: u64 = 20_000_000;

/// Quantas linhas o programa escreve. São poucas: a tela tem 25 linhas e os
/// testes leem a tela, então a saída dos dois programas tem de caber nela.
const LINHAS: u64 = 8;

fn main() -> i32 {
    let mut contador: u64 = 0;
    for linha in 1..=LINHAS {
        for _ in 0..PASSO {
            // `black_box` esconde o valor do compilador: sem ele, o laço seria
            // eliminado e o programa terminaria na hora.
            contador = black_box(contador).wrapping_add(1);
        }
        // Pede ao kernel (syscall `write`) que escreva a linha na tela.
        println!("A: {}", linha);
    }
    0
}
```

### Programa de exemplo 6: contador_b

O irmão do `contador_a`, escrevendo `B:` em vez de `A:`. Arquivo: `programs/src/bin/contador_b.rs`.

```rust
//! `contador_b`: conta em laço longo, **sem nunca ceder a CPU**.
//!
//! Este programa não pede a vez a ninguém: ele só conta, como se fosse o único
//! programa do sistema. Mesmo assim, rodando ao lado do `contador_a`
//! (`run contador_a contador_b`), as linhas dos dois aparecem intercaladas na
//! tela, porque o timer do kernel interrompe cada programa depois de uma
//! fatia de tempo e passa a CPU ao outro. Quem manda na CPU é o kernel, não o
//! programa.

#![no_std]
#![no_main]

use core::hint::black_box;
use runtime::{entry, println};

entry!(main);

/// Quantas iterações o programa conta entre uma linha e a seguinte. É grande
/// de propósito: leva várias fatias de tempo do timer, então as linhas dos
/// dois programas só se alternam se o kernel os interromper.
const PASSO: u64 = 20_000_000;

/// Quantas linhas o programa escreve. São poucas: a tela tem 25 linhas e os
/// testes leem a tela, então a saída dos dois programas tem de caber nela.
const LINHAS: u64 = 8;

fn main() -> i32 {
    let mut contador: u64 = 0;
    for linha in 1..=LINHAS {
        for _ in 0..PASSO {
            // `black_box` esconde o valor do compilador: sem ele, o laço seria
            // eliminado e o programa terminaria na hora.
            contador = black_box(contador).wrapping_add(1);
        }
        // Pede ao kernel (syscall `write`) que escreva a linha na tela.
        println!("B: {}", linha);
    }
    0
}
```

Rode os dois juntos:

```text
os-rust> run contador_a contador_b
B: 1
A: 1
B: 2
A: 2
...
```

As linhas dos dois aparecem intercaladas, embora nenhum deles peça a vez: quem manda na CPU é o kernel, que interrompe cada programa depois de uma fatia de tempo. A ordem exata das linhas pode variar de uma execução para outra.

### Programa de exemplo 7: eco2

O irmão do `eco`, com o prefixo `eco2:` na resposta, para rodar ao lado dele. Arquivo: `programs/src/bin/eco2.rs`.

```rust
//! `eco2`: o irmão do `eco`, para rodar ao lado dele.
//!
//! Faz o mesmo que o `eco` (lê uma linha do teclado e responde), mas põe o
//! prefixo `eco2:` na resposta. Rode `run eco eco2`, digite uma linha e Enter, e
//! depois outra: o teclado vai ao programa que **pediu primeiro** (o `eco`), e a
//! segunda linha vai ao outro. Cada resposta mostra quem recebeu qual linha.
//! Enquanto um programa espera o teclado ele não gasta CPU: o kernel passa a
//! vez ao outro.

#![no_std]
#![no_main]

// O alocador global vem da biblioteca de runtime; `extern crate alloc`
// dá acesso a `Vec`, `Box` e `String`.
extern crate alloc;

use alloc::vec::Vec;
use runtime::{entry, print, println, read_line};

entry!(main);

fn main() -> i32 {
    print!("eco2: digite algo: ");

    // O buffer é do programa. `read_line` escreve nele a linha digitada e
    // devolve o texto, sem o Enter. Cabem até 127 caracteres.
    let mut buffer = [0u8; 128];
    let texto = read_line(&mut buffer);

    // A resposta: exatamente o que foi digitado, com o prefixo do programa.
    println!("eco2: voce digitou: {}", texto);

    // `collect` cria um `Vec`: a primeira alocação faz a biblioteca pedir
    // memória ao kernel (syscall `SYS_ALLOC`).
    let palavras: Vec<&str> = texto.split_whitespace().collect();
    println!("eco2: palavras: {}", palavras.len());

    0
}
```

Rode os dois juntos, digite uma linha e Enter, e depois outra:

```text
os-rust> run eco eco2
digite algo: eco2: digite algo: um dois
voce digitou: um dois
palavras: 2
tres
eco2: voce digitou: tres
eco2: palavras: 1
os-rust>
```

A primeira linha vai ao `eco`, que pediu primeiro; a segunda vai ao `eco2`.

### Escrevendo um programa que coopera

1. Crie `programs/src/bin/meuprograma.rs` como na seção anterior e importe `yield_now` junto com o resto: `use runtime::{entry, println, yield_now};`.
2. Escreva um laço que faz um pouco de trabalho, escreve uma linha com `println!` e chama `yield_now()`.
3. Rode `cargo run` e, no prompt, `run ping meuprograma`: as linhas do seu programa se alternam com as do `ping`.

## Arquivos

O os-rust lê arquivos, **somente leitura**: nenhum programa cria, altera ou apaga arquivos. Os arquivos moram em dois volumes, ambos no formato FAT16, e o seu programa os enxerga pelo mesmo caminho, sem saber de onde vêm os bytes:

| Volume | O que é |
|---|---|
| `/ram` | o ramdisk, embutido na imagem de boot |
| `/disco` | o disco, anexado ao QEMU pelo `cargo run` (se o QEMU subir sem ele, `/disco` fica indisponível e o `/ram` continua funcionando) |

O conteúdo dos dois vem do diretório `discos/` do repositório (`discos/ram/` e `discos/disco/`), transformado em imagem de volume dentro de `cargo run`/`cargo test`: para dar um arquivo novo ao seu programa, ponha-o ali.

### Caminhos

Um caminho é `/<volume>/<componente>/<componente>...`, por exemplo `/disco/docs/longo.txt`.

- Cada componente é um nome **8.3**: até 8 caracteres, e opcionalmente um ponto e até 3 de extensão (letras, dígitos, `_`, `-`, `~`, `$`). Nomes longos não existem.
- Maiúsculas e minúsculas não fazem diferença: `/DISCO/DOCS/LONGO.TXT` é o mesmo arquivo. Os nomes que o kernel devolve vêm em minúsculas.
- `.` e `..` não são aceitos: nenhum caminho sai do volume.
- No máximo 64 bytes e 8 componentes por caminho, e no máximo 4 arquivos abertos ao mesmo tempo por programa.

### A API

```rust
use runtime::{Dir, DirEntry, File, FsError};

let mut arquivo = File::open("/disco/docs/longo.txt")?;   // Result<File, FsError>
let n = arquivo.read(&mut buffer)?;                       // Result<usize, FsError>; 0 = acabou

let mut diretorio = Dir::open("/disco")?;                 // Result<Dir, FsError>
while let Some(entrada) = diretorio.next()? {             // Result<Option<DirEntry>, FsError>
    entrada.name();    // &str, ex.: "leiame.txt"
    entrada.is_dir();  // bool
    entrada.size();    // u32, em bytes (0 para diretório)
}
```

- `File::read` lê **a partir de onde a leitura anterior parou** e avança. Ela devolve quantos bytes leu; só devolve `0` quando o arquivo acabou. Um arquivo vazio devolve `0` na primeira leitura.
- `Dir::next` entrega uma entrada por vez, na ordem do diretório, sem `.` nem `..`.
- `File` e `Dir` **fecham sozinhos** quando saem de escopo. Se o seu programa terminar (por `exit`, ou por um erro) com arquivos abertos, o kernel os fecha.
- Cada programa tem a **sua** lista de arquivos abertos: a posição de leitura de um programa nunca é a de outro, nem quando dois programas leem o mesmo arquivo ao mesmo tempo.

### Os erros

Toda operação devolve `Result<_, FsError>`. `FsError` implementa `Display`, então `println!("{}", erro)` escreve uma frase.

| `FsError` | Quando |
|---|---|
| `NotFound` | o caminho (ou um componente dele) não existe |
| `NoVolume` | o volume não existe ou não está disponível (por exemplo, QEMU sem disco, ou volume com defeito) |
| `WrongType` | ler um diretório como arquivo, listar um arquivo, ou um arquivo no meio do caminho |
| `TooManyOpen` | o programa já tem 4 arquivos abertos |
| `PathTooLong` | mais de 64 bytes ou 8 componentes |
| `Invalid` | caminho malformado: componente vazio, nome fora de 8.3, `.` ou `..`, ou sem `/` no começo |
| `Io` | erro ao ler o volume: o disco não respondeu ou a estrutura do volume está corrompida |

Um erro de arquivo **nunca** derruba o seu programa nem o kernel: você recebe o `Err` e decide o que fazer.

### Programa de exemplo 8: leitor

Abre um arquivo do disco e escreve o conteúdo na tela. O arquivo ocupa mais de um cluster do disco, e o programa não sabe nem precisa saber disso. Arquivo: `programs/src/bin/leitor.rs`. Rode com `run leitor`.

```rust
//! `leitor`: abre um arquivo do disco, lê o conteúdo e o escreve na tela.
//!
//! O programa nunca fala com o disco: ele pede ao kernel, por syscalls, e a
//! biblioteca de runtime esconde as syscalls atrás de `File`. O arquivo é
//! lido em pedaços de 128 bytes (um buffer na pilha) até `read` devolver `0`,
//! que quer dizer "o arquivo acabou". O caminho é fixo porque programas não
//! recebem argumentos de linha de comando neste marco.
//!
//! Rode com `run leitor`. Para ler outro arquivo, troque `CAMINHO`.

#![no_std]
#![no_main]

use runtime::sys::write;
use runtime::{entry, println, File};

entry!(main);

/// O arquivo que o programa lê: um texto de dois clusters, no disco.
const CAMINHO: &str = "/disco/docs/longo.txt";

fn main() -> i32 {
    // `File::open` devolve um erro de Rust se o arquivo não existe, se o disco
    // não está lá, etc. O arquivo é fechado sozinho quando `arquivo` sai de escopo.
    let mut arquivo = match File::open(CAMINHO) {
        Ok(arquivo) => arquivo,
        Err(erro) => {
            println!("leitor: nao abriu {}: {}", CAMINHO, erro);
            return 1;
        }
    };

    let mut pedaco = [0u8; 128];
    loop {
        match arquivo.read(&mut pedaco) {
            // 0 bytes: o arquivo acabou.
            Ok(0) => return 0,
            // `write` entrega os bytes à tela exatamente como estão.
            Ok(n) => {
                write(&pedaco[..n]);
            }
            Err(erro) => {
                println!("leitor: erro de leitura: {}", erro);
                return 2;
            }
        }
    }
}
```

Repare que `Err(erro)` aparece em dois lugares: ao abrir (o arquivo pode não existir, ou o disco pode não estar lá) e ao ler. O programa avisa e sai com um código diferente de zero, e o sistema escreve `[run] leitor terminou com codigo <n>`.

### Programa de exemplo 9: listador

Lista um diretório do disco. Arquivo: `programs/src/bin/listador.rs`. Rode com `run listador`.

```rust
//! `listador`: lista um diretório do disco e escreve o nome de cada entrada.
//!
//! Um diretório é aberto como um arquivo, com `Dir::open`, e lido uma entrada
//! por vez com `next`, até ela devolver `None`. Cada entrada diz o nome, se é
//! um diretório e o tamanho. O caminho é fixo porque programas não recebem
//! argumentos de linha de comando neste marco.
//!
//! Rode com `run listador`. Para listar outro diretório, troque `CAMINHO`.

#![no_std]
#![no_main]

use runtime::{entry, println, Dir};

entry!(main);

/// O diretório que o programa lista: a raiz do disco.
const CAMINHO: &str = "/disco";

fn main() -> i32 {
    let mut diretorio = match Dir::open(CAMINHO) {
        Ok(diretorio) => diretorio,
        Err(erro) => {
            println!("listador: nao abriu {}: {}", CAMINHO, erro);
            return 1;
        }
    };

    loop {
        match diretorio.next() {
            Ok(Some(entrada)) => {
                let tipo = if entrada.is_dir() { "dir" } else { "arquivo" };
                println!("{} {} {}", tipo, entrada.size(), entrada.name());
            }
            // Sem mais entradas.
            Ok(None) => return 0,
            Err(erro) => {
                println!("listador: erro de leitura: {}", erro);
                return 2;
            }
        }
    }
}
```

### Rodando um programa que está no disco

`run` também aceita o caminho de um arquivo executável: um argumento que começa com `/` é um caminho, e qualquer outro é o nome de um programa embutido. Experimente `run /disco/bin/visita`: `visita` existe **só** no disco (`programs/src/disco/visita.rs`), o kernel nunca o viu em tempo de compilação, e mesmo assim ele roda. O arquivo precisa ser um ELF64 estático válido e ter no máximo 64 KiB; os dois tipos de alvo se misturam (`run /disco/bin/visita hello`).

### Escrevendo um programa que lê arquivos

1. Ponha o arquivo que você quer ler em `discos/disco/` (por exemplo, `discos/disco/notas.txt`; lembre do nome 8.3).
2. Crie `programs/src/bin/meuleitor.rs` copiando o `leitor` e troque a constante `CAMINHO` por `"/disco/notas.txt"`.
3. Rode `cargo run` e, no prompt, `run meuleitor`.
4. Para só conferir o arquivo sem escrever um programa, o prompt tem `ls <caminho>` e `cat <caminho>` (por exemplo, `ls /disco`, `cat /disco/notas.txt`).

## Escrevendo e compilando o seu próprio programa

1. Crie um arquivo novo em `programs/src/bin/`, com o nome do seu programa (por exemplo, `programs/src/bin/meuprograma.rs`).
2. Comece pela estrutura mínima da seção acima: `#![no_std]`, `#![no_main]`, `entry!(main);` e `fn main() -> i32`. Não crie `Cargo.toml`: os programas compartilham o de `programs/`, que já depende da biblioteca de runtime.
3. Escreva a lógica do seu programa usando `print!`/`println!`, `read_line`, e `Box`/`Vec` se precisar (com `extern crate alloc;`).
4. Rode `cargo run` na raiz do repositório. Seu programa é compilado automaticamente junto com os demais, e embutido na imagem de boot.
5. No prompt do os-rust, digite `run meuprograma`. Se o nome não bater com nenhum programa embutido, o prompt lista os nomes disponíveis, então confira o nome do seu arquivo.

Não existe nenhum passo de compilação manual separado: se `cargo run` funciona, seu programa está embutido.

## Testando

A forma mais simples de testar é manual: `cargo run`, depois `run <nome do seu programa>`, observando a tela.

Para verificação automatizada, `cargo test` roda a suíte do kernel, que inclui testes cobrindo o carregamento e a execução de programas de usuário (usando `hello`, `eco` e `falha_memoria` como referência), de vários programas ao mesmo tempo (`tests/multitarefa.rs`: `ping`, `pong`, `contador_a`, `contador_b`, `eco2`) e de programas que leem arquivos (`tests/sistema_de_arquivos.rs`: `leitor`, `listador`, `visita`). Se você quiser um teste dedicado ao seu próprio programa, o padrão usado pelo projeto é um arquivo em `tests/`, dando boot no kernel, "digitando" a entrada com `interrupts::push_scancode`, e verificando o resultado na tela; `tests/user_runtime.rs` é o modelo.

## Erros comuns

- **Meu programa não aparece em `run`**: confira se o nome do arquivo em `programs/src/bin/` (sem `.rs`) é exatamente o que você digitou depois de `run`, e se `cargo run` terminou sem erro de compilação.
- **`cargo run` falha ao compilar meu programa**: o erro apontado é do seu código, não do kernel; corrija e rode `cargo run` de novo. O kernel não sobe com um programa que falhou ao compilar.
- **Erro de ligação dizendo que falta `_start`**: faltou `entry!(main);` no seu programa.
- **Meu programa e outro estão intercalando a saída e eu queria uma ordem fixa**: a ordem entre programas só é a do rodízio de `yield_now` enquanto o timer não interrompe ninguém; o kernel pode interromper qualquer programa em qualquer instrução. Se a ordem importa, cada programa precisa esperar pelo que precisa de outro jeito (não existe comunicação entre programas nesta versão).
- **Meu programa trava e nunca volta ao prompt**: confira se todo caminho de código do seu `main` eventualmente retorna (ou chama `exit`); um programa que nunca sai continua ocupando o prompt. Se ele está em `read_line`, está esperando você apertar Enter.
- **`memory allocation of N bytes failed` e `terminou com codigo 101`**: o programa pediu mais memória do que o heap de 1 MiB comporta (a mensagem vem depois de uma linha `[panic] panicked at ...`).
- **Meu programa usa uma syscall que não existe**: o kernel encerra o programa com uma mensagem de erro, do mesmo jeito que trata um acesso inválido à memória; confira o número da syscall no `SYSCALLS.md`.
- **`File::open` devolve `NoVolume` para `/disco`**: o QEMU subiu sem o disco, ou o volume está com defeito. `cargo run` já anexa o disco sozinho; se você roda o QEMU à mão, anexe `target/imagens/disco.img` como disco IDE (`-drive file=target/imagens/disco.img,format=raw,if=ide,index=1`).
- **`File::open` devolve `NotFound` para um arquivo que está em `discos/`**: o nome precisa caber em 8.3 (até 8 caracteres, ponto, até 3 de extensão), e `cargo run` precisa ter sido refeito depois de você adicionar o arquivo.
- **`run /disco/...` diz `arquivo grande demais`**: executáveis lidos de arquivo têm no máximo 64 KiB.
