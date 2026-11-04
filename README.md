# proto-os

Uma demonstração mínima de "programação sem sistema operacional" em Rust:
um binário que dá boot direto via BIOS em uma máquina virtual QEMU e escreve
texto na tela usando o buffer de vídeo VGA, sem nenhum sistema operacional
por baixo.

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
de boas-vindas — não um terminal comum.

Para encerrar a demonstração, feche a janela do QEMU. Isso é o fim normal
da execução, não um erro.

## Estrutura do projeto

- `src/main.rs` — ponto de entrada do boot; limpa a tela, escreve a
  mensagem de boas-vindas e registra o tratamento de panic.
- `src/vga_buffer.rs` — toda a lógica de escrita de texto no buffer de
  vídeo VGA (`0xb8000`): cores, avanço de linha, rolagem.
- `src/panic.rs` — o que acontece quando o sistema encontra um erro
  irrecuperável (panic): mostra uma mensagem legível na tela em vez de
  travar ou reiniciar sem explicação.
- `x86_64-proto_os.json` — a especificação do target bare-metal customizado
  (sem sistema operacional por baixo).
- `.cargo/config.toml` — configura o `cargo run` para usar o `bootimage`
  como *runner* automaticamente.

## Escopo desta versão

Este projeto é uma demonstração didática, não um kernel de verdade. Nesta
versão: boot via BIOS (não UEFI), saída de texto em modo VGA com mensagem e
cores fixas no código. Não há leitura de teclado, sistema de arquivos,
rede, multitarefa ou suporte a hardware físico — apenas QEMU. Veja
`.specify/memory/constitution.md` para os princípios completos do projeto.
