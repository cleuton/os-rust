[ENGLISH VERSION](PROGRAMMERS_GUIDE.md)

# Guia do Programador - os-rust

Este guia é para quem quer escrever um programa que roda dentro do os-rust, sem precisar entender o kernel por dentro. Se você quer saber como o boot, a memória ou as interrupções funcionam, o documento certo é o `WALKTHROUGH.md`. Este aqui assume que você só quer escrever código que rode em modo usuário, usando a biblioteca de runtime do projeto.

## Pré-requisitos

Os mesmos três do `README.md`: Rust com a toolchain nightly pinada pelo projeto (`rust-toolchain.toml`), `bootimage` e QEMU. Se você já roda `cargo run` com sucesso na raiz do repositório, está pronto para seguir este guia. Para criar programas **fora** do repositório e levá-los ao os-rust por FTP, veja a seção "Programas fora do repositório".

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
- `File`, `Dir`, `DirEntry` e `FsError`: abrir e ler arquivos e listar diretórios (veja a seção **Arquivos**). `File` e `Dir` só leem.
- `FileWriter`, `remove`, `mkdir` e `rmdir`: criar um arquivo, acrescentar bytes ao fim dele, apagá-lo, e criar e remover diretórios, nos volumes `/ram` e `/disco` (veja a seção **Escrevendo arquivos**).
- `time::now()`: a data e a hora atuais, em UTC (veja a seção **A hora**).
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
| 6 | `SYS_OPEN` | Abre um arquivo ou diretório para leitura | `File::open`, `Dir::open` |
| 7 | `SYS_READ` | Lê bytes de um arquivo aberto | `File::read` |
| 8 | `SYS_CLOSE` | Fecha um arquivo ou diretório | automático, quando o `File`/`Dir` sai de escopo |
| 9 | `SYS_READ_DIR` | Lê a próxima entrada de um diretório aberto | `Dir::next` |
| 11 | `SYS_CREATE` | Cria um arquivo vazio e o abre para acrescentar | `FileWriter::create` |
| 12 | `SYS_OPEN_APPEND` | Abre um arquivo existente para acrescentar | `FileWriter::append` |
| 13 | `SYS_WRITE_FILE` | Grava bytes no fim do arquivo | `FileWriter::write`, `FileWriter::write_all` |
| 14 | `SYS_REMOVE` | Apaga um arquivo | `remove` |

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

O os-rust tem arquivos em dois volumes (os dois aceitam escrita, veja **Escrevendo arquivos**). Esta seção mostra como **ler**. Os arquivos moram em dois volumes, ambos no formato FAT16, e o seu programa os enxerga pelo mesmo caminho, sem saber de onde vêm os bytes:

| Volume | O que é |
|---|---|
| `/ram` | o ramdisk, copiado da imagem embutida na imagem de boot (leitura e escrita) |
| `/disco` | o disco, anexado ao QEMU pelo `cargo run`, leitura e escrita (se o QEMU subir sem ele, `/disco` fica indisponível e o `/ram` continua funcionando) |

O conteúdo dos dois vem do diretório `discos/` do repositório (`discos/ram/` e `discos/disco/`), transformado em imagem de volume pelo `build.rs` (sempre que ele roda): para dar um arquivo novo ao seu programa, ponha-o ali.

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
| `Exists` | `FileWriter::create` num nome que já existe |
| `NoSpace` | o volume (ou o diretório) não tem mais espaço |
| `ReadOnly` | o volume não aceita escrita (só acontece com um volume de mais de 1 setor por cluster) |
| `Busy` | `remove` de um arquivo, ou `rmdir` de um diretório, que algum programa tem aberto; `rmdir` da raiz de um volume |
| `NotEmpty` | `rmdir` de um diretório que ainda tem entradas |

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

## Escrevendo arquivos

Os volumes `/ram` e `/disco` aceitam escrita. A escrita é simples de propósito: **um arquivo nasce vazio e só cresce pelo fim; apagar devolve o espaço; nada fica guardado para depois** (cada `write` já deixa os dados no volume, não existe `flush`). Quem quiser trocar o conteúdo de um arquivo apaga e cria de novo.

```rust
use runtime::{mkdir, remove, rmdir, FileWriter, FsError};

let mut arquivo = FileWriter::create("/ram/nota.txt")?;  // Result<FileWriter, FsError>; Exists se o nome já existe
arquivo.write_all(b"ola, ramdisk\n")?;                   // grava tudo, repetindo write até acabar
let n = arquivo.write(b"mais")?;                         // Result<usize, FsError>; pode gravar menos que o pedido

let mut mesmo = FileWriter::append("/ram/nota.txt")?;    // abre um arquivo que já existe, para acrescentar
remove("/ram/nota.txt")?;                                // Result<(), FsError>
mkdir("/ram/docs/novo")?;                               // cria um diretório vazio; Exists se o nome já existe
rmdir("/ram/docs/novo")?;                               // remove um diretório vazio; NotEmpty se tem entradas
```

As regras:

- **Só pelo fim.** Não há posicionar, sobrescrever no meio nem truncar. `FileWriter::create` falha com `Exists` se o nome já existe: nada é sobrescrito por acidente.
- **Arquivo aberto não se apaga.** `remove` de um arquivo que *algum* programa tem aberto (inclusive o seu) devolve `Busy`. Feche antes: solte o `File` ou o `FileWriter` de escopo (`drop(arquivo)`).
- **Volume cheio.** `write` devolve quantos bytes coube; a chamada seguinte devolve `NoSpace`. O que coube fica gravado.
- **Quem lê vê o tamanho de quando abriu.** Um `File` aberto antes de um `write` não enxerga o que foi acrescentado depois; uma nova abertura enxerga tudo.
- **Diretórios.** `mkdir` cria um diretório vazio (o pai precisa existir; não é recursivo) e `rmdir` remove um diretório **vazio** que ninguém tem aberto. Um diretório novo ocupa um cluster e não cresce: cabem 14 entradas (sem posição livre, `NoSpace`).
- **O que persiste.** Reiniciar o sistema devolve o `/ram` ao conteúdo original. O `/disco` guarda o que você grava **na imagem em que o QEMU está rodando**: com o `cargo run` (`snapshot=on`) isso é descartado quando o QEMU fecha. Para a gravação sobreviver a um reinício, rode o QEMU à mão sobre uma cópia da imagem do disco (README, "Preservando o disco entre boots"). Nada é protegido: qualquer arquivo, ou diretório vazio, pode ser apagado.
- O prompt tem os comandos `grava <caminho> [<texto>]`, `anexa <caminho> <texto>`, `apaga <caminho>`, `mkdir <caminho>` e `rmdir <caminho>` para fazer o mesmo à mão.

### Programa de exemplo 10: gravador

Cria `/ram/gravado.txt` com 20 linhas (680 bytes); se o arquivo já existe, acrescenta uma linha. Arquivo: `programs/src/bin/gravador.rs`. Rode com `run gravador` (cria), de novo (amplia), e confira com `cat /ram/gravado.txt`.

```rust
//! `gravador`: cria um arquivo no ramdisk e, na vez seguinte, o amplia.
//!
//! Um programa nunca fala com o disco: ele pede ao kernel, por syscalls, e a
//! biblioteca de runtime esconde as syscalls atrás de `FileWriter`, que fecha o
//! descritor sozinho quando sai de escopo. Escrever é sempre **pelo fim**: um
//! arquivo nasce vazio e só cresce acrescentando bytes. Por isso o programa
//! tenta criar primeiro; se o nome já existe (`Exists`), abre o arquivo para
//! acrescentar em vez de sobrescrevê-lo.
//!
//! O `/ram` volta ao conteúdo original a cada boot (o `/disco` também aceita
//! escrita, mas só persiste se o QEMU roda sem `snapshot=on`). O caminho é fixo
//! porque programas não recebem argumentos de linha de comando.
//!
//! Rode com `run gravador` (cria), de novo (amplia) e `run apagador` (remove).

#![no_std]
#![no_main]

use runtime::{entry, println, FileWriter, FsError};

entry!(main);

/// O arquivo que o programa cria e amplia.
const CAMINHO: &str = "/ram/gravado.txt";

fn main() -> i32 {
    match FileWriter::create(CAMINHO) {
        Ok(mut arquivo) => {
            if let Err(erro) = grava_linhas(&mut arquivo) {
                println!("gravador: nao gravou {}: {}", CAMINHO, erro);
                return 1;
            }
            println!("gravador: criei {} (680 bytes)", CAMINHO);
            0
        }
        // O nome já existe: acrescenta ao fim, sem tocar no que já está lá.
        Err(FsError::Exists) => {
            let resultado = FileWriter::append(CAMINHO)
                .and_then(|mut arquivo| arquivo.write_all(b"mais uma linha do gravador\n"));
            match resultado {
                Ok(()) => {
                    println!("gravador: ampliei {} (+27 bytes)", CAMINHO);
                    0
                }
                Err(erro) => {
                    println!("gravador: nao gravou {}: {}", CAMINHO, erro);
                    1
                }
            }
        }
        Err(erro) => {
            println!("gravador: nao gravou {}: {}", CAMINHO, erro);
            1
        }
    }
}

/// Grava as 20 linhas `linha NN: gravada por um programa`, cada uma com 34
/// bytes (33 de texto e a quebra de linha), 680 no total. A linha é montada
/// num buffer da pilha: o programa não precisa de heap.
fn grava_linhas(arquivo: &mut FileWriter) -> Result<(), FsError> {
    for numero in 1..=20u8 {
        let mut linha = *b"linha NN: gravada por um programa\n";
        linha[6] = b'0' + numero / 10;
        linha[7] = b'0' + numero % 10;
        arquivo.write_all(&linha)?;
    }
    Ok(())
}
```

### Programa de exemplo 11: apagador

Escreve na tela o arquivo que o `gravador` criou, fecha-o e o apaga. Arquivo: `programs/src/bin/apagador.rs`. Rode com `run apagador`.

```rust
//! `apagador`: mostra o arquivo que o `gravador` criou e o apaga.
//!
//! Primeiro lê o conteúdo (em pedaços de 128 bytes, como o `leitor`) e o
//! escreve na tela, o que também prova que um programa lê o que outro gravou.
//! Depois remove o arquivo com `remove`.
//!
//! Se o arquivo não existe, o programa avisa e termina com código 1.
//!
//! Rode com `run gravador` antes, e depois `run apagador`.

#![no_std]
#![no_main]

use runtime::sys::write;
use runtime::{entry, println, remove, File, FsError};

entry!(main);

/// O arquivo que o `gravador` cria.
const CAMINHO: &str = "/ram/gravado.txt";

fn main() -> i32 {
    let mut arquivo = match File::open(CAMINHO) {
        Ok(arquivo) => arquivo,
        Err(FsError::NotFound) => {
            println!("apagador: nao ha {} (rode gravador antes)", CAMINHO);
            return 1;
        }
        Err(erro) => {
            println!("apagador: nao abriu {}: {}", CAMINHO, erro);
            return 2;
        }
    };

    let mut pedaco = [0u8; 128];
    loop {
        match arquivo.read(&mut pedaco) {
            // 0 bytes: o arquivo acabou.
            Ok(0) => break,
            Ok(n) => {
                write(&pedaco[..n]);
            }
            Err(erro) => {
                println!("apagador: erro de leitura: {}", erro);
                return 2;
            }
        }
    }

    // O kernel não apaga um arquivo aberto (um leitor poderia passar a ler
    // clusters reaproveitados): `remove` daria `Busy`. Por isso o arquivo é
    // fechado antes, soltando-o de escopo.
    drop(arquivo);

    match remove(CAMINHO) {
        Ok(()) => {
            println!("apagador: removi {}", CAMINHO);
            0
        }
        Err(erro) => {
            println!("apagador: nao removeu {}: {}", CAMINHO, erro);
            3
        }
    }
}
```

Repare que o `apagador` fecha o arquivo (`drop(arquivo)`) **antes** de chamar `remove`: com o arquivo aberto, `remove` devolveria `Busy`.

### Programa de exemplo 12: caderno

Acrescenta uma linha com a hora (UTC) a `/disco/caderno.txt` a cada execução (20 bytes por linha), e depois escreve o arquivo na tela e conta as linhas. Arquivo: `programs/src/bin/caderno.rs`. Rode com `run caderno` duas vezes e confira com `ls /disco` (20 e depois 40 bytes). Ele escreve no **`/disco`**, então só deixa a anotação para o próximo boot se o QEMU rodar à mão sobre uma cópia da imagem (README, "Preservando o disco entre boots"); com o `cargo run`, a anotação vale até fechar o QEMU.

```rust
//! `caderno`: acrescenta uma linha com a hora a um arquivo do disco e mostra o
//! arquivo inteiro.
//!
//! Cada vez que roda, o programa anota a hora atual (UTC) em
//! `/disco/caderno.txt`: na primeira vez cria o arquivo, nas seguintes
//! acrescenta ao fim. Depois abre o arquivo, escreve o conteúdo na tela e
//! conta as linhas. É o jeito mais curto de ver a persistência do `/disco`:
//! rode `run caderno` duas vezes e o arquivo terá duas linhas (20 bytes cada).
//!
//! Atenção: enquanto o QEMU roda com `snapshot=on` (é o caso de `cargo run` e
//! `cargo test`), o `/disco` volta ao inicial quando o QEMU fecha. Para a
//! anotação sobreviver a um reinício, rode o QEMU à mão sobre uma cópia da
//! imagem do disco (passo a passo no README, "Preservando o disco entre boots").
//!
//! O escritor é fechado **antes** de o arquivo ser aberto para leitura, e o
//! caminho é fixo porque programas não recebem argumentos de linha de comando.
//!
//! Rode com `run caderno`.

#![no_std]
#![no_main]

use core::fmt::{self, Write};

use runtime::sys::write;
use runtime::{entry, println, time, File, FileWriter, FsError, TimeError};

entry!(main);

/// O arquivo que o programa cria e amplia.
const CAMINHO: &str = "/disco/caderno.txt";

/// Uma linha do caderno: `AAAA-MM-DD HH:MM:SS` (19 caracteres) mais `\n`.
const TAMANHO_DA_LINHA: usize = 20;

/// Por que o programa não conseguiu anotar ou mostrar o caderno.
enum Falha {
    Arquivo(FsError),
    Hora(TimeError),
    /// A linha não coube nos 20 bytes (não deveria acontecer).
    Formato,
}

impl From<FsError> for Falha {
    fn from(erro: FsError) -> Falha {
        Falha::Arquivo(erro)
    }
}

impl From<TimeError> for Falha {
    fn from(erro: TimeError) -> Falha {
        Falha::Hora(erro)
    }
}

impl fmt::Display for Falha {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Falha::Arquivo(erro) => write!(f, "{}", erro),
            Falha::Hora(erro) => write!(f, "{}", erro),
            Falha::Formato => f.write_str("linha com formato inesperado"),
        }
    }
}

/// Um buffer na pilha que recebe a linha formatada: o programa não precisa de heap.
struct Linha {
    bytes: [u8; TAMANHO_DA_LINHA],
    usados: usize,
}

impl Write for Linha {
    fn write_str(&mut self, texto: &str) -> fmt::Result {
        let fim = self.usados + texto.len();
        if fim > TAMANHO_DA_LINHA {
            return Err(fmt::Error);
        }
        self.bytes[self.usados..fim].copy_from_slice(texto.as_bytes());
        self.usados = fim;
        Ok(())
    }
}

fn main() -> i32 {
    match anota().and_then(|_| mostra()) {
        Ok(linhas) => {
            // "1 linha", mas "2 linhas".
            let palavra = if linhas == 1 { "linha" } else { "linhas" };
            println!("caderno: {} {} em {}", linhas, palavra, CAMINHO);
            0
        }
        Err(erro) => {
            println!("caderno: nao gravou {}: {}", CAMINHO, erro);
            1
        }
    }
}

/// Acrescenta a linha com a hora ao caderno, criando o arquivo se ele não existe.
fn anota() -> Result<(), Falha> {
    // O nome já existe (`Exists`): acrescenta ao fim, sem tocar no que já está lá.
    let mut escritor = match FileWriter::create(CAMINHO) {
        Ok(escritor) => escritor,
        Err(FsError::Exists) => FileWriter::append(CAMINHO)?,
        Err(erro) => return Err(erro.into()),
    };
    let agora = time::now()?;
    let mut linha = Linha { bytes: [0; TAMANHO_DA_LINHA], usados: 0 };
    // `{}` escreve `AAAA-MM-DD HH:MM:SS`; o relógio é sempre UTC.
    writeln!(linha, "{}", agora).map_err(|_| Falha::Formato)?;
    escritor.write_all(&linha.bytes[..linha.usados])?;
    // `escritor` sai de escopo aqui e o descritor é fechado, antes de o
    // arquivo ser aberto para leitura.
    Ok(())
}

/// Escreve o caderno na tela, em pedaços de 128 bytes, e devolve quantas
/// linhas ele tem.
fn mostra() -> Result<usize, Falha> {
    let mut arquivo = File::open(CAMINHO)?;
    let mut pedaco = [0u8; 128];
    let mut linhas = 0;
    loop {
        match arquivo.read(&mut pedaco)? {
            // 0 bytes: o arquivo acabou.
            0 => return Ok(linhas),
            n => {
                // `write` entrega os bytes à tela exatamente como estão.
                write(&pedaco[..n]);
                linhas += pedaco[..n].iter().filter(|&&b| b == b'\n').count();
            }
        }
    }
}
```

Repare que o escritor é fechado (sai de escopo em `anota`) **antes** de o arquivo ser aberto para leitura em `mostra`.

## A hora

Um programa pode perguntar a data e a hora ao kernel, que as lê do relógio do computador (o chip RTC). Você não toca no relógio: usa `time::now()`, da biblioteca de runtime, que faz a syscall `SYS_TIME` por você.

- `time::now()` devolve `Result<DateTime, TimeError>`.
- Um `DateTime` tem os campos `year`, `month`, `day`, `hour`, `minute` e `second`. Escrito com `{}`, ele sai no formato `AAAA-MM-DD HH:MM:SS`, por exemplo `2026-10-08 15:04:05`.
- A hora é **sempre UTC**. O os-rust não conhece fuso horário nem horário de verão: se você quer a hora local, some a diferença você mesmo.
- `TimeError::Clock` quer dizer que o relógio devolveu valores impossíveis ou não respondeu. É raro, mas um programa cuidadoso trata o erro em vez de supor que a hora sempre vem.

O programa `hora` faz só isso. Rode `run hora` e compare com o comando `data` do prompt: a diferença é de segundos. O código completo:

```rust
//! `hora`: escreve a data e a hora atuais (UTC).
//!
//! O programa não toca no relógio: pede a hora ao kernel (syscall `time`, pela
//! biblioteca de runtime) e escreve o que recebeu, no mesmo formato do comando
//! `data` do prompt. Rode `run hora` e compare com `data`.

#![no_std]
#![no_main]

use runtime::{entry, println, time};

entry!(main);

fn main() -> i32 {
    // Pede ao kernel (syscall `time`) a data e a hora. Só falha se o relógio
    // do computador devolver valores impossíveis.
    match time::now() {
        Ok(agora) => {
            // `{}` escreve `AAAA-MM-DD HH:MM:SS`; o relógio é sempre UTC.
            println!("{} UTC", agora);
            0
        }
        Err(erro) => {
            // Um código de saída diferente de zero faz o kernel avisar no
            // prompt (`[run] hora terminou com codigo 1`).
            println!("hora: {}", erro);
            1
        }
    }
}
```

Para ver a hora sem escrever um programa, o prompt tem o comando `data`. O contrato da syscall (registradores, formato do buffer de 8 bytes e erros) está em `SYSCALLS.md`, seção 5, `SYS_TIME`.

## Rede (TCP)

Um programa fala com a rede por **sockets TCP**. Você não chama as syscalls 17 a 23 na mão: usa os tipos da biblioteca de runtime.

- `Ipv4`: um endereço, quatro bytes. `Ipv4::parse("10.0.2.2")` devolve `Option<Ipv4>`; escrito com `{}`, sai como `a.b.c.d`. O endereço do próprio computador, visto de dentro do programa, é `127.0.0.1` (o loopback, que funciona até sem placa de rede); `10.0.2.2` é o computador de verdade quando o os-rust roda no QEMU.
- `TcpListener::bind(porta)` abre um servidor; `accept()` espera uma conexão e devolve um `TcpStream`; `set_timeout(ms)` muda o tempo limite.
- `TcpStream::connect(ip, porta)` abre uma conexão; `read(&mut buffer)` lê o que chegou e devolve `0` quando o outro lado fechou; `write(&bytes)` envia o que couber e `write_all(&bytes)` envia tudo; `set_timeout(ms)` muda o tempo limite.
- `NetError` diz o que deu errado (`TimedOut`, `Refused`, `Reset`, `AddrInUse`, `NoBuffers`, `NoNetwork`, ...) e se escreve com `{}`.

As regras:

- **Não existe espera infinita.** Toda espera (`accept`, `read`, `connect`, `write` com o buffer cheio) tem tempo limite: 10 segundos por padrão, de 0 a 60 000 ms com `set_timeout`; o `connect` espera no máximo 3 segundos. Um servidor esquecido não prende o computador, porque não existe Ctrl+C.
- **Feche o que abriu.** `TcpListener` e `TcpStream` fecham o socket sozinhos quando saem de escopo (`drop`). Um programa pode ter até 4 sockets abertos ao mesmo tempo, e o kernel inteiro tem 8 conexões; se o programa termina sem fechar, o kernel fecha por ele.
- **Só TCP.** Não há UDP, nomes de máquina (DNS) nem endereço automático (DHCP). Esperar a rede não gasta CPU: o programa fica bloqueado, e os outros programas continuam.

### Programa de exemplo 13: servidor

Espera uma conexão na porta 8000 e responde com uma página HTTP/1.0 mínima (88 bytes: 65 de cabeçalho e 23 de corpo). Arquivo: `programs/src/bin/servidor.rs`. Rode com `run servidor cliente`, ou sozinho com `run servidor` (e, no QEMU iniciado à mão com `hostfwd`, `curl http://127.0.0.1:8080/` no computador; veja o README, "Rede no QEMU").

```rust
//! `servidor`: espera uma conexão TCP na porta 8000 e responde a uma
//! requisição, no formato mínimo de uma página HTTP/1.0.
//!
//! É a metade "servidora" da demonstração de rede do Marco 12. Rode junto do
//! `cliente` (`run servidor cliente`), e o tráfego passa pelo loopback
//! (`127.0.0.1`), sem nenhuma placa de rede. Com o QEMU iniciado à mão com
//! `hostfwd` (README, "Rede no QEMU"), o próprio computador de verdade também
//! consegue conversar com ele: `curl http://127.0.0.1:8080/`.
//!
//! O programa não interpreta a requisição: lê o que chegar (uma vez, até 512
//! bytes) e devolve sempre a mesma página, com 65 bytes de cabeçalho e 23 de
//! corpo. Toda espera tem tempo limite, porque não existe Ctrl+C: se ninguém
//! se conectar em 30 segundos, o programa desiste e o prompt volta.

#![no_std]
#![no_main]

use runtime::{entry, println, NetError, TcpListener};

entry!(main);

/// A porta em que o servidor espera.
const PORTA: u16 = 8000;

/// Quanto o servidor espera alguém se conectar (30 s).
const ESPERA_CONEXAO_MS: u32 = 30_000;

/// Quanto espera a requisição, depois de conectado (10 s).
const ESPERA_DADOS_MS: u32 = 10_000;

/// O corpo da resposta: 23 bytes, com o `\n` final.
const CORPO: &str = "ola da rede do os-rust\n";

/// A resposta inteira: 65 bytes de cabeçalho (`\r\n` termina cada linha, e uma
/// linha em branco termina o cabeçalho) mais os 23 do corpo, 88 no total.
const RESPOSTA: &[u8] =
    b"HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 23\r\n\r\nola da rede do os-rust\n";

fn main() -> i32 {
    match servir() {
        Ok(()) => 0,
        Err(codigo) => codigo,
    }
}

/// Atende uma conexão. O erro é o código de saída do programa.
fn servir() -> Result<(), i32> {
    let ouvinte = TcpListener::bind(PORTA).map_err(erro_de_rede)?;
    ouvinte.set_timeout(ESPERA_CONEXAO_MS).map_err(erro_de_rede)?;
    println!("servidor: ouvindo na porta {}", PORTA);

    let conexao = match ouvinte.accept() {
        Ok(conexao) => conexao,
        Err(NetError::TimedOut) => {
            println!("servidor: ninguem se conectou em 30 s");
            return Err(1);
        }
        Err(erro) => return Err(erro_de_rede(erro)),
    };
    conexao.set_timeout(ESPERA_DADOS_MS).map_err(erro_de_rede)?;

    // Uma leitura basta: a requisição do `cliente` cabe num segmento só.
    let mut requisicao = [0u8; 512];
    let lidos = conexao.read(&mut requisicao).map_err(erro_de_rede)?;
    println!("servidor: recebi {} bytes", lidos);

    conexao.write_all(RESPOSTA).map_err(erro_de_rede)?;
    // Fechar a conexão e o ouvinte: o kernel termina de enviar o que falta e
    // encerra a conexão em segundo plano, mesmo depois de o programa acabar.
    drop(conexao);
    drop(ouvinte);
    println!("servidor: respondi {} bytes de corpo", CORPO.len());
    Ok(())
}

/// Mostra o erro e devolve o código de saída 2.
fn erro_de_rede(erro: NetError) -> i32 {
    println!("servidor: erro de rede: {}", erro);
    2
}
```

### Programa de exemplo 14: cliente

Conecta num servidor na porta 8000, pede uma página (33 bytes) e escreve a resposta na tela, sem os `\r`, até 1024 bytes. Arquivo: `programs/src/bin/cliente.rs`. Apertar só Enter no endereço usa `127.0.0.1`.

```rust
//! `cliente`: conecta num servidor na porta 8000, pede uma página e a mostra.
//!
//! É a metade "cliente" da demonstração de rede do Marco 12. Pergunta o
//! endereço do servidor; apertar só Enter usa `127.0.0.1`, e então o par é o
//! programa `servidor` (`run servidor cliente`), pelo loopback, sem placa de
//! rede. Digitando `10.0.2.2` (o computador de verdade, visto de dentro do
//! QEMU), conversa com um servidor do host, por exemplo
//! `python3 -m http.server 8000` (README, "Rede no QEMU").
//!
//! A requisição tem 33 bytes. A resposta é escrita na tela sem os `\r` (que só
//! servem ao protocolo), até 1024 bytes; se vier mais que isso, o programa
//! mostra o começo e avisa que cortou. Toda espera tem tempo limite (10 s).

#![no_std]
#![no_main]

use runtime::sys::write;
use runtime::{entry, print, println, read_line, Ipv4, NetError, TcpStream};

entry!(main);

/// A porta do servidor.
const PORTA: u16 = 8000;

/// O pedido: 33 bytes (`GET / HTTP/1.0\r\n` 16, `Host: os-rust\r\n` 15, `\r\n` 2).
const REQUISICAO: &[u8] = b"GET / HTTP/1.0\r\nHost: os-rust\r\n\r\n";

/// Quanto da resposta vai para a tela.
const LIMITE_NA_TELA: usize = 1024;

/// Quanto espera cada resposta do servidor (10 s).
const ESPERA_MS: u32 = 10_000;

fn main() -> i32 {
    print!("cliente: endereco do servidor (vazio = 127.0.0.1): ");
    let mut linha = [0u8; 64];
    let texto = read_line(&mut linha).trim();
    let ip = if texto.is_empty() {
        Ipv4([127, 0, 0, 1])
    } else {
        match Ipv4::parse(texto) {
            Some(ip) => ip,
            None => {
                println!("cliente: endereco invalido: {}", texto);
                return 2;
            }
        }
    };

    let conexao = match TcpStream::connect(ip, PORTA) {
        Ok(conexao) => conexao,
        Err(erro) => {
            println!("cliente: nao conectou em {}:{}: {}", ip, PORTA, erro);
            return 1;
        }
    };

    match conversar(&conexao) {
        Ok(()) => 0,
        Err(erro) => {
            println!("cliente: erro de rede: {}", erro);
            2
        }
    }
}

/// Envia o pedido e mostra a resposta até o servidor fechar a conexão.
fn conversar(conexao: &TcpStream) -> Result<(), NetError> {
    conexao.set_timeout(ESPERA_MS)?;
    conexao.write_all(REQUISICAO)?;

    let mut lidos_no_total = 0;
    let mut escritos = 0;
    let mut cortou = false;
    let mut pedaco = [0u8; 256];
    loop {
        let n = conexao.read(&mut pedaco)?;
        if n == 0 {
            break;
        }
        lidos_no_total += n;
        // Tira os `\r` e respeita o limite da tela.
        let mut limpo = [0u8; 256];
        let mut tamanho = 0;
        for &byte in &pedaco[..n] {
            if byte == b'\r' {
                continue;
            }
            if escritos + tamanho == LIMITE_NA_TELA {
                cortou = true;
                break;
            }
            limpo[tamanho] = byte;
            tamanho += 1;
        }
        write(&limpo[..tamanho]);
        escritos += tamanho;
        if cortou {
            break;
        }
    }

    if cortou {
        println!("cliente: {} bytes recebidos (cortado)", LIMITE_NA_TELA);
    } else {
        println!("cliente: {} bytes recebidos", lidos_no_total);
    }
    Ok(())
}
```

Repare que cada espera já tem o seu tempo limite (`set_timeout`), que o `servidor` fecha a conexão e o ouvinte (`drop`) antes de dizer que respondeu, e que o `cliente` lê até `read` devolver `0`, o sinal de que o servidor fechou.

### Programa de exemplo 15: ftpd

Um servidor FTP: troca arquivos entre o computador de verdade e os volumes `/ram` e `/disco`. Cada comando FTP (`LIST`, `RETR`, `STOR`, `DELE`, `MKD`...) vira uma chamada de arquivo, e os bytes viajam por TCP. Arquivo: `programs/src/bin/ftpd.rs`. Para rodar, suba o QEMU à mão com `hostfwd` (README, "Servidor FTP"), digite `run ftpd`, aperte Enter na pergunta do endereço e use `curl ftp://127.0.0.1:2121/` no computador. Termina com `SITE EXIT` ou depois de 30 s sem cliente.

```rust
//! `ftpd`: um servidor FTP que troca arquivos entre o computador de verdade e
//! os volumes `/ram` e `/disco` do os-rust.
//!
//! O programa não guarda arquivos: cada comando FTP vira uma chamada de
//! arquivo (`File`, `FileWriter`, `Dir`, `remove`, `mkdir`, `rmdir`) e os bytes
//! viajam por TCP (`TcpListener` e `TcpStream`). Como o FTP usa duas conexões,
//! uma de **controle** (os comandos e as respostas, porta 21) e uma de
//! **dados** (o conteúdo de uma transferência, porta 2100), o programa fica com
//! no máximo quatro sockets abertos: a escuta da 21, a conexão de controle, a
//! escuta da 2100 e a conexão de dados. É exatamente o limite de cada programa,
//! por isso atende **uma sessão por vez** e só fala o **modo passivo** (`PASV`
//! e `EPSV`): no modo ativo o os-rust teria de conectar de volta ao cliente.
//!
//! Para o computador alcançar o servidor, o QEMU é iniciado à mão com
//! `hostfwd` (README, "Servidor FTP"); então `curl ftp://127.0.0.1:2121/` lista
//! as pastas `ram` e `disco`. Rode com `run ftpd`, aperte Enter na pergunta do
//! endereço (ele vai no `PASV`) e use o cliente FTP do computador.
//!
//! Não existe espera infinita nem Ctrl+C, então o servidor tem tempos: 30 s
//! esperando um cliente, 30 s sem receber comando e 10 s por espera de dados.
//! Ele termina sozinho depois de 30 s sem cliente, ou quando o cliente manda
//! `SITE EXIT`, e o prompt volta.
//!
//! Sem senha: qualquer `USER` e `PASS` entram, e todo arquivo é binário (nenhuma
//! quebra de linha é convertida). `STOR` nunca sobrescreve: um nome que já
//! existe dá erro, e uma transferência que falha no meio apaga o arquivo
//! parcial. Para trocar um arquivo, apague-o antes (`DELE`).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use runtime::{
    entry, mkdir, print, println, read_line, remove, rmdir, Dir, File, FileWriter, FsError, Ipv4,
    NetError, TcpListener, TcpStream,
};

entry!(main);

/// A porta da conexão de controle.
const PORTA_CONTROLE: u16 = 21;

/// A porta da conexão de dados, a mesma que o `PASV` anuncia (8 * 256 + 52).
const PORTA_DADOS: u16 = 2100;

/// Quanto espera um cliente se conectar (30 s).
const ESPERA_CLIENTE_MS: u32 = 30_000;

/// Quanto espera o próximo comando de um cliente conectado (30 s).
const ESPERA_COMANDO_MS: u32 = 30_000;

/// Quanto espera a conexão de dados ser aberta e cada leitura ou escrita dela
/// (10 s).
const ESPERA_DADOS_MS: u32 = 10_000;

/// Tamanho do buffer de comandos: uma linha de 512 bytes ou mais sem fim de
/// linha é recusada.
const TAM_LINHA: usize = 512;

/// Tamanho de cada pedaço de uma transferência (um segmento TCP).
const TAM_PEDACO: usize = 1024;

/// A resposta de quem pede algo na raiz virtual `/`, que só lista `ram` e `disco`.
const RAIZ_SO_LISTA: &str = "550 a raiz so lista /ram e /disco";

/// A resposta de um caminho que não começa por `ram` nem por `disco`.
const NAO_ENCONTRADO: &str = "550 nao encontrado";

/// A resposta de um comando de dados sem `PASV` ou `EPSV` antes.
const SEM_PASV: &str = "425 use PASV ou EPSV antes";

/// A resposta de uma conexão de dados que caiu ou parou.
const INTERROMPIDA: &str = "426 transferencia interrompida";

/// O que fazer depois de um comando.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fluxo {
    /// Esperar o próximo comando.
    Continua,
    /// Terminar esta sessão e esperar outro cliente.
    Encerra,
    /// Terminar a sessão **e** o programa (`SITE EXIT`).
    Sai,
}

fn main() -> i32 {
    match servir() {
        Ok(()) => 0,
        Err(codigo) => codigo,
    }
}

/// Pergunta o endereço, abre a escuta e atende um cliente por vez. O erro é o
/// código de saída do programa.
fn servir() -> Result<(), i32> {
    print!("ftpd: endereco a anunciar no modo passivo (vazio = 127.0.0.1): ");
    let mut linha = [0u8; 64];
    let texto = read_line(&mut linha).trim();
    let anunciado = if texto.is_empty() {
        Ipv4([127, 0, 0, 1])
    } else {
        match Ipv4::parse(texto) {
            Some(ip) => ip,
            None => {
                println!("ftpd: endereco invalido: {}", texto);
                return Err(2);
            }
        }
    };

    let ouvinte = TcpListener::bind(PORTA_CONTROLE).map_err(erro_de_rede)?;
    ouvinte.set_timeout(ESPERA_CLIENTE_MS).map_err(erro_de_rede)?;
    println!(
        "ftpd: ouvindo na porta {} (dados na porta {}), anunciando {}",
        PORTA_CONTROLE, PORTA_DADOS, anunciado
    );

    loop {
        let controle = match ouvinte.accept() {
            Ok(controle) => controle,
            Err(NetError::TimedOut) => {
                println!("ftpd: ninguem se conectou em 30 s, encerrando");
                return Ok(());
            }
            Err(erro) => return Err(erro_de_rede(erro)),
        };
        println!("ftpd: cliente conectado");
        // A sessão é consumida aqui: ao terminar, a conexão de controle, o
        // ouvinte de dados e a conexão de dados saem de escopo e são fechados.
        let fluxo = Sessao::nova(controle, anunciado).executar();
        println!("ftpd: sessao encerrada");
        if fluxo == Fluxo::Sai {
            println!("ftpd: encerrado por SITE EXIT");
            return Ok(());
        }
    }
}

/// Mostra o erro e devolve o código de saída 2.
fn erro_de_rede(erro: NetError) -> i32 {
    println!("ftpd: erro de rede: {}", erro);
    2
}

/// Uma sessão: o que vale entre a conexão de um cliente e o fim dela.
struct Sessao {
    /// A conexão de controle: comandos de entrada, respostas de saída.
    controle: TcpStream,
    /// Bytes lidos do controle que ainda não formaram uma linha inteira.
    entrada: [u8; TAM_LINHA],
    tam: usize,
    /// O diretório atual, sempre normalizado (`/` ou `/ram/docs`...).
    cwd: String,
    usuario_visto: bool,
    logado: bool,
    /// O ouvinte da porta 2100, armado por `PASV` ou `EPSV` e gasto por um
    /// comando de dados. Nunca há dois: a escuta duplicada daria erro.
    dados: Option<TcpListener>,
    /// O endereço que o `PASV` anuncia.
    anunciado: Ipv4,
}

impl Sessao {
    fn nova(controle: TcpStream, anunciado: Ipv4) -> Sessao {
        Sessao {
            controle,
            entrada: [0; TAM_LINHA],
            tam: 0,
            cwd: String::from("/"),
            usuario_visto: false,
            logado: false,
            dados: None,
            anunciado,
        }
    }

    /// Cumprimenta o cliente e atende comandos até a sessão acabar.
    fn executar(mut self) -> Fluxo {
        if self.controle.set_timeout(ESPERA_COMANDO_MS).is_err() {
            return Fluxo::Encerra;
        }
        if self.responder("220 os-rust ftpd pronto").is_err() {
            return Fluxo::Encerra;
        }
        loop {
            // Um segmento pode trazer vários comandos, e um comando pode vir
            // partido em vários segmentos: só se age sobre linhas inteiras.
            while let Some(fim) = self.entrada[..self.tam].iter().position(|&b| b == b'\n') {
                let mut linha: Vec<u8> = self.entrada[..fim].to_vec();
                if linha.last() == Some(&b'\r') {
                    linha.pop();
                }
                self.entrada.copy_within(fim + 1..self.tam, 0);
                self.tam -= fim + 1;
                let fluxo = match core::str::from_utf8(&linha) {
                    Ok(texto) => self.tratar(texto),
                    // Bytes fora do ASCII: um verbo que ninguém conhece.
                    Err(_) => self.tratar("?"),
                };
                if fluxo != Fluxo::Continua {
                    return fluxo;
                }
            }
            if self.tam == TAM_LINHA {
                let _ = self.responder("421 linha longa demais");
                return Fluxo::Encerra;
            }
            match self.controle.read(&mut self.entrada[self.tam..]) {
                // O cliente fechou a conexão sem `QUIT`.
                Ok(0) => return Fluxo::Encerra,
                Ok(n) => self.tam += n,
                Err(NetError::TimedOut) => {
                    let _ = self.responder("421 tempo esgotado, ate logo");
                    return Fluxo::Encerra;
                }
                Err(_) => return Fluxo::Encerra,
            }
        }
    }

    /// Escreve uma resposta no controle, com o `\r\n` do protocolo.
    fn responder(&self, texto: &str) -> Result<(), NetError> {
        let linha = format!("{}\r\n", texto);
        self.controle.write_all(linha.as_bytes())
    }

    /// Responde e segue; se a resposta não puder ser escrita, a sessão acaba
    /// em silêncio (o cliente já foi embora).
    fn dizer(&self, texto: &str) -> Fluxo {
        if self.responder(texto).is_ok() {
            Fluxo::Continua
        } else {
            Fluxo::Encerra
        }
    }

    /// Interpreta uma linha de comando.
    fn tratar(&mut self, linha: &str) -> Fluxo {
        let linha = linha.trim();
        let (verbo, arg) = match linha.split_once(' ') {
            Some((verbo, arg)) => (verbo, arg.trim()),
            None => (linha, ""),
        };
        let verbo = verbo.to_ascii_uppercase();

        if !self.logado && !matches!(verbo.as_str(), "USER" | "PASS" | "QUIT" | "NOOP") {
            return self.dizer("530 faca login primeiro");
        }

        match verbo.as_str() {
            "USER" => {
                if arg.is_empty() {
                    return self.dizer("501 faltou o argumento");
                }
                self.usuario_visto = true;
                self.dizer("331 envie a senha")
            }
            "PASS" => {
                if !self.usuario_visto {
                    return self.dizer("503 envie USER antes");
                }
                self.logado = true;
                self.dizer("230 login aceito")
            }
            "QUIT" => {
                let _ = self.responder("221 ate logo");
                Fluxo::Encerra
            }
            "NOOP" => self.dizer("200 ok"),
            "SYST" => self.dizer("215 UNIX Type: L8"),
            "TYPE" => {
                // Os dois tipos transferem os mesmos bytes: nada é convertido.
                let tipo = arg.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
                match tipo.as_str() {
                    "" => self.dizer("501 faltou o argumento"),
                    "A" => self.dizer("200 tipo A"),
                    "I" => self.dizer("200 tipo I"),
                    _ => self.dizer("504 tipo nao suportado"),
                }
            }
            "SITE" => {
                if arg.is_empty() {
                    self.dizer("501 faltou o argumento")
                } else if arg.eq_ignore_ascii_case("EXIT") {
                    let _ = self.responder("221 encerrando o servidor");
                    Fluxo::Sai
                } else {
                    self.dizer("504 so SITE EXIT existe")
                }
            }
            "PWD" => {
                let resposta = format!("257 \"{}\" e o diretorio atual", self.cwd);
                self.dizer(&resposta)
            }
            "CWD" => {
                if arg.is_empty() {
                    self.dizer("501 faltou o argumento")
                } else {
                    self.mudar_diretorio(arg)
                }
            }
            "CDUP" => self.mudar_diretorio(".."),
            "PASV" | "EPSV" => self.armar_dados(verbo == "PASV"),
            "LIST" | "NLST" => {
                let fluxo = self.listar(arg, verbo == "LIST");
                // Todo comando de dados gasta o ouvinte, dê certo ou não.
                self.dados = None;
                fluxo
            }
            "RETR" => {
                let fluxo = self.baixar(arg);
                self.dados = None;
                fluxo
            }
            "STOR" => {
                let fluxo = self.enviar(arg);
                self.dados = None;
                fluxo
            }
            "SIZE" => self.tamanho(arg),
            "DELE" => self.apagar(arg),
            "MKD" => self.criar_diretorio(arg),
            "RMD" => self.remover_diretorio(arg),
            _ => self.dizer("502 comando nao implementado"),
        }
    }

    /// `CWD` e `CDUP`: só muda de diretório se o destino for mesmo um diretório.
    fn mudar_diretorio(&mut self, arg: &str) -> Fluxo {
        let caminho = match resolver(&self.cwd, arg) {
            Ok(caminho) => caminho,
            Err(resposta) => return self.dizer(&resposta),
        };
        // Abrir e ler a primeira entrada é o que revela um arquivo
        // (`WrongType`) ou um volume ausente; a raiz virtual não existe no kernel.
        if let Err(resposta) = listar_entradas(&caminho) {
            return self.dizer(&resposta);
        }
        self.cwd = caminho;
        self.dizer("250 diretorio mudado")
    }

    /// `PASV` e `EPSV`: abre a escuta de dados e diz ao cliente onde conectar.
    fn armar_dados(&mut self, passivo_simples: bool) -> Fluxo {
        // O ouvinte antigo sai antes do novo entrar: duas escutas na mesma
        // porta dão "porta em uso".
        self.dados = None;
        let ouvinte = match TcpListener::bind(PORTA_DADOS) {
            Ok(ouvinte) => ouvinte,
            Err(NetError::NoBuffers) => return self.dizer("425 sem conexoes livres no kernel"),
            Err(erro) => return self.dizer(&format!("425 erro de rede: {}", erro)),
        };
        if let Err(erro) = ouvinte.set_timeout(ESPERA_DADOS_MS) {
            return self.dizer(&format!("425 erro de rede: {}", erro));
        }
        self.dados = Some(ouvinte);
        if passivo_simples {
            let ip = self.anunciado.0;
            let resposta = format!(
                "227 Entering Passive Mode ({},{},{},{},{},{})",
                ip[0],
                ip[1],
                ip[2],
                ip[3],
                PORTA_DADOS >> 8,
                PORTA_DADOS & 0xff
            );
            self.dizer(&resposta)
        } else {
            let resposta =
                format!("229 Entering Extended Passive Mode (|||{}|)", PORTA_DADOS);
            self.dizer(&resposta)
        }
    }

    /// Espera o cliente abrir a conexão de dados no ouvinte armado. O ouvinte
    /// é fechado aqui, com qualquer desfecho: serve a uma transferência só.
    fn abrir_dados(&mut self) -> Result<TcpStream, String> {
        let ouvinte = self.dados.take().ok_or_else(|| String::from(SEM_PASV))?;
        let conexao = match ouvinte.accept() {
            Ok(conexao) => conexao,
            Err(NetError::TimedOut) => {
                return Err(String::from("425 o cliente nao abriu a conexao de dados"))
            }
            Err(erro) => return Err(format!("425 erro de rede: {}", erro)),
        };
        drop(ouvinte);
        conexao
            .set_timeout(ESPERA_DADOS_MS)
            .map_err(|_| String::from(INTERROMPIDA))?;
        Ok(conexao)
    }

    /// `LIST` (uma linha detalhada por entrada) e `NLST` (só os nomes).
    fn listar(&mut self, arg: &str, detalhado: bool) -> Fluxo {
        if self.dados.is_none() {
            return self.dizer(SEM_PASV);
        }
        // Opções como `-a` e `-la` são ignoradas; a primeira palavra que sobra
        // é o diretório.
        let alvo = arg.split_whitespace().find(|p| !p.starts_with('-')).unwrap_or("");
        let caminho = match resolver(&self.cwd, alvo) {
            Ok(caminho) => caminho,
            Err(resposta) => return self.dizer(&resposta),
        };
        // A listagem inteira é lida antes do `150`: se o diretório não abre,
        // o cliente recebe o erro em vez de uma transferência vazia.
        let entradas = match listar_entradas(&caminho) {
            Ok(entradas) => entradas,
            Err(resposta) => return self.dizer(&resposta),
        };
        if self.dizer("150 abrindo a conexao de dados") == Fluxo::Encerra {
            return Fluxo::Encerra;
        }
        let conexao = match self.abrir_dados() {
            Ok(conexao) => conexao,
            Err(resposta) => return self.dizer(&resposta),
        };
        let mut texto = String::new();
        for entrada in &entradas {
            if detalhado {
                let (modo, tamanho) = if entrada.diretorio {
                    ("drwxr-xr-x", 0)
                } else {
                    ("-rw-r--r--", entrada.tamanho)
                };
                texto.push_str(&format!(
                    "{} 1 os-rust os-rust {} Jan  1  1980 {}\r\n",
                    modo, tamanho, entrada.nome
                ));
            } else {
                texto.push_str(&format!("{}\r\n", entrada.nome));
            }
        }
        let enviado = conexao.write_all(texto.as_bytes());
        // Fechar a conexão é o que avisa o cliente de que a lista acabou.
        drop(conexao);
        match enviado {
            Ok(()) => self.dizer("226 transferencia completa"),
            Err(_) => self.dizer(INTERROMPIDA),
        }
    }

    /// `RETR`: manda o conteúdo de um arquivo.
    fn baixar(&mut self, arg: &str) -> Fluxo {
        if arg.is_empty() {
            return self.dizer("501 faltou o argumento");
        }
        if self.dados.is_none() {
            return self.dizer(SEM_PASV);
        }
        let caminho = match self.alvo(arg, false) {
            Ok(caminho) => caminho,
            Err(resposta) => return self.dizer(&resposta),
        };
        // Abrir e ler o primeiro pedaço antes do `150`: é aqui que um arquivo
        // inexistente, ou um diretório, vira erro sem abrir transferência.
        let mut arquivo = match File::open(&caminho) {
            Ok(arquivo) => arquivo,
            Err(erro) => return self.dizer(&resposta_de_erro(erro)),
        };
        let mut pedaco = [0u8; TAM_PEDACO];
        let mut lidos = match arquivo.read(&mut pedaco) {
            Ok(n) => n,
            Err(erro) => return self.dizer(&resposta_de_erro(erro)),
        };
        if self.dizer("150 abrindo a conexao de dados") == Fluxo::Encerra {
            return Fluxo::Encerra;
        }
        let conexao = match self.abrir_dados() {
            Ok(conexao) => conexao,
            Err(resposta) => return self.dizer(&resposta),
        };

        let mut total = 0;
        let resultado: Result<(), String> = loop {
            if lidos == 0 {
                break Ok(());
            }
            if conexao.write_all(&pedaco[..lidos]).is_err() {
                break Err(String::from(INTERROMPIDA));
            }
            total += lidos;
            lidos = match arquivo.read(&mut pedaco) {
                Ok(n) => n,
                Err(erro) => break Err(resposta_de_erro(erro)),
            };
        };
        // O arquivo e a conexão fecham antes da resposta: quem espera o `226`
        // já pode apagar o arquivo, e o cliente vê o fim dos dados.
        drop(arquivo);
        drop(conexao);
        match resultado {
            Ok(()) => {
                println!("ftpd: enviou {} ({} bytes)", caminho, total);
                self.dizer("226 transferencia completa")
            }
            Err(resposta) => self.dizer(&resposta),
        }
    }

    /// `STOR`: recebe os bytes de um arquivo novo.
    fn enviar(&mut self, arg: &str) -> Fluxo {
        if arg.is_empty() {
            return self.dizer("501 faltou o argumento");
        }
        if self.dados.is_none() {
            return self.dizer(SEM_PASV);
        }
        let caminho = match self.alvo(arg, true) {
            Ok(caminho) => caminho,
            Err(resposta) => return self.dizer(&resposta),
        };
        // Criar antes do `150`: nome repetido, nome fora de 8.3, pasta cheia ou
        // volume cheio viram erro sem transferência e sem arquivo parcial.
        let mut escritor = match FileWriter::create(&caminho) {
            Ok(escritor) => escritor,
            Err(erro) => return self.dizer(&resposta_de_erro(erro)),
        };
        if self.dizer("150 abrindo a conexao de dados") == Fluxo::Encerra {
            drop(escritor);
            let _ = remove(&caminho);
            return Fluxo::Encerra;
        }
        let conexao = match self.abrir_dados() {
            Ok(conexao) => conexao,
            Err(resposta) => {
                drop(escritor);
                let _ = remove(&caminho);
                return self.dizer(&resposta);
            }
        };

        let mut pedaco = [0u8; TAM_PEDACO];
        let mut total = 0;
        let resultado: Result<(), String> = loop {
            match conexao.read(&mut pedaco) {
                // O cliente fechou a conexão de dados: o arquivo está completo.
                Ok(0) => break Ok(()),
                Ok(n) => {
                    if let Err(erro) = escritor.write_all(&pedaco[..n]) {
                        break Err(resposta_de_erro(erro));
                    }
                    total += n;
                }
                Err(_) => break Err(String::from(INTERROMPIDA)),
            }
        };
        // Fechar o arquivo vem antes de apagá-lo: o kernel recusa apagar um
        // arquivo aberto.
        drop(escritor);
        drop(conexao);
        match resultado {
            Ok(()) => {
                println!("ftpd: recebeu {} ({} bytes)", caminho, total);
                self.dizer("226 transferencia completa")
            }
            Err(resposta) => {
                let _ = remove(&caminho);
                self.dizer(&resposta)
            }
        }
    }

    /// `SIZE`: o tamanho de um arquivo, achado na listagem do diretório pai.
    fn tamanho(&mut self, arg: &str) -> Fluxo {
        let caminho = match self.alvo(arg, false) {
            Ok(caminho) => caminho,
            Err(resposta) => return self.dizer(&resposta),
        };
        let (pai, nome) = match caminho.rsplit_once('/') {
            // `/ram` e `/disco` são diretórios.
            Some(("", _)) | None => {
                return self.dizer("550 tipo errado (arquivo ou diretorio)");
            }
            Some(partes) => partes,
        };
        let entradas = match listar_entradas(pai) {
            Ok(entradas) => entradas,
            Err(resposta) => return self.dizer(&resposta),
        };
        match entradas.iter().find(|e| e.nome.eq_ignore_ascii_case(nome)) {
            None => self.dizer(NAO_ENCONTRADO),
            Some(entrada) if entrada.diretorio => {
                self.dizer("550 tipo errado (arquivo ou diretorio)")
            }
            Some(entrada) => self.dizer(&format!("213 {}", entrada.tamanho)),
        }
    }

    /// `DELE`: apaga um arquivo.
    fn apagar(&mut self, arg: &str) -> Fluxo {
        let caminho = match self.alvo(arg, false) {
            Ok(caminho) => caminho,
            Err(resposta) => return self.dizer(&resposta),
        };
        match remove(&caminho) {
            Ok(()) => self.dizer("250 apagado"),
            Err(erro) => self.dizer(&resposta_de_erro(erro)),
        }
    }

    /// `MKD`: cria um diretório.
    fn criar_diretorio(&mut self, arg: &str) -> Fluxo {
        let caminho = match self.alvo(arg, true) {
            Ok(caminho) => caminho,
            Err(resposta) => return self.dizer(&resposta),
        };
        match mkdir(&caminho) {
            Ok(()) => self.dizer(&format!("257 \"{}\" criado", caminho)),
            Err(erro) => self.dizer(&resposta_de_erro(erro)),
        }
    }

    /// `RMD`: remove um diretório vazio.
    fn remover_diretorio(&mut self, arg: &str) -> Fluxo {
        let caminho = match self.alvo(arg, false) {
            Ok(caminho) => caminho,
            Err(resposta) => return self.dizer(&resposta),
        };
        match rmdir(&caminho) {
            Ok(()) => self.dizer("250 removido"),
            Err(erro) => self.dizer(&resposta_de_erro(erro)),
        }
    }

    /// O caminho de um comando que age sobre um arquivo ou diretório: exige o
    /// argumento e recusa a raiz virtual, onde não se lê nem se escreve. Quem
    /// cria (`cria`) algo direto na raiz, como `/x.txt`, recebe o aviso da raiz
    /// em vez de "não encontrado": a raiz existe, só não guarda nada.
    fn alvo(&self, arg: &str, cria: bool) -> Result<String, String> {
        if arg.is_empty() {
            return Err(String::from("501 faltou o argumento"));
        }
        let caminho = match resolver(&self.cwd, arg) {
            Ok(caminho) => caminho,
            Err(resposta) => {
                if cria && normalizar(&self.cwd, arg).len() == 1 {
                    return Err(String::from(RAIZ_SO_LISTA));
                }
                return Err(resposta);
            }
        };
        if caminho == "/" {
            return Err(String::from(RAIZ_SO_LISTA));
        }
        Ok(caminho)
    }
}

/// Uma entrada de diretório, já copiada para fora do kernel.
struct Entrada {
    nome: String,
    diretorio: bool,
    tamanho: u32,
}

/// As entradas de um diretório, na ordem dele. A raiz virtual `/` tem sempre
/// `ram` e `disco`, sem perguntar ao kernel (que não tem raiz única). Um
/// arquivo no lugar de um diretório dá `WrongType` na primeira leitura.
fn listar_entradas(caminho: &str) -> Result<Vec<Entrada>, String> {
    let mut entradas = Vec::new();
    if caminho == "/" {
        for nome in ["ram", "disco"] {
            entradas.push(Entrada { nome: String::from(nome), diretorio: true, tamanho: 0 });
        }
        return Ok(entradas);
    }
    let mut diretorio = Dir::open(caminho).map_err(resposta_de_erro)?;
    while let Some(entrada) = diretorio.next().map_err(resposta_de_erro)? {
        entradas.push(Entrada {
            nome: String::from(entrada.name()),
            diretorio: entrada.is_dir(),
            tamanho: entrada.size(),
        });
    }
    Ok(entradas)
}

/// Transforma o argumento de um comando num caminho que o kernel entende. O
/// kernel não aceita `.` nem `..` e não tem raiz única, então quem normaliza
/// é o `ftpd`: relativo ao diretório atual, sem componentes vazios nem `.`,
/// `..` sobe um nível (na raiz, fica na raiz) e tudo em minúsculas. O primeiro
/// componente tem de ser `ram` ou `disco`. Os limites de tamanho ficam com o
/// kernel.
fn resolver(cwd: &str, arg: &str) -> Result<String, String> {
    let partes = normalizar(cwd, arg);
    if let Some(primeiro) = partes.first() {
        if primeiro != "ram" && primeiro != "disco" {
            return Err(String::from(NAO_ENCONTRADO));
        }
    }
    let mut caminho = String::new();
    for parte in &partes {
        caminho.push('/');
        caminho.push_str(parte);
    }
    if caminho.is_empty() {
        caminho.push('/');
    }
    Ok(caminho)
}

/// Os componentes de `arg` (relativo a `cwd`, se não começar por `/`), sem os
/// vazios nem `.`, com `..` subindo um nível e tudo em minúsculas.
fn normalizar(cwd: &str, arg: &str) -> Vec<String> {
    let base = if arg.starts_with('/') { "" } else { cwd };
    let mut partes: Vec<String> = Vec::new();
    for trecho in base.split('/').chain(arg.split('/')) {
        match trecho {
            "" | "." => {}
            ".." => {
                partes.pop();
            }
            outro => partes.push(outro.to_ascii_lowercase()),
        }
    }
    partes
}

/// A resposta FTP de um erro de arquivo: código `550`, `552`, `553` ou `451`
/// e o texto do próprio erro.
fn resposta_de_erro(erro: FsError) -> String {
    let codigo = match erro {
        FsError::NotFound
        | FsError::NoVolume
        | FsError::WrongType
        | FsError::Exists
        | FsError::ReadOnly
        | FsError::Busy
        | FsError::NotEmpty => 550,
        FsError::Invalid | FsError::PathTooLong => 553,
        FsError::NoSpace => 552,
        FsError::TooManyOpen | FsError::BadDescriptor | FsError::Io | FsError::Fault => 451,
    };
    format!("{} {}", codigo, erro)
}
```

O FTP usa **duas conexões**, e o programa combina os dois tipos de socket com a API de arquivos:

- A **conexão de controle** (porta 21) leva os comandos e as respostas. O `TcpListener` da 21 fica aberto a vida toda; cada `accept` devolve um `TcpStream` que vira uma `Sessao`. Os comandos chegam em pedaços quaisquer, então a sessão acumula bytes e só age sobre linhas inteiras.
- A **conexão de dados** (porta 2100) leva o conteúdo de **uma** transferência. `PASV` e `EPSV` abrem um `TcpListener` novo na 2100 e dizem ao cliente onde conectar; o comando de dados faz `accept` nele e o **gasta**: o ouvinte e a conexão são fechados ao fim de cada transferência, com sucesso ou não. Um programa só tem 4 sockets (a escuta da 21, o controle, a escuta da 2100 e os dados), por isso o servidor atende **um cliente por vez** e só no modo passivo.
- A **validação vem antes do `150`**: `RETR` abre o arquivo e lê o primeiro pedaço, `LIST` lê o diretório e `STOR` cria o arquivo antes de dizer `150`. Assim um nome inexistente ou inválido vira `550` ou `553` sem abrir transferência nenhuma.
- A **limpeza do arquivo parcial**: se um `STOR` falha no meio, o arquivo é fechado (`drop`) e só então apagado com `remove`, porque o kernel não apaga um arquivo aberto. Como `FileWriter::create` nunca sobrescreve, um nome que já existe dá erro e o arquivo antigo fica intacto.
- Não existe espera infinita nem Ctrl+C, então toda espera tem tempo limite e `SITE EXIT` encerra o programa.

## Escrevendo e compilando o seu próprio programa

1. Crie um arquivo novo em `programs/src/bin/`, com o nome do seu programa (por exemplo, `programs/src/bin/meuprograma.rs`).
2. Comece pela estrutura mínima da seção acima: `#![no_std]`, `#![no_main]`, `entry!(main);` e `fn main() -> i32`. Não crie `Cargo.toml`: os programas compartilham o de `programs/`, que já depende da biblioteca de runtime.
3. Escreva a lógica do seu programa usando `print!`/`println!`, `read_line`, e `Box`/`Vec` se precisar (com `extern crate alloc;`).
4. Rode `cargo run` na raiz do repositório. Seu programa é compilado automaticamente junto com os demais, e embutido na imagem de boot.
5. No prompt do os-rust, digite `run meuprograma`. Se o nome não bater com nenhum programa embutido, o prompt lista os nomes disponíveis, então confira o nome do seu arquivo.

Não existe nenhum passo de compilação manual separado: se `cargo run` funciona, seu programa está embutido. Isso vale para programas dentro do repositório; para criar um programa fora dele, veja a seção seguinte.

## Programas fora do repositório

Até aqui, todo programa vivia dentro do repositório, em `programs/src/bin/`, e entrava no kernel a cada `cargo run`. Esta seção ensina o outro caminho: criar o programa **fora** do repositório, numa pasta sua, compilá-lo para o os-rust e levá-lo para dentro do sistema por FTP, **sem recompilar o kernel**. O ciclo tem quatro passos:

1. **Compilar** o programa no seu computador, para o target de usuário do os-rust.
2. **Mandar** o arquivo para o os-rust com o servidor FTP (`ftpd`).
3. **Rodar** no prompt, com `run /disco/<nome>`.
4. **Corrigir** o código, recompilar e mandar de novo.

O caminho é útil quando você quer experimentar rápido, ou quando o programa não deve ficar misturado com o código do projeto. Se você só quer um programa a mais embutido na imagem de boot, a seção anterior continua sendo o caminho mais simples.

### Requisitos

- **`rustup`** instalado. Ele baixa sozinho a toolchain nightly que o projeto fixa e o código-fonte da biblioteca padrão (`rust-src`), de que o `build-std` precisa.
- **O repositório do os-rust clonado**, por exemplo em `~/os-rust`. Dele saem a biblioteca de runtime, o target de usuário e o linker script. O seu programa não copia o `runtime`: ele aponta para o do repositório, e por isso o programa e o kernel são sempre da mesma versão.
- **Internet na primeira compilação.** O `rustup` baixa a nightly e o `rust-src`, e o cargo baixa uma dependência do `runtime` (o `linked_list_allocator`). Depois disso, tudo fica no cache.
- **O QEMU com o FTP funcionando**, como ensina a seção "Servidor FTP" do `README.md`: a linha do QEMU com `hostfwd` e uma cópia do disco, sem `snapshot=on`.

Você **não** precisa do `bootimage` para compilar o programa externo; ele só é necessário para construir o kernel.

**Atenção:** crie o projeto **fora** da pasta do repositório. O cargo procura um workspace subindo pelas pastas, e um projeto criado dentro do os-rust seria tratado como parte dele, com a configuração dele, o que quebra a receita (veja "Erros comuns").

### O que cada arquivo faz

Um programa de dentro do repositório herda configuração das pastas acima dele. Um projeto de fora não herda nada, então você precisa criar, na pasta nova, cada item que o repositório fornece por baixo dos panos. São sete:

| Arquivo | De onde vem | Para que serve |
|---------|-------------|----------------|
| `rust-toolchain.toml` | cópia | Fixa a mesma nightly do kernel e baixa o `rust-src`. Sem ele, o `build-std` não funciona. |
| `x86_64-os_rust_user.json` | cópia | O target de usuário: x86-64 sem sistema, sem SSE, `panic = abort`. |
| `link.ld` | cópia de `programs/link.ld` | Põe o programa em `0x4000_0000`, com seções em páginas separadas (W^X), como o carregador exige. |
| `.cargo/config.toml` | escrito | Liga o `build-std` (compila `core`, `alloc` e `compiler_builtins` para o target). |
| `Cargo.toml` | escrito | Nome do programa e a dependência do `runtime`. |
| `build.rs` | escrito | Passa `-T link.ld` ao linker. |
| `src/main.rs` | escrito | O programa. |

Alguns termos, para quem ainda não os viu:

- **Target** é a descrição da máquina para a qual o compilador gera código. Aqui, um processador x86-64 sem sistema operacional por baixo, descrito no arquivo `x86_64-os_rust_user.json`.
- **`build-std`** é o recurso do cargo que compila a biblioteca padrão mínima (`core`, `alloc` e `compiler_builtins`) para esse target, já que não existe uma versão pronta dela.
- **Linker script** (`link.ld`) é o arquivo que diz ao linker em que endereço o programa fica e como as seções (código, dados) se organizam na memória.
- **W^X** (escrever ou executar, nunca os dois) é a regra de que uma página de memória não pode ser escrita e executada ao mesmo tempo; por isso cada seção fica em páginas separadas.
- **ELF** é o formato do arquivo executável que o compilador gera e que o carregador do os-rust sabe ler.

### Criando o projeto

Os comandos abaixo funcionam em Linux, macOS e no WSL do Windows. A variável `OSRUST` guarda a pasta do repositório; ajuste o valor para o seu caso.

```sh
# 1. onde está o os-rust, e criar o projeto FORA dele
export OSRUST="$HOME/os-rust"
cd "$HOME"
cargo new saudacao --vcs none
cd saudacao

# 2. copiar do repositório a toolchain, o target e o linker script
cp "$OSRUST/rust-toolchain.toml" "$OSRUST/x86_64-os_rust_user.json" "$OSRUST/programs/link.ld" .

# 3. ligar o build-std
mkdir -p .cargo
cat > .cargo/config.toml <<'EOF'
[unstable]
build-std = ["core", "compiler_builtins", "alloc"]
build-std-features = ["compiler-builtins-mem"]
json-target-spec = true
EOF

# 4. Cargo.toml (o caminho do runtime vem de $OSRUST)
cat > Cargo.toml <<EOF
[package]
name = "saudacao"
version = "0.1.0"
edition = "2021"

[dependencies]
runtime = { path = "$OSRUST/runtime" }
EOF

# 5. build.rs: diz ao linker para usar o link.ld
cat > build.rs <<'EOF'
fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    println!("cargo:rustc-link-arg=-T{manifest}/link.ld");
    println!("cargo:rerun-if-changed=link.ld");
}
EOF
```

O que cada passo faz e por que ele existe:

1. **Criar o projeto.** `cargo new saudacao --vcs none` cria a pasta `saudacao` com um `Cargo.toml` e um `src/main.rs`. O `--vcs none` evita criar um repositório git à toa. Use até 8 letras minúsculas no nome: o binário se chama como o pacote, e o nome do arquivo no volume do os-rust precisa caber em 8.3.
2. **Copiar três arquivos.** Copiar, e não escrever à mão, evita divergência: a toolchain, o target e o endereço base ficam idênticos aos do kernel. Sem o `rust-toolchain.toml`, o cargo usaria outra toolchain e o `build-std` não encontraria o `rust-src`.
3. **Ligar o `build-std`.** Sem o bloco `[unstable]`, o cargo procuraria uma biblioteca padrão pronta para o target do os-rust, que não existe, e a compilação falharia com `can't find crate for core`.
4. **Escrever o `Cargo.toml`.** Ele dá o nome ao programa e aponta o `runtime` pelo caminho. O `<<EOF` sem aspas deixa o shell trocar `$OSRUST` pelo valor da variável; nos outros blocos, as aspas (`<<'EOF'`) impedem essa troca, porque o texto deve ficar como está. O arquivo também **não** define `[build] target`: o alvo vem sempre da linha de comando.
5. **Escrever o `build.rs`.** Ele roda antes da compilação e entrega ao linker a opção `-T .../link.ld`. Sem ele, o linker usaria o endereço padrão do Linux, e o programa travaria ao iniciar ou seria recusado pelo carregador do os-rust.

O passo 6 escreve `src/main.rs`, substituindo o "Hello, world!" que o `cargo new` criou:

```rust
#![no_std]
#![no_main]

use runtime::{entry, print, println, read_line};

entry!(main);

fn main() -> i32 {
    print!("seu nome: ");
    let mut buffer = [0u8; 128];
    let nome = read_line(&mut buffer);
    println!("ola, {}! este programa foi feito fora do os-rust.", nome);
    0
}
```

É o mesmo esqueleto da seção "Estrutura mínima de um programa": `#![no_std]` e `#![no_main]` tiram a biblioteca padrão e o `main` convencional, `entry!(main)` liga o ponto de entrada à sua função, e `main` devolve um `i32`, o código de saída (`0` é sucesso). A diferença é a leitura: `read_line` recebe um buffer **seu** (aqui, 128 bytes) e devolve o texto digitado, como no "Programa de exemplo 1: eco". Este programa usa só `print!`, `println!` e `read_line`; não precisa de `alloc`.

Antes de compilar, `ls -a` na pasta do projeto deve mostrar:

```text
.cargo/  Cargo.toml  build.rs  link.ld  rust-toolchain.toml  src/  x86_64-os_rust_user.json
```

O `Cargo.lock` e a pasta `target/` aparecem depois da primeira compilação.

### Compilando

```sh
cargo build --release --target x86_64-os_rust_user.json
wc -c target/x86_64-os_rust_user/release/saudacao
```

A primeira vez demora: o `rustup` baixa a nightly e o `rust-src`, o cargo baixa o `linked_list_allocator`, e o `build-std` compila `core` e `alloc` do zero. As seguintes são rápidas.

O ELF fica em `target/x86_64-os_rust_user/release/saudacao`. O nome da pasta do meio é o nome do target, ou seja, o nome do `.json` sem a extensão, e o nome do binário é o nome do pacote. O segundo comando mostra o tamanho em bytes: um programa deste tipo fica na casa de algumas dezenas de KB (na conferência deste guia, cerca de 17,8 KB). O carregador do os-rust aceita no máximo 64 KiB, ou seja, 65536 bytes; passando disso, o `run` recusa o arquivo.

### Levando por FTP e rodando

1. Suba o os-rust com a linha de QEMU da seção "Servidor FTP" do `README.md` (cópia do disco, sem `snapshot=on`, com `hostfwd`). A linha não é repetida aqui para que haja uma única versão dela.
2. No prompt do os-rust, digite `run ftpd` e aperte `Enter` na pergunta do endereço, para aceitar `127.0.0.1`. O servidor mostra que está ouvindo na porta 21.
3. Em outro terminal, **em até 30 segundos** (o `ftpd` desiste se ninguém conectar nesse prazo), envie o programa, a partir da pasta do projeto:

```sh
curl -T target/x86_64-os_rust_user/release/saudacao ftp://127.0.0.1:2121/disco/saudacao
```

   O `-T` manda o arquivo; o destino é `/disco/saudacao`, no volume do disco. A porta 2121 do seu computador é encaminhada pelo QEMU para a 21 do os-rust.
4. Ainda dentro da janela de 30 segundos (o `ftpd` volta a esperar outro cliente depois de cada sessão), confira pelo próprio FTP com `curl ftp://127.0.0.1:2121/disco/`. A listagem deve mostrar `saudacao` com o mesmo tamanho que o `wc -c` deu.
5. Termine o servidor, para que o prompt do os-rust volte: `curl -Q "SITE EXIT" ftp://127.0.0.1:2121/`. Se preferir, basta esperar 30 segundos sem cliente.
6. No prompt do os-rust, confira com `ls /disco` e rode o programa com `run /disco/saudacao`:

```text
os-rust> run /disco/saudacao
seu nome: Cleuton
ola, Cleuton! este programa foi feito fora do os-rust.
os-rust>
```

### Alterando e reenviando

Mudou o texto? Recompile com o mesmo `cargo build`. Para reenviar, **apague a versão antiga antes**, porque o `ftpd` nunca sobrescreve um arquivo que já existe (assim uma transferência interrompida não destrói o original):

```sh
curl -Q "DELE /disco/saudacao" ftp://127.0.0.1:2121/
```

e depois repita o `curl -T` da seção anterior. O `ftpd` atende uma sessão por vez e volta a esperar cliente, mas cada `curl` tem de começar em até 30 segundos depois do anterior; senão o `ftpd` encerra, o prompt volta, e é preciso rodar `run ftpd` de novo.

Lembre-se de onde o arquivo mora. O disco do `cargo run` usa `snapshot=on`, e por isso o arquivo enviado some no próximo boot; com a linha do QEMU do `README.md` (cópia do disco, sem `snapshot=on`), ele fica. Já o `/ram` volta ao conteúdo original em todo boot, aconteça o que acontecer.

### Regras que continuam valendo

- O contrato de syscalls é o mesmo (`SYSCALLS.md`), e tudo o que este guia ensina sobre o `runtime` (arquivos, rede, hora, `alloc`) vale igualmente para o programa externo.
- O programa não recebe argumentos de linha de comando.
- O arquivo executável pode ter no máximo 64 KiB (65536 bytes).
- No máximo 4 programas rodam ao mesmo tempo.
- Quando você atualizar o repositório do os-rust, **recompile** o programa externo: o contrato de syscalls pode ter mudado.

### Resumo

1. `cargo new saudacao --vcs none`, fora do repositório.
2. `cp` de `rust-toolchain.toml`, `x86_64-os_rust_user.json` e `link.ld`.
3. Escreva `.cargo/config.toml`, `Cargo.toml`, `build.rs` e `src/main.rs`.
4. `cargo build --release --target x86_64-os_rust_user.json`.
5. `run ftpd` no os-rust e, em até 30 segundos, `curl -T ... ftp://127.0.0.1:2121/disco/saudacao`.
6. `SITE EXIT`, e no prompt `run /disco/saudacao`. Para trocar o programa: `DELE`, recompile, `curl -T` outra vez.

## Testando

A forma mais simples de testar é manual: `cargo run`, depois `run <nome do seu programa>`, observando a tela.

Para verificação automatizada, `cargo test` roda a suíte do kernel, que inclui testes cobrindo o carregamento e a execução de programas de usuário (usando `hello`, `eco` e `falha_memoria` como referência), de vários programas ao mesmo tempo (`tests/multitarefa.rs`: `ping`, `pong`, `contador_a`, `contador_b`, `eco2`) e de programas que leem arquivos (`tests/sistema_de_arquivos.rs`: `leitor`, `listador`, `visita`) e que pedem a hora (`tests/relogio.rs`: `hora`). Se você quiser um teste dedicado ao seu próprio programa, o padrão usado pelo projeto é um arquivo em `tests/`, dando boot no kernel, "digitando" a entrada com `interrupts::push_scancode`, e verificando o resultado na tela; `tests/user_runtime.rs` é o modelo.

## Erros comuns

- **Meu programa não aparece em `run`**: confira se o nome do arquivo em `programs/src/bin/` (sem `.rs`) é exatamente o que você digitou depois de `run`, e se `cargo run` terminou sem erro de compilação.
- **`cargo run` falha ao compilar meu programa**: o erro apontado é do seu código, não do kernel; corrija e rode `cargo run` de novo. O kernel não sobe com um programa que falhou ao compilar.
- **Erro de ligação dizendo que falta `_start`**: faltou `entry!(main);` no seu programa.
- **Meu programa e outro estão intercalando a saída e eu queria uma ordem fixa**: a ordem entre programas só é a do rodízio de `yield_now` enquanto o timer não interrompe ninguém; o kernel pode interromper qualquer programa em qualquer instrução. Se a ordem importa, cada programa precisa esperar pelo que precisa de outro jeito (dois programas só conversam pela rede, por sockets TCP no `127.0.0.1`; veja "Rede (TCP)").
- **Meu programa trava e nunca volta ao prompt**: confira se todo caminho de código do seu `main` eventualmente retorna (ou chama `exit`); um programa que nunca sai continua ocupando o prompt. Se ele está em `read_line`, está esperando você apertar Enter.
- **`memory allocation of N bytes failed` e `terminou com codigo 101`**: o programa pediu mais memória do que o heap de 1 MiB comporta (a mensagem vem depois de uma linha `[panic] panicked at ...`).
- **Meu programa usa uma syscall que não existe**: o kernel encerra o programa com uma mensagem de erro, do mesmo jeito que trata um acesso inválido à memória; confira o número da syscall no `SYSCALLS.md`.
- **`File::open` devolve `NoVolume` para `/disco`**: o QEMU subiu sem o disco, ou o volume está com defeito. `cargo run` já anexa o disco sozinho; se você roda o QEMU à mão, anexe `target/imagens/disco.img` como disco IDE (`-drive file=target/imagens/disco.img,format=raw,if=ide,index=1`).
- **`File::open` devolve `NotFound` para um arquivo que está em `discos/`**: o nome precisa caber em 8.3 (até 8 caracteres, ponto, até 3 de extensão), e `cargo run` precisa ter sido refeito depois de você adicionar o arquivo.
- **`run /disco/...` diz `arquivo grande demais`**: executáveis lidos de arquivo têm no máximo 64 KiB.
- **O cargo não acha `runtime/Cargo.toml` (programa fora do repositório)**: `OSRUST` está errado, ou o `Cargo.toml` foi escrito quando `OSRUST` estava vazio. Confira com `echo "$OSRUST"` e escreva o `Cargo.toml` de novo.
- **`can't find crate for core`, ou o cargo pede para instalar o `rust-src`**: falta o `rust-toolchain.toml` copiado, ou falta o `.cargo/config.toml` com o `build-std`, ou o cargo foi rodado fora da pasta do projeto.
- **`relocation R_X86_64_32 out of range`, ou o programa trava ao iniciar**: o `link.ld` ou o `build.rs` não está em uso. Confira se os dois estão na raiz do projeto.
- **Erro de ligação dizendo que falta `_start` (programa fora do repositório)**: falta `entry!(main);` no `src/main.rs`.
- **O cargo diz que o pacote "acredita estar num workspace que não é o seu"**: o projeto foi criado dentro da pasta do os-rust. Crie-o fora dela.
- **`run /disco/<nome>` diz `arquivo grande demais`**: o ELF passa de 64 KiB (65536 bytes). Confira o tamanho com `wc -c`.
- **`run` diz que o arquivo não é um ELF ou que o executável é inválido**: você enviou o arquivo errado (por exemplo `target/x86_64-os_rust_user/release/saudacao.d`). O ELF é o arquivo sem extensão.
- **`curl` termina com erro de FTP ao enviar**: o arquivo já existe (apague com `DELE /disco/saudacao` e envie de novo) ou o `ftpd` já tinha encerrado por esperar mais de 30 segundos (rode `run ftpd` outra vez).
