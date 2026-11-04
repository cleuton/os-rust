# Implementation Plan: Demonstração de Boot Bare Metal em VGA (proto-os)

**Branch**: `001-vga-boot-demo` | **Date**: 2026-09-22 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/001-vga-boot-demo/spec.md`

## Summary

Um binário Rust `no_std`/`no_main` que dá boot via BIOS em QEMU x86_64 (sem sistema
operacional embaixo), limpa a tela, escreve uma mensagem de boas-vindas fixa no
buffer de texto VGA (`0xb8000`) com paleta de cores fixa, avança linha/cursor com
rolagem simples, e trata panics mostrando uma mensagem legível na tela em vez de
travar silenciosamente. Tudo sobe com um único `cargo run`, documentado em
`README.md`, com `WALKTHROUGH.md` explicando o boot para quem nunca viu sistemas
operacionais. Abordagem técnica: crate `bootloader` 0.9.x + ferramenta `bootimage`
como *runner* do Cargo — é o caminho com menos peças móveis para satisfazer
"reprodutibilidade com um único comando" (FR-006) e "simplicidade sobre
generalidade" (Princípio III da constituição), em vez da API mais nova
(`bootloader` 0.11+ com `BiosBoot`/`build.rs`/artifact-dependencies), que exige
mais boilerplate e duas crates separadas.

## Technical Context

**Language/Version**: Rust nightly, pinado em uma **data exata** via
`rust-toolchain.toml` (`nightly-2026-08-01`, componentes `rust-src` +
`llvm-tools-preview`) — não um canal flutuante. Nightly é exigido porque o
kernel usa `#![no_std]`/`#![no_main]`, um *custom target* `x86_64` e
`build-std`, que ainda não são estáveis; o pin em uma data exata (em vez de
`channel = "nightly"`) foi validado como necessário na prática durante
`/speckit-implement` — o formato do *target spec* JSON é instável e mudou
de forma incompatível entre nightlies testados nesta sessão (ver
`research.md` Decisão 1.1). Confirma a restrição técnica já fixada na
constituição do projeto.

**Primary Dependencies**: crate `bootloader` (`0.9.23`, resolve para
`0.9.35`, BIOS) + ferramenta `bootimage` **pinada em `0.10.5`**
(`cargo install bootimage --version 0.10.5 --locked`) como *runner* de
`cargo run`; `volatile` para escrita seguindo semântica de memória mapeada
no buffer VGA; `spin` para um `Mutex` sem sistema operacional (sem
`std::sync`); `lazy_static` (feature `spin_no_std`) para inicializar o
`Writer` global sem alocador. A instrução de parada da CPU após um panic
(`hlt`) é emitida via `core::arch::asm!` diretamente (bloco `unsafe`
comentado) — **não** é adicionada a crate `x86_64` como dependência só para
essa instrução única (ver `research.md` Decisão 4), mantendo a lista de
dependências acima completa e fechada. `.cargo/config.toml` também declara
`json-target-spec = true` em `[unstable]`, exigido pelo nightly pinado (ver
`research.md` Decisão 1.1).

**Nota de validação empírica**: uma migração completa para `bootloader`
0.11+ foi implementada e testada de ponta a ponta durante
`/speckit-implement`, e **revertida** — `bootloader_api::BootInfo` (0.11+)
não expõe mais um buffer de texto VGA, apenas um `framebuffer` de pixels,
o que violaria a restrição técnica fixa "sem driver gráfico" da
constituição. A decisão original por `bootloader` 0.9.x (abaixo) está
confirmada como correta por motivo funcional, não só de simplicidade — ver
`research.md` Decisão 1 para o relato completo.

**Storage**: N/A — não há persistência; todo estado vive em memória durante a
execução do QEMU.

**Testing**: Verificação manual e visual no QEMU, documentada em
`quickstart.md`, é o método de aceitação principal (alinhado ao Princípio IV da
constituição — "Testável em QEMU, sem hardware real"). Testes automatizados de
kernel (`custom_test_frameworks` rodando dentro do QEMU) ficam fora do escopo
da v1 por adicionarem complexidade incompatível com uma aula de 60 minutos;
funções puras sem dependência de hardware (ex.: cálculo de deslocamento de
linha ao rolar a tela, encoding de `ColorCode`) podem opcionalmente ganhar
`#[cfg(test)]` compilados para o host, mas isso não é obrigatório para o v1.

**Target Platform**: QEMU (`qemu-system-x86_64`), boot via BIOS. Nenhum
hardware físico é suportado ou necessário (Princípio IV / restrições técnicas
fixas da constituição).

**Project Type**: Binário Rust único, bare-metal (`no_std`/`no_main`) — não é
biblioteca, CLI, nem serviço web.

**Performance Goals**: Do `cargo run` até a mensagem visível no QEMU em até 30s
(SC-001), tipicamente poucos segundos em máquina de desenvolvimento razoável.

**Constraints**: `no_std`, sem alocador de heap (ou alocador mínimo se
estritamente necessário — não previsto nesta versão), single-threaded/fluxo
único de execução, todo bloco `unsafe` comentado explicando por que é seguro
naquele ponto (FR-009), sem `unwrap` solto em código sem rede de segurança,
volume total de código-fonte na ordem de poucas centenas de linhas (SC-006).

**Scale/Scope**: Um único binário, uma única sessão de demonstração por
execução, sem múltiplos usuários, sem múltiplos processos.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Princípio / Restrição | Avaliação | Status |
|---|---|---|
| I. Escopo mínimo, sempre | Plano cobre exatamente boot BIOS + saída VGA texto; teclado, sistema de arquivos, rede, multitarefa, driver além do essencial e heap sofisticado ficam de fora. | PASS |
| II. Sem kernel de verdade | Sem scheduler, sem processos, sem sistema de arquivos, sem user space; fluxo único, modo privilegiado. Nenhuma interrupção além do necessário para exibir texto/panic (nenhuma leitura de teclado nesta versão). | PASS |
| III. Simplicidade sobre generalidade | Escolhida a rota `bootloader` 0.9.x + `bootimage` (menos peças móveis) em vez da API mais nova e mais genérica; módulos nomeados por conceito didático (`vga_buffer`, `panic`), sem abstrações especulativas. | PASS |
| IV. Testável em QEMU, sem hardware real | Método de aceitação é execução no QEMU via `quickstart.md`; nenhuma dependência de hardware físico. | PASS |
| V. Rust idiomático em `no_std` | Tipos dedicados (`ColorCode`, `ScreenChar`, `Buffer`, `Writer`), todo `unsafe` comentado, sem `unwrap` solto fora de código de boot já protegido pelo próprio design. | PASS |
| VI. Reprodutibilidade | `cargo run` único, via `bootimage runner` configurado em `.cargo/config.toml`; README documenta pré-requisitos exatos (toolchain nightly, `bootimage`, QEMU). | PASS |
| Restrições técnicas fixas (target `x86_64` `no_std`/`no_main`, boot BIOS via `bootloader`, saída só VGA texto em `0xb8000`, sem entrada na v1, execução só via QEMU) | Todas respeitadas pelo Technical Context acima. | PASS |

Nenhuma violação identificada. `Complexity Tracking` não se aplica e foi
omitida.

**Nota sobre FR-010/FR-011**: FR-010 (nomes de módulos/funções refletindo o
conceito didático) é satisfeita pela escolha de nomes em `src/` (Project
Structure abaixo) e verificada explicitamente por uma tarefa de revisão na
fase de Polish (`tasks.md`). FR-011/SC-003 (comportamento idêntico em
qualquer máquina com os pré-requisitos, sem depender de hardware específico)
é garantida por construção: o target custom `x86_64-proto_os.json` não faz
detecção de CPU/host, não há caminho de código condicionado a hardware
específico, e a única variável de execução é o próprio QEMU — também
verificado por uma tarefa dedicada na fase de Polish.

## Project Structure

### Documentation (this feature)

```text
specs/001-vga-boot-demo/
├── plan.md              # This file (/speckit-plan command output)
├── research.md          # Phase 0 output (/speckit-plan command)
├── data-model.md        # Phase 1 output (/speckit-plan command)
├── quickstart.md        # Phase 1 output (/speckit-plan command)
├── contracts/           # Phase 1 output (/speckit-plan command)
│   └── vga-output-contract.md
└── tasks.md             # Phase 2 output (/speckit-tasks command - NOT created by /speckit-plan)
```

### Source Code (repository root)

```text
# Binário Rust bare-metal único (sem workspace, sem múltiplos crates)
.cargo/
└── config.toml           # target custom + `bootimage runner` para `cargo run`

x86_64-proto_os.json       # especificação do target custom x86_64 (no_std/no_main)
rust-toolchain.toml        # pina a toolchain nightly exigida

Cargo.toml                 # dependências: bootloader 0.9.x, volatile, spin, lazy_static

src/
├── main.rs                # entry point (`_start`/bootloader entry), orquestra boot →
│                           #   limpar tela → mensagem de boas-vindas; define
│                           #   `#[panic_handler]` delegando para `panic::handle`
├── vga_buffer.rs           # `Writer`, `ColorCode`, `ScreenChar`, `Buffer`, macros
│                           #   `print!`/`println!`, avanço de linha/cursor e rolagem
└── panic.rs                # formatação e escrita da mensagem de panic na tela;
                            #   trava de reentrância (`AtomicBool`) para que um
                            #   segundo panic dentro do handler nunca tente
                            #   reescrever a tela nem trave em deadlock —
                            #   apenas para a CPU (`hlt` via `core::arch::asm!`),
                            #   ver research.md § 4.1

README.md                  # instalação de pré-requisitos + como compilar/rodar
WALKTHROUGH.md              # explicação do boot/código para quem nunca viu SOs
```

**Structure Decision**: Projeto de binário único na raiz do repositório (sem
`src/models`, `src/services` etc. — não se aplicam a um kernel bare-metal
mínimo). Três módulos em `src/`: `main.rs` (orquestração/boot), `vga_buffer.rs`
(saída de texto VGA) e `panic.rs` (tratamento de panic), nomes escolhidos para
refletir o conceito didático de cada um (FR-010). Sem diretório `tests/`
dedicado na v1 — a validação é feita via `quickstart.md` rodando no QEMU
(Princípio IV), conforme decisão de Testing no Technical Context.

## Complexity Tracking

> Não se aplica — nenhuma violação do Constitution Check foi identificada.
