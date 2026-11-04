# proto-os

![](imagem.jpg)

Uma demonstração mínima de "programação sem sistema operacional" em Rust:
um binário que dá boot direto via BIOS em uma máquina virtual QEMU, escreve
texto na tela usando o buffer de vídeo VGA e lê o teclado via interrupção de
hardware (IRQ1) para alimentar um prompt de comandos mínimo — sem nenhum
sistema operacional por baixo.

Se você nunca viu como um kernel/bootloader funciona, leia também o
[`WALKTHROUGH.md`](./WALKTHROUGH.md) — ele explica o que está acontecendo
por trás do boot e do código.

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
arquivo `rust-toolchain.toml` na raiz do repositório — você **não** precisa
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
formato de *target spec* JSON usado neste projeto é instável e muda de vez
em quando entre versões do nightly — uma data fixa garante que `cargo run`
funcione igual em qualquer máquina, hoje e daqui a um ano.

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
projeto (`x86_64-proto_os.json`), gerar uma imagem de boot com `bootimage`,
e abrir uma janela do QEMU que dá boot via BIOS direto nesse binário. Em
poucos segundos você deve ver uma tela de texto colorida com uma mensagem
de boas-vindas, seguida de um prompt `proto-os> ` pronto para digitação —
não um terminal comum.

Clique na janela do QEMU para garantir que ela tem o foco do teclado e
digite um comando. O layout de teclado suportado é **US QWERTY, somente
ASCII** — não há suporte a acentuação, ABNT2 ou outros layouts.

### Comandos disponíveis

| Comando | O que faz |
|---|---|
| `help` | Lista os comandos disponíveis |
| `clear` | Limpa a tela e reposiciona o prompt no topo |
| `echo <texto>` | Escreve `<texto>` na linha seguinte |
| `sobre` | Mostra uma descrição curta do proto-os |
| `panic` | Dispara um panic proposital (mesma tela de erro do tratamento de panic) |

Backspace apaga o último caractere digitado; Enter executa a linha. Um
comando não reconhecido mostra uma mensagem de erro sugerindo `help`.

Para encerrar a demonstração, digite `panic` ou simplesmente feche a
janela do QEMU. Ambos são um fim normal da execução, não um erro.

## Estrutura do projeto

- `src/main.rs` — ponto de entrada do boot; limpa a tela, escreve a
  mensagem de boas-vindas, inicializa interrupções e roda o laço ocioso que
  alimenta o prompt de comandos com o teclado.
- `src/vga_buffer.rs` — toda a lógica de escrita de texto no buffer de
  vídeo VGA (`0xb8000`): cores, avanço de linha, rolagem, backspace e
  cursor de hardware.
- `src/panic.rs` — o que acontece quando o sistema encontra um erro
  irrecuperável (panic): mostra uma mensagem legível na tela em vez de
  travar ou reiniciar sem explicação.
- `src/interrupts.rs` — a IDT, os handlers de breakpoint/double
  fault/teclado (IRQ1) e a reprogramação do PIC 8259.
- `src/keyboard.rs` — tradução de scancodes (Scan Code Set 1) para ASCII,
  layout US QWERTY.
- `src/shell.rs` — o buffer de linha e o prompt de comandos (`help`,
  `clear`, `echo`, `sobre`, `panic`).
- `x86_64-proto_os.json` — a especificação do target bare-metal customizado
  (sem sistema operacional por baixo).
- `.cargo/config.toml` — configura o `cargo run` para usar o `bootimage`
  como *runner* automaticamente.

## Escopo desta versão

Este projeto é uma demonstração didática, não um kernel de verdade. Cobre:
boot via BIOS (não UEFI); saída de texto em modo VGA com mensagem e cores
fixas no código; e leitura de teclado via IRQ1 alimentando um prompt de
comandos fechado (US QWERTY, somente ASCII). Não há sistema de arquivos,
rede, multitarefa, histórico de comandos, outros layouts de teclado ou
suporte a hardware físico, mas apenas QEMU. 

Próximos passos para um kernel de verdade
O que está aqui é o primeiro degrau, não o kernel. Para sair de "escreve texto na tela" e chegar em algo que mereça o nome de sistema operacional, faltam camadas inteiras, cada uma bem mais trabalhosa que esta demo. Em ordem aproximada de dificuldade:

Gerência de memória de verdade. O bootloader já configura uma paginação mínima para o binário rodar, mas um kernel de verdade precisa de um alocador de frames físicos e um alocador de heap (GlobalAlloc) para poder usar Vec, Box e afins dentro do próprio kernel.

Multitarefa. Depois de ter heap, dá para pensar em um scheduler. A versão mais simples é cooperativa (cada tarefa cede o controle voluntariamente); a versão de verdade precisa de troca de contexto via interrupção de timer (PIT ou APIC timer) e um scheduler preemptivo.

Modo usuário (user space). Hoje tudo roda em modo privilegiado (ring 0). Um kernel de verdade separa kernel de aplicação: ring 3, chamadas de sistema (syscalls) via interrupção de software ou syscall/sysret, e isolamento de memória entre processos via paginação.

Sistema de arquivos. Começa com algo simples, como um driver de disco ATA/AHCI e um sistema de arquivos read-only tipo FAT ou até um formato próprio, antes de pensar em algo como ext.

Drivers. Cada periférico a mais (disco, rede, som) é um driver novo, geralmente a parte mais entediante e mais cheia de detalhes de hardware de qualquer kernel.

Cada um desses itens dá, sozinho, para um módulo de curso inteiro. Eu vou começar a construir passo a passo, sempre que tiver um intervalo. Se quiser acompanhar, seja bem vindo ou bem vinda. 
