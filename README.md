# os-rust

```text
                    #####
                   // : \\
                ..-  --:  -..
           //--/ .:==███==+. \--\\
          //-\ +- ███ █ ███ -+ /-\\
           \\ | ██  @@@@@  ██ | //
            | | @  @     @  @ | |
            | | @██@ ███ @██@ | |
            | | @  @@@@@@   @ | |
           // | ██ @  █  % ██ | \\
          \\.- -- ███ █ ███ -- -.//
           \\--\. -- ███ -- ./--//
                 --. -.- .--
                   \\ : //
                    #####
                                         []
 .o:o.   .s::.       [.o= []  |]   .s:: =++=t
[o   o] [:___  ===== [r   []  |]  [:___  []
[o   o]     :]       []   []  |]      :] []
  o:o   ::__:        []    uu/|]  ::__:   \ut
```

![](imagem.jpg)

O os-rust começou como uma demonstração mínima de "programação sem
sistema operacional" em Rust: um binário que dá boot direto via BIOS em
uma máquina virtual QEMU, escreve texto na tela usando o buffer de vídeo
VGA e lê o teclado via interrupção de hardware (IRQ1) para alimentar um
prompt de comandos mínimo, sem nenhum sistema operacional por baixo. A
partir daqui, o projeto passa a evoluir em etapas pequenas rumo a um
objetivo maior: veja a seção Visão logo abaixo.

Se você nunca viu como um kernel/bootloader funciona, leia também o
[`WALKTHROUGH.md`](./WALKTHROUGH.md): ele explica o que está acontecendo
por trás do boot e do código.

## Visão

O objetivo central do os-rust é permitir que alguém escreva um programa,
compile esse programa separadamente do kernel e o execute no os-rust, em
modo usuário, usando uma interface de programação documentada. Não é um
kernel de propósito geral: é um kernel educacional que evolui por marcos
pequenos, cada um pensado para caber em uma aula, sempre priorizando o
caminho mais curto até rodar programas de usuário antes de recursos como
sistema de arquivos ou multitarefa completa.

## Status

**Versão atual: 0.9.0.** Os Marcos 0 (boot em modo texto VGA, com
mensagem de boas-vindas, rolagem e tratamento de panic legível), 1
(interrupções, teclado e prompt de comandos), 2 (infraestrutura de
depuração: saída serial e testes automatizados dentro do QEMU), 3
(memória: alocador de frames físicos, paginação e heap do kernel), 4
(proteção: GDT e TSS próprias, handlers para as exceções principais,
double fault com pilha dedicada), 4.1 (nova identidade: o projeto passa
a se chamar os-rust, com o logo acima aparecendo na tela a cada boot) e
5 (primeiro programa de usuário: ring 3, syscalls `write` e `exit`,
carregador de ELF64 estático e o comando `run hello`), 6 (interface de
programação: leitura de teclado e memória para os programas, biblioteca de
runtime, e o isolamento de falhas demonstrado com `run eco` e `run
falha_memoria`) e 7 (multitarefa: vários programas ao mesmo tempo, cada um na
sua memória, com troca de contexto cooperativa e preemptiva, demonstrada com
`run ping pong` e `run contador_a contador_b`) e 8 (sistema de arquivos: um
ramdisk e um disco ATA, ambos FAT16 somente leitura, com `ls`, `cat`, syscalls
de arquivo e a execução de um programa lido do disco, demonstrada com `run
/disco/bin/visita`) e 9 (driver de RTC: o kernel lê a data e a hora do
relógio do computador, mostradas pelo comando `data` e oferecidas aos programas
pela syscall `SYS_TIME`, com `run hora`) estão concluídos e são o que este
repositório executa hoje. Os Marcos 10 e 11 (escrita em arquivos) estão
planejados. A demonstração original de palestra, no
formato usado em aula, está preservada na tag git `v1.0-demo` e continua
podendo ser usada como está.

## Roadmap

O os-rust avança em marcos numerados, cada um terminando em algo visível
no QEMU. A tabela abaixo resume os treze primeiros marcos planejados; os
detalhes de cada um vêm na sequência.

| Marco | Objetivo | Demonstrável | Status |
|---|---|---|---|
| 0. Boot e texto VGA | Dar boot via BIOS e escrever texto em modo VGA, com tratamento de panic legível. | Mensagem de boas-vindas, rolagem de texto e uma tela de panic legível. | Concluído |
| 1. Interrupções, teclado e prompt | Tratar interrupções de hardware, ler o teclado e oferecer um prompt de comandos fixos. | Digitar `help` no prompt e ver a resposta. | Concluído |
| 2. Infraestrutura de depuração | Ter saída serial e testes automatizados rodando dentro do QEMU. | `cargo test` executando testes do kernel dentro do QEMU. | Concluído |
| 3. Memória | Alocar frames físicos e páginas, e ter um alocador de heap dentro do kernel. | `Vec` e `Box` funcionando dentro do kernel, visíveis por um comando do prompt. | Concluído |
| 4. Proteção | Ter GDT e TSS próprias e tratar as exceções principais, incluindo page fault e double fault com pilha dedicada. | Provocar um page fault e ver uma mensagem legível em vez de reboot. | Concluído |
| 4.1. Nova identidade | Renomear o projeto para os-rust em todo lugar (pacote, crate, target, pastas, prompt, mensagens, documentação) e acrescentar o logo na tela de boot e no README. | `cargo run` mostra o logo, depois `os-rust v0.4.1` e o prompt `os-rust> `. | Concluído |
| **5. Primeiro programa de usuário (marco central)** | Rodar o primeiro programa de usuário em modo protegido (ring 3), usando um mecanismo de syscall e um carregador de executáveis ELF64 embutidos na imagem de boot. | Comando `run hello` no prompt executa um programa em modo usuário que imprime na tela e retorna ao prompt. | Concluído |
| 6. Interface de programação | Ampliar o contrato de syscalls (teclado, memória, código de saída) e oferecer uma biblioteca de runtime para quem escreve programas. | Um programa escrito por um aluno lê entrada do teclado e responde (`run eco`); um programa com acesso inválido à memória é encerrado sem derrubar o kernel (`run falha_memoria`). | Concluído |
| 7. Multitarefa | Trocar de contexto entre mais de um programa carregado, primeiro de forma cooperativa e depois preemptiva. | Dois programas intercalando saída na tela (`run ping pong`, `run contador_a contador_b`). | Concluído |
| 8. Sistema de arquivos | Ler arquivos de um sistema de arquivos, primeiro um ramdisk embutido e depois um driver de disco com leitura somente. | Listar arquivos e executar um programa lido do disco (`ls /ram`, `cat /ram/ola.txt`, `run /disco/bin/visita`). | Concluído |
| 9. Driver de RTC | Ler a data e a hora do relógio de tempo real (chip CMOS) por polling e oferecê-las ao prompt e aos programas de usuário por uma syscall nova. | `data` mostra a data e a hora em UTC; `run hora` mostra o mesmo, pedido ao kernel por um programa. | Concluído |
| 10. Escrita no ramdisk | Escrever em FAT16, criando, gravando, ampliando e apagando arquivos, aplicado ao volume `/ram`. | Criar um arquivo em `/ram`, mostrá-lo com `cat` e apagá-lo, também por programa de usuário. | Planejado |
| 11. Escrita no disco ATA | Gravar setores no disco ATA e tornar o `/disco` gravável pela mesma camada de escrita, com persistência entre boots. | Gravar um arquivo em `/disco`, reiniciar o QEMU e ler o arquivo. | Planejado |

### Detalhes dos marcos

**Marco 0. Boot e texto VGA.** Concluído. Boot via BIOS, mensagem de boas
vindas, rolagem de texto e tela de panic legível. Não depende de nenhum
outro marco.

**Marco 1. Interrupções, teclado e prompt.** Concluído. IDT, PIC 8259,
handler de IRQ1, tradução de scancodes e um prompt com comandos fixos.
Demonstrável: digitar `help` e ver a resposta. Depende do Marco 0.

**Marco 2. Infraestrutura de depuração.** Concluído. Saída serial e
testes automatizados rodando dentro do QEMU, com resultado reportado ao
host. Demonstrável: `cargo test` rodando testes do kernel no QEMU.
Depende do Marco 1.

**Marco 3. Memória.** Concluído. Alocador de frames físicos a partir do
mapa de memória do bootloader (usando o mapeamento completo da física
que a feature `map_physical_memory` do `bootloader` fornece),
tradução/criação de mapeamentos na tabela de páginas ativa, e um heap
fixo mapeado no boot (100 KiB neste marco; 16 MiB desde o Marco 8), com um alocador global (`Box`, `Vec`,
`String`, ...). Demonstrável: o comando `mem` no prompt mostra a memória
física utilizável, a posição/tamanho do heap, um `Box` com seu endereço,
e um `Vec` construído a partir de vazio. Depende do Marco 2.

**Marco 4. Proteção.** Concluído. GDT própria (segmento de código do
kernel + descritor da TSS) e uma TSS com uma pilha dedicada de 20 KiB na
Interrupt Stack Table para o double fault; handlers para as cinco
exceções principais (`#BP`, `#UD`, `#GP`, `#PF`, `#DF`), com tela legível
(tela + serial) para as quatro fatais. Demonstrável: o comando `falha
<tipo>` no prompt provoca cada exceção de propósito — `falha pagina`
mostra uma mensagem legível em vez de reboot, e `falha pilha` prova que
um estouro de pilha do kernel não reinicia mais o QEMU. Depende do
Marco 3.

**Marco 4.1. Nova identidade.** Concluído. Marco de manutenção: o
projeto passa a se chamar `os-rust` em todo lugar (pacote em
`Cargo.toml`, crate `os_rust`, target customizado `x86_64-os_rust.json`,
pasta raiz do repositório, prompt, mensagens do kernel e documentação),
a versão sobe para 0.4.1, e o logo ASCII acima aparece na tela de boot,
antes da identificação e do prompt. Nenhuma capacidade nova do kernel.
Demonstrável: `cargo run` mostra o logo completo, depois
`os-rust v0.4.1`, depois o prompt `os-rust> `. Depende do Marco 4.

**Marco 5. Primeiro programa de usuário (marco central do roadmap).**
Concluído. Ring 3, mecanismo de syscall (`syscall`/`sysret`), primeira versão do contrato de syscalls com
`write` e `exit`, carregador de ELF64 estático e um programa embutido na
imagem de boot, compilado à parte do kernel e embutido em tempo de
compilação. O contrato v1 está em [`SYSCALLS.md`](SYSCALLS.md).
Demonstrável: o comando `run hello` no prompt executa um programa em modo
usuário que imprime na tela e retorna ao prompt; `run crash` mostra que
uma falha em programa não derruba o kernel. Depende do Marco 4. Este é o marco que entrega o objetivo central descrito na
seção Visão.

**Marco 6. Interface de programação.** Concluído. Contrato de syscalls
ampliado (versão 2 do [`SYSCALLS.md`](SYSCALLS.md): `SYS_READ_LINE` lê uma
linha do teclado e `SYS_ALLOC` dá memória ao programa), uma biblioteca de
runtime para programas (`runtime/`: ponto de entrada com `entry!`, `print!`
e `println!`, `read_line` e um alocador que permite `Box` e `Vec`), e
isolamento: uma falha no programa encerra o programa, não o kernel.
Demonstrável: `run eco` lê uma linha do teclado e responde; `run
falha_memoria` acessa memória inválida e é encerrado sem derrubar o kernel,
com o prompt respondendo logo depois. Quem quiser escrever o próprio
programa segue o [`GUIA_DO_PROGRAMADOR.md`](GUIA_DO_PROGRAMADOR.md).
Depende do Marco 5.

**Marco 7. Multitarefa.** Concluído. Mais de um programa carregado ao mesmo
tempo, cada um na sua própria memória (uma tabela de páginas por programa),
com troca de contexto primeiro cooperativa (a syscall `SYS_YIELD`, contrato
versão 3 do [`SYSCALLS.md`](SYSCALLS.md)) e depois preemptiva (o timer PIT, a
100 Hz, pela IRQ0 do PIC 8259, tira a CPU de quem não a cede depois de uma
fatia de 50 ms). O escalonador é um rodízio, o kernel nunca é trocado, e o
teclado vai ao programa que pediu primeiro. Demonstrável: `run ping pong`
mostra as linhas dos dois alternadas (cada um cede a CPU ao outro);
`run contador_a contador_b` mostra a saída dos dois intercalada sem que nenhum
peça a vez; `run falha_memoria contador_a` encerra só quem falhou.
Depende do Marco 5.

**Marco 8. Sistema de arquivos.** Concluído. O kernel passa a ler arquivos,
**somente leitura** (até o Marco 9; a escrita chega com os Marcos 10 e 11), de dois volumes FAT16 fixos: `/ram`, um ramdisk
embutido na imagem de boot, e `/disco`, um disco ATA lido por PIO (portas de
E/S, por polling, sem DMA e sem interrupção de disco). O mesmo leitor de FAT
lê os dois, através de uma abstração mínima de dispositivo de blocos. Os
comandos `ls` e `cat` listam e mostram arquivos; quatro syscalls novas
(`SYS_OPEN`, `SYS_READ`, `SYS_CLOSE`, `SYS_READ_DIR`, contrato versão 4 do
[`SYSCALLS.md`](SYSCALLS.md)) deixam um programa abrir, ler e listar, cada um
com a sua tabela de arquivos; e `run` aceita o caminho de um executável.
As imagens dos volumes são geradas dentro de `cargo run`/`cargo test` pela
crate `fatimg`, a partir do diretório `discos/`, sem nenhuma ferramenta nova.
Demonstrável: `ls /ram` e `cat /ram/ola.txt` mostram o ramdisk; `run
/disco/bin/visita` executa um programa que **existe só no disco** (o kernel
nunca o viu em tempo de compilação); `run leitor` e `run listador` leem o
disco por syscalls. Sem o disco, o kernel dá boot normalmente e `ls /disco`
diz que o volume está indisponível. Depende do Marco 5.

**Marco 9. Driver de RTC.** Concluído. O kernel passa a ter o quinto driver
(depois de VGA, serial, teclado PS/2 e disco ATA): o relógio de tempo real, o
chip CMOS que mantém a data e a hora com bateria própria. O chip é lido por
duas portas de E/S (`0x70` escolhe o registrador, `0x71` entrega o valor), por
polling, sem a interrupção IRQ8 e sem mexer no PIC. O driver trata valores em
BCD ou binário, hora em 12 ou 24 horas, o ano de dois dígitos (com o registrador
de século, quando existe), espera o fim da atualização que o chip faz a cada
segundo e lê duas vezes até as leituras coincidirem, e rejeita datas
impossíveis (mês 0, 31 de fevereiro). O kernel só conhece UTC: não há fuso
horário. Uma syscall nova, `SYS_TIME` (contrato versão 5 do
[`SYSCALLS.md`](SYSCALLS.md)), entrega a hora a um programa, e a biblioteca de
runtime a esconde atrás de `time::now()`. Demonstrável: `data` mostra
`AAAA-MM-DD HH:MM:SS UTC` e, alguns segundos depois, uma hora posterior; `run
hora` mostra a mesma hora, pedida ao kernel por um programa. Depende do Marco 5.

**Marco 10. Escrita no ramdisk.** Planejado. A primeira metade da escrita em
arquivos, no volume que vive na memória e se perde ao reiniciar, onde errar não
custa nada. Escopo: a camada de escrita do FAT16 (alocar e liberar clusters,
atualizar as duas cópias da FAT, criar, gravar, ampliar e apagar arquivos),
aplicada ao volume `/ram`; comandos de prompt para criar e apagar arquivos;
syscalls novas de criação, escrita e remoção (contrato versão 6); um programa de
exemplo que grava um arquivo e outro programa que o lê. O `/disco` continua
somente leitura. Demonstrável: criar um arquivo em `/ram`, mostrá-lo com `cat` e
apagá-lo, também por programa de usuário. Depende do Marco 8.

**Marco 11. Escrita no disco ATA.** Planejado. A segunda metade: levar a mesma
camada ao disco de verdade. Escopo: escrita de setores por PIO no driver ATA
(incluindo o comando de descarga do cache do disco); o `/disco` gravável pela
mesma camada do Marco 10; criação e remoção de diretórios; a ordem das escritas
que mantém o volume consistente se for interrompida; e a persistência entre dois
boots do QEMU sobre a mesma imagem, com a decisão de como o `build.rs` deixa de
regenerar o disco quando o objetivo é preservá-lo (hoje ele o regenera a cada
`cargo run`/`cargo test`, o que apagaria qualquer gravação). Os testes operam
sempre sobre uma cópia da imagem. Contrato versão 7, se houver syscalls novas
(por exemplo, de diretório). Demonstrável: gravar um arquivo em `/disco`,
reiniciar o QEMU e ler o arquivo. Depende do Marco 10.

### Por que essa ordem

O primeiro programa de usuário (Marco 5) não depende de sistema de
arquivos nem de multitarefa: até existir sistema de arquivos, o programa
viaja embutido na própria imagem de boot, e basta rodar um programa por
vez para provar que o modo usuário funciona de ponta a ponta. Por isso
sistema de arquivos (Marco 8) e multitarefa (Marco 7) vêm depois do
primeiro programa de usuário, e não antes.

### Interface de programação

A interface de programação entre o kernel e os programas de usuário (o
contrato de syscalls: números, semântica, convenções de registradores,
códigos de erro, formato de executável, região de carga e pilha inicial)
está documentada em um único arquivo versionado, [`SYSCALLS.md`](SYSCALLS.md),
desde o Marco 5 (versão 1 do contrato: `write` e `exit`). A versão atual é a
5, do Marco 9, que acrescenta ao que já havia (leitura de uma linha do
teclado e memória, no Marco 6; ceder a CPU, no Marco 7; leitura de arquivos,
no Marco 8) a hora (`SYS_TIME`). Ele é a única fonte dessa
interface: nenhuma syscall existe sem estar nele, e um teste automatizado
confere que o texto continua batendo com o código. Uma mudança incompatível
no contrato aumenta a versão dele e exige atualizar, no mesmo marco, a
biblioteca de runtime dos programas (`runtime/`), que usa as mesmas
constantes que o kernel (crate `abi`).

Quem quiser **escrever o próprio programa de usuário** deve seguir o
[`GUIA_DO_PROGRAMADOR.md`](GUIA_DO_PROGRAMADOR.md): a estrutura mínima de um
programa, a biblioteca de runtime (`print!`, `read_line`, `Box`/`Vec`), um
resumo do contrato e o passo a passo para compilar e rodar com `run <nome>`.
O [`WALKTHROUGH.md`](WALKTHROUGH.md) explica o kernel por dentro; o guia
explica como programar para ele, sem precisar entender GDT, IDT ou o
carregador de ELF64.

### Sobre mudar essa ordem

A tabela de marcos acima é a referência oficial do projeto. Mudar a
ordem dos marcos ou inserir um marco novo exige atualizar esta tabela no
README antes de qualquer implementação começar.

## Pré-requisitos

Você vai precisar de três coisas: o Rust (com a toolchain **nightly**), a
ferramenta `bootimage` e o **QEMU**. O passo a passo abaixo assume que você
nunca instalou nenhum dos três.

### 1. Rust + toolchain nightly

Se você ainda não tem o Rust instalado, instale via
[rustup](https://rustup.rs/):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Este projeto já pina a toolchain nightly exata que ele precisa através do
arquivo `rust-toolchain.toml` na raiz do repositório: você **não** precisa
rodar `rustup default nightly` nem nada parecido; o `cargo` detecta esse
arquivo automaticamente e baixa a toolchain certa na primeira vez que você
compilar o projeto. Você só precisa garantir que o `rustup` em si está
instalado (passo acima).

Se preferir instalar a toolchain manualmente antes de compilar, use a mesma
versão fixada em `rust-toolchain.toml`:

```sh
rustup toolchain install "$(grep channel rust-toolchain.toml | cut -d'"' -f2)" \
  --component rust-src,llvm-tools-preview
```

A toolchain é pinada em uma data exata (não "nightly" flutuante) porque o
formato do arquivo JSON de especificação de target customizado usado neste
projeto é instável e muda de vez em quando entre versões do nightly: uma
data fixa garante que `cargo run` funcione igual em qualquer máquina, hoje
e daqui a um ano.

### 2. `bootimage`

`bootimage` é a ferramenta que transforma o binário do kernel em uma imagem
de disco bootável e sabe como chamar o QEMU. Instale a versão exata testada
por este projeto com:

```sh
cargo install bootimage --version 0.10.5 --locked
```

### 3. QEMU

QEMU é o emulador de máquina virtual usado para "dar boot" no sistema sem
precisar de hardware físico.

- **Ubuntu/Debian**:
  ```sh
  sudo apt install qemu-system-x86
  ```
- **Fedora**:
  ```sh
  sudo dnf install qemu-system-x86
  ```
- **Arch Linux**:
  ```sh
  sudo pacman -S qemu-full
  ```
- **macOS** (via [Homebrew](https://brew.sh/)):
  ```sh
  brew install qemu
  ```
- **Windows**: baixe o instalador em
  [qemu.org/download](https://www.qemu.org/download/#windows) e garanta que
  `qemu-system-x86_64.exe` fique disponível no `PATH`.

Confirme que o QEMU está acessível:

```sh
qemu-system-x86_64 --version
```

## Compilando e rodando

Com os três pré-requisitos acima instalados, a partir da raiz do
repositório rode:

```sh
cargo run
```

Isso vai: compilar o kernel para o target bare-metal customizado deste
projeto (`x86_64-os_rust.json`), gerar uma imagem de boot com `bootimage`,
e abrir uma janela do QEMU que dá boot via BIOS direto nesse binário. Em
poucos segundos você deve ver o logo do os-rust (as 20 linhas do símbolo
e do nome, no topo da tela), seguido da linha de identificação
`os-rust v0.9.0` — a versão atual do projeto — e do prompt `os-rust> `
pronto para digitação, não um terminal comum.

O mesmo `cargo run` também compila a biblioteca de runtime (`runtime/`) e
os programas de usuário (a pasta `programs/`, para o target
`x86_64-os_rust_user.json`) e os embute na imagem de boot, e também gera as
imagens do ramdisk e do disco (`discos/`, pela crate `fatimg`) e anexa o disco
ao QEMU (`target/imagens/disco.img`, pelos argumentos do `bootimage` no
`Cargo.toml`): não há nenhum passo manual a mais, nem ferramenta nova. Rode
`cargo run` e `cargo test` sempre a partir da raiz do repositório: o caminho do
disco é relativo a ela. Se um programa de
usuário não compilar, o `cargo run` para com o erro do compilador em vez
de gerar uma imagem com um programa desatualizado.

Clique na janela do QEMU para garantir que ela tem o foco do teclado e
digite um comando. O layout de teclado suportado é **US QWERTY, somente
ASCII**: não há suporte a acentuação, ABNT2 ou outros layouts.

Ao mesmo tempo, o próprio terminal onde você rodou `cargo run` passa a
mostrar mensagens de diagnóstico escritas pelo kernel (início do boot com
a versão, GDT/TSS ativas, interrupções ativadas, memória inicializada,
prompt pronto, e qualquer breakpoint, instrução inválida, violação de
proteção, page fault, double fault ou panic que aconteça) — um canal de
texto separado da tela do QEMU, que pode ser rolado, copiado e colado.
Não é preciso nenhum passo manual adicional para isso: é o mesmo
`cargo run` de sempre.

### Comandos disponíveis

| Comando | O que faz |
|---|---|
| `help` | Lista os comandos disponíveis |
| `clear` | Limpa a tela e reposiciona o prompt no topo |
| `echo <texto>` | Escreve `<texto>` na linha seguinte |
| `sobre` | Mostra uma descrição curta do os-rust, incluindo a versão atual |
| `panic` | Dispara um panic proposital (mesma tela de erro do tratamento de panic) |
| `mem` | Mostra memória física utilizável, posição/tamanho do heap, um `Box` e um `Vec` |
| `falha <tipo>` | Provoca uma exceção de CPU de propósito: `pagina` (`#PF`), `pilha` (`#DF`), `opcode` (`#UD`), `protecao` (`#GP`) ou `breakpoint` (`#BP`); sem argumento ou com um tipo desconhecido, lista os tipos disponíveis |
| `ls <caminho>` | Lista as entradas de um diretório (tipo, tamanho e nome), por exemplo `ls /ram` ou `ls /disco/docs`; sem argumento mostra o uso e o estado dos volumes; caminho inexistente, arquivo no lugar de diretório e volume indisponível dão uma mensagem clara |
| `cat <caminho>` | Mostra o conteúdo de um arquivo, por exemplo `cat /ram/ola.txt` (bytes fora do ASCII imprimível aparecem como `■`); mesmas mensagens de erro de `ls` |
| `data` | Mostra a data e a hora do relógio do computador, em UTC, no formato `AAAA-MM-DD HH:MM:SS UTC`; se o relógio estiver inválido ou não responder, mostra uma mensagem clara. Argumentos extras são ignorados |
| `run <alvo> [<alvo>...]` | Executa programas de usuário em modo usuário (ring 3), até 4 ao mesmo tempo (o mesmo alvo pode se repetir), e volta ao prompt quando o último termina. Um alvo que começa com `/` é o caminho de um executável ELF64 (até 64 KiB) lido de um volume, como `/disco/bin/visita`; qualquer outro é o nome de um programa embutido na imagem. Os dois se misturam (`run /disco/bin/visita hello`). Sem argumento, com um alvo inválido ou com mais de 4 alvos, não inicia nenhum |

### Programas de usuário: `hello`, `crash`, `eco`, `falha_memoria`, `ping`, `pong`, `contador_a`, `contador_b`, `eco2`, `leitor`, `listador`, `hora` e `visita`

O comando `run` executa programas escritos fora do kernel. Cada um vive em
`programs/src/bin/` e é compilado como um executável ELF64 estático:

- **`hello`** (`run hello`): escreve `Ola do ring 3!` e termina. Demonstra o
  caminho completo: o programa roda em ring 3, pede ao kernel que escreva
  na tela com a syscall `write` e encerra com a syscall `exit`, e o prompt
  volta a responder. Desde o Marco 6 ele usa a biblioteca de runtime
  (`println!`); a instrução `syscall` crua mora em `runtime/src/sys.rs`.
- **`crash`** (`run crash`): executa uma instrução inválida de propósito.
  O kernel mostra `[run] crash encerrado por erro: #UD (Invalid Opcode) em
  <endereço>` e o prompt continua funcionando (inclusive `run hello` logo
  depois). Compare com `falha opcode`, que provoca o mesmo erro **dentro
  do kernel** e para tudo: um erro em programa nunca derruba o kernel.
- **`eco`** (`run eco`): pede um texto (`digite algo: `), espera você
  digitar e apertar Enter (o texto aparece enquanto você digita, e o
  Backspace apaga), e responde `voce digitou: <texto>` e `palavras: <n>`.
  Demonstra a leitura do teclado por syscall (`SYS_READ_LINE`) e a memória
  dinâmica dentro de um programa (a contagem de palavras usa um `Vec`, pelo
  alocador da biblioteca, que pede memória com `SYS_ALLOC`). Enquanto ele
  espera, o teclado é dele; quando termina, volta ao prompt.
- **`falha_memoria`** (`run falha_memoria`): escreve num endereço que não é
  dele (`0xdeadbeef`). O kernel mostra `[run] falha_memoria encerrado por
  erro de memoria: #PF (Page Fault) em <endereço>`, o código de erro e o
  `endereco de falha`, encerra só o programa, e o prompt continua
  funcionando (`run hello` e `run eco` logo depois).
- **`ping`** e **`pong`** (`run ping pong`): cada um escreve quatro linhas
  (`ping 1`, `pong 1`, ...) e **cede a CPU** (`yield_now`, a syscall
  `SYS_YIELD`) ao outro depois de cada linha, então as linhas aparecem
  alternadas. Demonstram a troca de contexto cooperativa.
- **`contador_a`** e **`contador_b`** (`run contador_a contador_b`): contam em
  um laço longo e escrevem oito linhas cada (`A: 1`, `B: 1`, ...) **sem nunca
  ceder a CPU**. As linhas dos dois aparecem intercaladas mesmo assim, porque o
  timer interrompe cada programa depois de uma fatia de tempo. Demonstram a
  preempção.
- **`eco2`** (`run eco eco2`): o irmão do `eco`, com o prefixo `eco2:` na
  resposta. Digite uma linha e Enter, e depois outra: a primeira vai ao `eco`,
  que pediu primeiro, e a segunda ao `eco2`.

- **`leitor`** (`run leitor`): abre `/disco/docs/longo.txt`, um arquivo que
  ocupa mais de um cluster do disco, e escreve o conteúdo na tela, lendo em
  pedaços de 128 bytes pelas syscalls de arquivo. O programa nunca fala com o
  disco: só pede ao kernel. `run leitor leitor` roda dois leitores ao mesmo
  tempo, cada um com a sua posição de leitura.
- **`listador`** (`run listador`): lista a raiz do disco, uma entrada por linha
  (`dir 0 bin`, `dir 0 docs`, `arquivo <tamanho> leiame.txt`).
- **`hora`** (`run hora`): pede a data e a hora ao kernel (syscall `SYS_TIME`,
  por `time::now()`) e as escreve como `AAAA-MM-DD HH:MM:SS UTC`, o mesmo
  formato do comando `data`; se o relógio estiver inválido, escreve `hora:
  relogio invalido` e sai com o código 1.
- **`visita`** (`run /disco/bin/visita`): escreve `visita: fui carregado do
  disco!`. Ele existe **só no disco**: o fonte fica em `programs/src/disco/`,
  fora de `programs/src/bin/`, então não entra na lista de `run` sem argumento
  nem na imagem do kernel.

### Arquivos: os volumes `/ram` e `/disco`

O conteúdo dos dois volumes vem do diretório `discos/` do repositório
(`discos/ram/` e `discos/disco/`), transformado em imagem FAT16 durante o
`cargo run`/`cargo test`; o disco ainda recebe o executável `visita` e um
arquivo de 65 537 bytes (`docs/grande.bin`) que existe só para provar a recusa
de um executável grande demais. Um caminho é `/<volume>/<componente>/...`, com
nomes 8.3 (até 8 caracteres, ponto e até 3 de extensão), sem diferenciar
maiúsculas de minúsculas, no máximo 64 bytes e 8 níveis; `.` e `..` não são
aceitos. Cada programa pode ter 4 arquivos abertos.

Demonstração:

```text
os-rust> ls /ram
os-rust> cat /ram/ola.txt
os-rust> ls /disco
os-rust> run /disco/bin/visita
os-rust> run leitor
os-rust> run listador
os-rust> run /disco/bin/visita hello
```

Para ver o kernel sem o disco, rode o QEMU à mão sem o segundo `-drive`
(`bootimage run`... ou `qemu-system-x86_64 -drive format=raw,file=<imagem do
bootimage>`): o boot é normal, `ls /ram` funciona e `ls /disco` diz `volume
indisponivel: /disco (sem disco)`. Limitações: só leitura (nada é criado,
alterado ou apagado), só nomes curtos 8.3 (entradas de nome longo do FAT são
ignoradas), um volume FAT16 por fonte, sem cache de blocos nem partições.

Para ver a multitarefa à mão, rode `cargo run` e digite `run ping pong`,
`run contador_a contador_b`, `run eco eco2` e `run falha_memoria contador_a`
(a mensagem de erro aparece na hora e o contador completa a saída).

Como um programa pede serviços ao kernel (números das syscalls,
registradores, códigos de erro, onde ele é carregado) está em
[`SYSCALLS.md`](SYSCALLS.md). Para escrever o seu próprio programa, veja o
[`GUIA_DO_PROGRAMADOR.md`](GUIA_DO_PROGRAMADOR.md).

Backspace apaga o último caractere digitado; Enter executa a linha. Um
comando não reconhecido mostra uma mensagem de erro sugerindo `help`.

Para encerrar a demonstração, digite `panic` ou simplesmente feche a
janela do QEMU. Ambos são um fim normal da execução, não um erro.

## Rodando os testes automatizados

Com os mesmos três pré-requisitos da seção anterior instalados, rode:

```sh
cargo test
```

Isso compila o kernel em um modo especial de teste, dá boot no QEMU
**sem abrir nenhuma janela** (funciona igual em uma sessão sem tela
gráfica, como um terminal remoto por SSH) e executa automaticamente
todos os testes do kernel. Ao final, o próprio QEMU se encerra sozinho —
não é preciso fechar nada manualmente.

O comando compila e roda vários binários de teste em sequência (a
biblioteca do kernel, o binário de produção e cada arquivo dentro de
`tests/`); para cada um deles, o terminal mostra uma linha `Running <N>
tests` com a quantidade total de testes daquele binário, seguida de uma
linha por teste terminando em `[ok]` quando ele passa. É normal ver essa
sequência se repetir várias vezes numa única chamada de `cargo test` —
cada binário reinicia o kernel do zero, então cada um tem sua própria
contagem.

Se um teste falha (uma verificação que deveria ser verdadeira não é,
por exemplo), o terminal mostra o nome do teste, a palavra que indica
falha, o arquivo e a linha onde a verificação falhou, e a mensagem da
falha; o QEMU daquele binário encerra imediatamente, sem rodar os testes
restantes daquele binário.

O resultado de tudo chega até você pelo **código de saída** do próprio
comando `cargo test`, do mesmo jeito que qualquer outro comando de
terminal: depois de rodar `cargo test`, digite

```sh
echo $?
```

Um `0` significa que todos os testes de todos os binários passaram. Um
número diferente de zero significa que pelo menos um teste falhou, travou
(um teste que nunca termina é interrompido depois de um tempo máximo) ou
provocou uma exceção da CPU sem tratador. Isso é útil para automatizar
verificações: um script pode rodar `cargo test` e decidir o que fazer só
olhando esse código de saída, sem precisar interpretar o texto.

Depois de rodar `cargo test`, `cargo run` continua funcionando
normalmente — nenhum código ou dispositivo exclusivo de teste faz parte
da imagem usada pela execução normal do kernel.

### O que os testes do Marco 5 cobrem

Para conferir o modo usuário à mão, rode `cargo run` e digite `run hello`
(mensagem na tela e prompt de volta) e `run crash` (mensagem de erro e
prompt de volta). A suíte automatizada (`cargo test`) cobre, além dos
testes anteriores:

- `tests/user_mode.rs` (roda programas em ring 3 dentro do QEMU): o `hello`
  escreve a mensagem e devolve o controle (transição para ring 3, `write`,
  `exit`); um nome inexistente lista os programas; a região de memória do
  usuário fica livre depois de cada programa, inclusive rodando o `hello`
  50 vezes seguidas; `write` com ponteiro do kernel ou tamanho grande demais
  devolve erro e o programa continua; um ELF inválido é recusado sem mapear
  nada; e cada falha de programa (`crash`, instrução privilegiada, divisão
  por zero, acesso a memória do kernel, pilha inválida, `int` sem gate,
  syscall inexistente) encerra só o programa, e o `hello` roda de novo
  depois.
- `src/elf.rs`: uma verificação para cada regra do leitor de ELF (magia,
  classe, tipo, máquina, tamanhos, alinhamento, região, sobreposição,
  ponto de entrada, número de segmentos).
- `src/syscall.rs`: `SYSCALLS.md` concorda com as constantes do código
  (números das syscalls, códigos de erro, região do usuário, versão).
- `src/shell.rs`: `run` sem argumento e `run` com nome desconhecido listam
  os programas. `src/vga_buffer.rs`: `write_bytes` troca bytes fora do
  ASCII pelo quadrado `0xfe`.

### O que os testes do Marco 6 cobrem

Para conferir à mão, rode `cargo run` e digite `run eco` (digite um texto e
Enter: ele volta na tela) e `run falha_memoria` (mensagem de erro de
memória, prompt de volta; `run hello` e `run eco` funcionam em seguida). A
suíte automatizada (`cargo test`) cobre, além dos testes anteriores:

- `tests/user_runtime.rs` (programas em ring 3 dentro do QEMU; os testes
  "digitam" empurrando scancodes na fila do teclado): `eco` devolve
  exatamente o texto digitado (com Shift e Backspace, e linha vazia) e
  conta as palavras com um `Vec`; a syscall de memória devolve o início do
  heap, áreas contíguas arredondadas em páginas, memória gravável e zerada,
  `ERR_INVAL` para tamanho zero e `ERR_NOMEM` acima de 1 MiB, e devolve os
  frames no fim (150 execuções de 1 MiB sem esgotar a memória); uma escrita
  além do fim do heap é `#PF`; `SYS_READ_LINE` com ponteiro do kernel, página
  somente leitura ou tamanho grande demais devolve erro sem esperar tecla;
  `falha_memoria` termina em `#PF` no endereço `0xdeadbeef` sem derrubar o
  kernel (o próximo programa roda, inclusive 40 falhas seguidas); o prompt
  volta a ler o teclado depois de um programa, e teclas digitadas com
  antecedência chegam a ele intactas; o `GUIA_DO_PROGRAMADOR.md` contém,
  literalmente, o código de `eco` e de `falha_memoria`; os nomes dos
  programas são únicos.
- `src/keyboard.rs`: `read_line` devolve exatamente o que foi digitado, com
  Shift, Backspace, limite de caracteres, teclas ignoradas, e volta com as
  interrupções desligadas. `src/syscall.rs`: `SYSCALLS.md` (versão 2)
  concorda com as constantes (números 3 e 4, `ERR_NOMEM`, janela do heap,
  limites de linha e de heap). `src/elf.rs`: um segmento que invade o heap é
  recusado. `src/gdt.rs`: o topo da pilha de entrada do kernel é múltiplo de
  16. `src/shell.rs`: `run` lista `eco` e `falha_memoria`, e `#PF` de
  programa mostra `erro de memoria`.

### O que os testes do Marco 9 cobrem

Para conferir à mão, rode `cargo run`, digite `data`, espere alguns segundos e
digite `data` de novo (a segunda hora é posterior; compare com `date -u` no
computador), depois `run hora` (a mesma hora, com diferença de segundos).
A suíte automatizada (`cargo test`) cobre, além dos testes anteriores, sem
depender da hora real:

- `src/rtc.rs`: a conversão sobre registradores fabricados: BCD e binário, 12 e
  24 horas (as quatro combinações dão o mesmo instante), meia-noite e
  meio-dia, campos impossíveis, dia contra o mês e o ano bissexto, a regra do
  século, e a repetição da leitura quando o chip está atualizando (incluindo a
  desistência depois de 5 tentativas);
- `src/shell.rs`: o comando `data` (formato, mensagens de erro, argumentos
  extras ignorados) e a presença em `help`;
- `src/syscall.rs`: o contrato versão 5 bate com o código (número, erro,
  layout de 8 bytes);
- `tests/relogio.rs` (dentro do QEMU): leitura em faixa plausível e sem andar
  para trás, `SYS_TIME` com buffer válido, tamanho errado e ponteiro inválido,
  `run hora`, dois `hora` ao mesmo tempo, e os programas dos marcos anteriores;
- `tests/user_runtime.rs`: o código de `hora` no guia é idêntico ao do repositório.

### O que os testes do Marco 8 cobrem

Para conferir à mão, rode `cargo run` e a demonstração da seção **Arquivos**
acima. A suíte automatizada (`cargo test`) cobre, além dos testes anteriores:

- `src/fat.rs` (o leitor de FAT, sobre imagens fabricadas em memória pela
  crate `fatimg` e adulteradas byte a byte): setor de boot válido e cada tipo
  de inválido; raiz e subdiretório; nomes 8.3 sem diferenciar caixa; arquivo
  vazio, de um cluster, de vários e de tamanho que não é múltiplo do cluster;
  leitura em pedaços; cadeia com ciclo, que sai do volume, que cai em cluster
  livre ou reservado, mais curta que o tamanho do arquivo; diretório com ciclo;
  entradas de nome longo, apagadas e de rótulo ignoradas; caminho inexistente,
  tipo errado e erro do dispositivo. `src/fs.rs`: validação de caminhos
  (limites, `.` e `..`, caixa), leitura do ramdisk, tabela de arquivos (limite,
  descritor inválido, posições independentes, `Drop`). `src/ata.rs`: a espera
  com limite exato, sem relógio. `src/blockdev.rs` e `src/shell.rs` (`ls` e
  `cat` com cada erro).
- `tests/disco_ata.rs` (dentro do QEMU, com os discos extras do `Cargo.toml`):
  setor de boot e arquivos conhecidos lidos do disco por PIO; drive ausente
  detectado sem travar; volume corrompido recusado sem pânico; o mesmo leitor de
  FAT sobre o ramdisk e sobre o disco.
- `tests/sistema_de_arquivos.rs`: `ls`, `cat` e `run` pelo prompt (disco e
  ramdisk), recusa de arquivo que não é ELF e de executável grande demais, recusa
  do comando inteiro com um alvo inválido, mistura de embutido e caminho, 100
  execuções de um programa do disco sem vazar frames; cada erro das syscalls de
  arquivo (ELFs montados à mão), o limite de 4 arquivos e a liberação ao
  terminar por `exit` e por erro; `leitor`, `listador` e dois leitores ao mesmo
  tempo; sem disco, volume inválido e FAT corrompida (mensagem clara, sem pânico
  nem laço infinito).
- `tests/user_runtime.rs`: o `GUIA_DO_PROGRAMADOR.md` contém, literalmente, o
  código de `leitor` e `listador`; `visita` não está entre os programas
  embutidos. `src/syscall.rs`: `SYSCALLS.md` (versão 4) concorda com as
  constantes (syscalls, erros, limites, volumes).
- `tests/artefatos.rs` continua conferindo os arquivos entregues, incluindo os
  novos (`fatimg/`, `discos/`, os módulos do kernel e os testes).

### O que os testes do Marco 7 cobrem

Para conferir à mão, rode `cargo run` e digite `run ping pong`, `run contador_a
contador_b`, `run falha_memoria contador_a` e `run eco eco2`. A suíte
automatizada (`cargo test`) cobre, além dos testes anteriores:

- `tests/multitarefa.rs` (vários programas em ring 3 ao mesmo tempo, dentro do
  QEMU; os programas de apoio são ELFs montados à mão):
  - **cooperativo**: a troca de contexto preserva registradores, pilha e
    memória de dois programas; `ping` e `pong` alternam a saída; `SYS_YIELD`
    sem outro programa pronto volta na hora; o que termina antes não impede o
    outro; programas simultâneos não enxergam a memória um do outro;
  - **`run`**: um programa só é igual ao Marco 6; nome desconhecido e mais de 4
    programas recusam o pedido inteiro sem iniciar nenhum (e sem vazar frames);
    o mesmo nome duas vezes são instâncias independentes; 100 execuções de dois
    programas devolvem todos os frames; o prompt volta a ler o teclado;
  - **preemptivo**: `contador_a` e `contador_b` (que não cedem a CPU) se
    intercalam por causa do timer; um programa curto termina antes de um longo;
    o tick em ring 0 não troca de contexto; o fim de interrupção nunca fica
    pendente; cada `write` sai inteiro mesmo sob preempção;
  - **falhas**: uma falha encerra só quem falhou, com a mensagem na hora, e os
    frames dele voltam enquanto os outros seguem vivos; `exit` de um programa
    não encerra os outros;
  - **teclado**: a linha vai ao programa que pediu primeiro, sem perda nem
    duplicação; uma linha pela metade é de quem pediu primeiro; programas
    bloqueados no teclado não consomem CPU (o kernel dorme, e cada volta
    ociosa vem de uma interrupção); uma tecla digitada enquanto outro programa
    computa acorda quem espera.
- `tests/artefatos.rs`: nenhum arquivo entregue do projeto cita a ferramenta
  usada para escrever as especificações, nem aponta para as pastas de
  especificação (a lista de arquivos é embutida pelo `build.rs`).
- `tests/user_runtime.rs`: o `GUIA_DO_PROGRAMADOR.md` contém, literalmente, o
  código de `ping`, `pong`, `contador_a`, `contador_b` e `eco2`; os nomes dos
  programas são únicos.
- `src/syscall.rs`: `SYSCALLS.md` (versão 3) concorda com as constantes
  (`SYS_YIELD`, máximo de programas, fatia e frequência do timer).
  `src/memory.rs`: um espaço de endereçamento novo e destruído devolve todos os
  frames, e uma página de um espaço não aparece nos outros. `src/task.rs`: o
  layout do contexto é o que o assembly assume. `src/timer.rs`: o divisor do
  PIT dá 100 Hz.

Para entender por dentro como esse mecanismo funciona (a porta serial, o
executor de testes sem biblioteca padrão, e como o resultado viaja do
kernel até o código de saída do `cargo test`), veja o capítulo
correspondente no [`WALKTHROUGH.md`](./WALKTHROUGH.md).

## Estrutura do projeto

- `src/lib.rs`: declara os módulos do kernel, expõe o nome (`NAME`) e a
  identificação de versão (`VERSION`, os dois derivados de `Cargo.toml`
  em tempo de compilação) e a mensagem de boas-vindas (`print_welcome`),
  inicializa a porta serial, a GDT/TSS, as interrupções, a memória
  (frames, paginação, heap), os volumes de arquivos, o timer e o mecanismo de syscall, e contém a infraestrutura de testes (o
  executor de testes, o tratamento de panic em modo de teste e a
  comunicação com o QEMU sobre sucesso ou falha).
- `src/main.rs`: o binário de produção — ponto de entrada do boot; limpa
  a tela, escreve a mensagem de boas-vindas, desenha o logo e roda o laço
  ocioso que alimenta o prompt de comandos com o teclado.
- `src/logo.rs` / `src/logo.txt`: o texto canônico do logo ASCII (20
  linhas), incluído em tempo de compilação sem nenhum escape de Rust.
- `src/vga_buffer.rs`: toda a lógica de escrita de texto no buffer de
  vídeo VGA (`0xb8000`): cores, avanço de linha, rolagem, backspace,
  cursor de hardware, e o desenho do logo (`draw_logo`) direto nas linhas
  0-19, traduzindo `█` para o byte `0xDB` da code page 437.
- `src/serial.rs`: escrita de texto na porta serial (UART 16550), usada
  para diagnóstico durante a execução normal e para reportar resultados
  de teste.
- `src/panic.rs`: o que acontece quando o sistema encontra um erro
  irrecuperável (panic); mostra uma mensagem legível na tela (e também na
  serial) em vez de travar ou reiniciar sem explicação. A guarda de
  reentrância que evita travar reescrevendo a tela é compartilhada com as
  telas de exceção fatal de `interrupts.rs`.
- `src/gdt.rs`: a GDT (segmentos de código e dados do kernel, segmentos de
  código e dados do usuário, descritor da TSS) e a TSS, com uma pilha
  dedicada de 20 KiB na Interrupt Stack Table para o double fault e outra
  para as entradas no kernel vindas de ring 3.
- `src/interrupts.rs`: a IDT, os handlers das exceções (`#BP`, `#DE`,
  `#UD`, `#GP`, `#SS`, `#NP`, `#PF`, `#DF`, este último rodando na pilha
  dedicada da TSS/IST) e a reprogramação do PIC 8259 para o teclado
  (IRQ0 e IRQ1). As exceções fatais compartilham uma única função que monta a
  tela de exceção (tela + serial); quando a exceção vem de um programa de
  usuário (ring 3), o handler encerra só o programa.
- `src/keyboard.rs`: tradução de scancodes (Scan Code Set 1) para ASCII,
  layout US QWERTY, o montador de linha (`LineEditor`, com eco e Backspace)
  que o escalonador usa para a linha de quem pediu primeiro, e `read_line`,
  a leitura bloqueante de um leitor só.
- `src/memory.rs`: o alocador de frames físicos a partir do mapa de
  memória do bootloader (global, com reciclagem de frames), e a
  tradução/criação de mapeamentos na tabela de páginas ativa (usando o
  mapeamento completo da física), inclusive as páginas dos programas de
  usuário, e o espaço de endereçamento de cada programa (`AddressSpace`:
  tabela P4 e P3 próprias, criadas, ativadas e destruídas).
- `src/elf.rs`: o leitor de executáveis ELF64 estáticos (escrito à mão) e
  as regras de validação.
- `src/user.rs`: o layout de memória do usuário (código e dados, heap e
  pilha), o carregador (um espaço de endereçamento por programa), o heap do
  programa (`grow_heap`), `run_images`/`run_all` (vários programas ao mesmo
  tempo) e a entrada e saída de ring 3 (`enter_user`, `resume_task`,
  `leave_user`).
- `src/task.rs`: o que o kernel guarda de cada programa carregado: o estado
  dele em ring 3 (`TaskContext`), em que ponto da vida está e o pedido de
  teclado em andamento.
- `src/scheduler.rs`: o escalonador em rodízio: escolhe a próxima tarefa, troca
  de contexto (por `SYS_YIELD`, pelo timer ou por espera de teclado), entrega a
  linha digitada a quem pediu primeiro, dorme com `hlt` quando todos esperam, e
  devolve os recursos de quem termina.
- `src/timer.rs`: o PIT (100 Hz, IRQ0) e o stub da interrupção do timer, que só
  troca de tarefa quando interrompe ring 3.
- `src/blockdev.rs`: o dispositivo de blocos (ler um setor de 512 bytes por
  endereço de bloco), implementado pelo ramdisk (`RamDisk`) e pelo disco ATA.
- `src/fat.rs`: o leitor de FAT16 somente leitura, que trata o volume como dado
  não confiável (cadeias com limite de passos, números de cluster validados).
- `src/ata.rs`: o driver de disco ATA por PIO e polling, com limite de espera e
  detecção de ausência, atômico em relação ao escalonador.
- `src/rtc.rs`: o driver do relógio de tempo real (portas `0x70`/`0x71`, BCD e
  binário, 12 e 24 horas, janela de atualização, século), por polling.
- `src/fs.rs`: os volumes `/ram` e `/disco`, os caminhos e a tabela de arquivos
  abertos de cada programa.
- `src/syscall.rs`: o mecanismo `syscall` (entrada pela instrução, retorno por
  `iretq`), o despachante e as syscalls `write`, `exit`, `read_line`, `alloc`,
  `yield`, `open`, `read`, `close`, `read_dir` e `time`.
- `src/programs.rs`: a tabela dos programas embutidos, gerada pelo
  `build.rs`.
- `src/allocator.rs`: a faixa fixa de endereços virtuais do heap, o
  alocador global (`Box`, `Vec`, `String`, ...) e o tratamento de heap
  esgotado.
- `src/shell.rs`: o buffer de linha e o prompt de comandos (`help`,
  `clear`, `echo`, `sobre`, `panic`, `mem`, `falha <tipo>`, `ls`, `cat`, `data`, `run <alvo>`).
- `abi/`: as constantes do contrato de syscalls (números, erros, limites),
  compartilhadas pelo kernel e pela biblioteca de runtime.
- `runtime/`: a biblioteca de runtime dos programas de usuário (`entry!`,
  `print!`/`println!`, `read_line`, `yield_now`, `File`/`Dir` para ler arquivos, `time::now()`,
  alocador global, tratador de `panic`).
- `programs/`: a crate dos programas de usuário (`hello`, `crash`, `eco`,
  `falha_memoria`, `ping`, `pong`, `contador_a`, `contador_b`, `eco2`, `leitor`,
  `listador`, `hora`, um arquivo por programa em `src/bin/`, mais `visita` em
  `src/disco/`, que só vai para o disco), com o linker
  script (`link.ld`); compilada para o target de usuário pelo `build.rs` da
  raiz, nunca diretamente.
- `GUIA_DO_PROGRAMADOR.md`: o guia de quem escreve programas para o os-rust.
- `fatimg/`: o gerador de imagens FAT16 (`no_std`, sem dependências), usado
  pelo `build.rs` para gerar o ramdisk e o disco e pelos testes para fabricar e
  adulterar imagens.
- `discos/`: o conteúdo dos volumes (`discos/ram/` e `discos/disco/`).
- `build.rs`: compila `programs/` (com um `cargo` aninhado), embute os ELFs no
  kernel, gera as imagens do ramdisk (embutida no kernel) e do disco
  (`target/imagens/`, anexada ao QEMU) e a lista de arquivos entregues que
  `tests/artefatos.rs` confere.
- `SYSCALLS.md`: o contrato de syscalls (versão 4).
- `tests/`: os testes de integração, cada um iniciando o kernel do zero
  em seu próprio binário — um teste de boot (que também confere a versão
  na mensagem de boas-vindas), um teste cujo resultado esperado é um
  panic, testes do alocador de frames, da paginação e do heap, e dois
  testes dedicados de proteção: `double_fault.rs` (prova que o handler
  roda na pilha dedicada da IST) e `page_fault.rs` (prova o endereço de
  falha esperado), `user_mode.rs` (roda programas de usuário em ring 3),
  `user_runtime.rs` (leitura de teclado, memória, isolamento e o guia),
  `multitarefa.rs` (vários programas ao mesmo tempo: troca de contexto,
  preempção, isolamento e teclado), `disco_ata.rs` (o driver ATA),
  `sistema_de_arquivos.rs` (`ls`, `cat`, `run` por caminho, syscalls de arquivo
  e volumes inválidos) e `artefatos.rs` (higiene dos arquivos entregues).
- `x86_64-os_rust.json`: a especificação do target bare-metal customizado
  (sem sistema operacional por baixo).
- `x86_64-os_rust_user.json`: a especificação do target dos programas de
  usuário (ring 3), distinto do target do kernel.
- `.cargo/config.toml`: configura o `cargo run`/`cargo test` para usar o
  `bootimage` como *runner* automaticamente, e habilita a compilação da
  crate `alloc` para este target customizado.
- `CHANGELOG.md`: o histórico de mudanças do projeto, versão por versão.
