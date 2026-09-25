# Changelog

Todas as mudanças notáveis deste projeto são registradas neste arquivo,
uma versão por vez. O formato segue, livremente,
[Keep a Changelog](https://keepachangelog.com/), e as versões seguem
[Versionamento Semântico](https://semver.org/lang/pt-BR/).

## [0.4.1] - 2026-09-25

Marco 4.1: nova identidade — o projeto passa a se chamar `os-rust`
(antes `proto-os`), e ganha um logo ASCII na tela de boot e no README.
Marco de manutenção: nenhuma capacidade nova do kernel.

### Adicionado

- Logo ASCII de 20 linhas (`src/logo.txt`/`src/logo.rs`), desenhado no
  topo da tela (`vga_buffer::draw_logo`) antes da identificação e do
  prompt, em tempo de boot — sem enviar nada para a serial. Traduz cada
  caractere `█` (UTF-8) para o byte `0xDB` da code page 437 (bloco cheio
  do VGA), um caractere por bloco.
- O mesmo logo, idêntico, no topo do `README.md`.
- `pub const NAME` em `src/lib.rs`, derivado de
  `env!("CARGO_PKG_NAME")` — fonte única do nome do projeto, ao lado de
  `VERSION` (que agora deriva nome e versão, não só a versão).
- 4 testes novos: dimensões do logo (`src/logo.rs`), o logo nas 20
  primeiras linhas da tela com `draw_logo()` isolado, o logo sobrevivendo
  intacto à sequência completa de boot (identificação + prompt já
  escritos — prova que nada rola por cima dele), e a identificação
  comparada de novo contra o manifesto (`tests/boot_integration.rs`).
- Marco 4.1 na tabela de Roadmap e em "Detalhes dos marcos" do
  `README.md`, entre o Marco 4 e o Marco 5.
- Seção nova no `WALKTHROUGH.md` explicando de onde vêm nome e versão,
  por que o pacote `os-rust` vira a crate `os_rust`, por que o nome do
  arquivo JSON do target decide o nome da subpasta em `target/`, e por
  que `█` em UTF-8 não é o byte `0xDB` da code page 437.
- Cláusula nos princípios de governança do projeto, codificando a
  exceção de "marco de manutenção" (v2.0.1).

### Alterado

- Nome do pacote em `Cargo.toml`: `proto_os` → `os-rust` (a crate no
  código Rust vira `os_rust`, com underscore). Versão do projeto:
  `0.4.0` → `0.4.1`.
- Arquivo de especificação do target customizado:
  `x86_64-proto_os.json` → `x86_64-os_rust.json` (mesmo conteúdo);
  `.cargo/config.toml` atualizado.
- Pasta raiz do repositório: `proto-os` → `os-rust`.
- Prompt: `proto-os> ` → `os-rust> `, derivado de `env!("CARGO_PKG_NAME")`
  (mesma fonte de `NAME`/`VERSION`), nunca escrito à mão.
- Mensagem de boas-vindas, comando `sobre`, telas de panic e de exceção
  fatal, e a primeira linha de diagnóstico da serial: todos passam a
  citar `os-rust`, lendo `NAME`/`VERSION` em vez de texto fixo.
- `README.md`, `WALKTHROUGH.md` e as entradas anteriores deste arquivo:
  texto atualizado para `os-rust`, sem mudar nenhum conteúdo técnico.
- Todas as referências à crate em `src/main.rs` e em cada arquivo de
  `tests/`: `proto_os::` → `os_rust::`.

## [0.4.0] - 2026-09-25

Marco 4: proteção — GDT e TSS próprias, handlers para as exceções
principais, comando `falha <tipo>` e identificação de versão única.

### Adicionado

- GDT própria do kernel (`src/gdt.rs`): segmento de código do kernel e
  descritor da TSS, carregada durante o boot antes da IDT.
- TSS com uma pilha dedicada de 20 KiB na Interrupt Stack Table
  (`DOUBLE_FAULT_IST_INDEX`), usada pelo handler de double fault — um
  estouro de pilha do kernel não causa mais triple fault e reinício
  silencioso do QEMU.
- Handlers para as cinco exceções principais: breakpoint (`#BP`, já
  existente, agora com tela reduzida a uma linha), instrução inválida
  (`#UD`), proteção geral (`#GP`), page fault (`#PF`) e double fault
  (`#DF`, agora na pilha dedicada). As quatro fatais mostram uma tela
  legível (tela + serial), no estilo da tela de panic, com nome, sigla,
  endereço da instrução, código de erro (quando há um) e a versão do
  os-rust; a tela de page fault também mostra o endereço de falha e a
  interpretação do código de erro em palavras (leitura/escrita,
  página ausente/violação de proteção).
- Comando `falha <tipo>` no prompt: `pagina`, `pilha`, `opcode`,
  `protecao` e `breakpoint`, cada um provocando de propósito a exceção
  correspondente; sem argumento ou com um tipo desconhecido, lista os
  tipos disponíveis.
- Identificação de versão única (`os_rust::VERSION`, derivada de
  `Cargo.toml` em tempo de compilação), exibida na mensagem de
  boas-vindas, na primeira linha de diagnóstico da serial, no comando
  `sobre`, e em toda tela de panic ou de exceção fatal.
- Nova linha de diagnóstico na serial ("gdt/tss ativos") logo após a
  inicialização da GDT/TSS.
- 7 testes novos: 2 testes de integração dedicados (`tests/double_fault.rs`,
  provando que o handler roda na pilha dedicada da IST; `tests/page_fault.rs`,
  provando o endereço de falha esperado), e 5 testes de unidade/integração
  de versão e do comando `falha` (`src/shell.rs`, `tests/boot_integration.rs`).
- Capítulo novo no `WALKTHROUGH.md` sobre exceções de CPU, GDT, TSS e
  Interrupt Stack Table, o double fault com pilha dedicada, como ler o
  código de erro e o endereço de um page fault, e como a versão chega do
  `Cargo.toml` até a tela.

### Alterado

- Versão do projeto: `0.3.0` → `0.4.0`.
- `os_rust::init` passa a chamar `gdt::init()` entre `serial::init()` e
  `interrupts::init()`.
- A guarda de reentrância do tratamento de panic (`panic.rs`) passa a
  ser compartilhada com a tela de exceção fatal, através de
  `panic::enter_fatal_handler()`.
- `vga_buffer::screen_contains` deixa de ser exclusiva de testes internos
  (`#[cfg(test)] pub(crate)`) e passa a `pub`, para ser reaproveitada por
  testes de integração em `tests/`.
- `README.md`: Marco 4 marcado como concluído na tabela de marcos, nos
  detalhes de cada marco e na seção Status; comando `falha <tipo>` na
  tabela de comandos; estrutura do projeto atualizada.

## [0.3.0] - 2026-09-25

Marco 3: memória — alocador de frames físicos, paginação e heap do kernel.

### Adicionado

- Alocador de frames físicos de 4 KiB (`BootInfoFrameAllocator`,
  `src/memory.rs`) a partir do mapa de memória entregue pelo bootloader.
- Tradução de endereço virtual para físico e criação de mapeamentos
  novos na tabela de páginas ativa (`memory::translate_addr`,
  `memory::map_page`), usando o mapeamento completo da memória física
  (feature `map_physical_memory` do `bootloader`).
- Heap do kernel: faixa fixa de 100 KiB (`src/allocator.rs`), mapeada no
  boot, com um alocador global (`linked_list_allocator`) — `Box`, `Vec`
  e o resto da crate `alloc` passam a funcionar em qualquer parte do
  kernel.
- Comando `mem` no prompt: mostra a memória física utilizável, a
  posição/tamanho do heap, um `Box` com seu valor e endereço, e um `Vec`
  construído a partir de vazio com tamanho, capacidade e soma.
- Nova linha de diagnóstico na serial ao final da inicialização de
  memória.
- 11 testes novos: 3 testes de integração (`tests/frame_allocator.rs`,
  `tests/paging.rs`, `tests/heap_allocation.rs`) e um teste de unidade
  do comando `mem` em `src/shell.rs`.
- Capítulo novo no `WALKTHROUGH.md` sobre memória física, frames e
  páginas, a tabela de páginas de 4 níveis, criação de mapeamentos, e o
  heap.

### Alterado

- Versão do projeto: `0.2.0` → `0.3.0`.
- `os_rust::init` passa a receber o `BootInfo` entregue pelo
  bootloader (mapa de memória e deslocamento da física completa).
- `Cargo.toml`: dependência `linked_list_allocator` nova; `bootloader`
  ganha a feature `map_physical_memory`.
- `.cargo/config.toml`: `build-std` ganha `"alloc"`.
- `README.md`: Marco 3 marcado como concluído na tabela de marcos, nos
  detalhes de cada marco e na seção Status; comando `mem` na tabela de
  comandos; estrutura do projeto atualizada.

## [0.2.0] - 2026-09-24

Marco 2: infraestrutura de depuração e testes automatizados.

### Adicionado

- Saída serial (UART 16550, porta `0x3F8`): o kernel agora escreve
  mensagens de diagnóstico do boot, dos eventos de breakpoint e double
  fault, e do tratamento de panic também no terminal do host, sem
  alterar a tela do QEMU.
- `cargo test`: o kernel compila em modo de teste, dá boot no QEMU sem
  janela gráfica, roda a suíte inteira e reporta sucesso ou falha ao
  host pelo código de saída do próprio comando.
- 23 testes novos: testes de unidade em `src/vga_buffer.rs`,
  `src/keyboard.rs`, `src/shell.rs`, `src/interrupts.rs` e
  `src/serial.rs`, mais dois testes de integração em `tests/`
  (`boot_integration.rs` e `should_panic.rs`).
- `src/serial.rs` (novo módulo) e `src/lib.rs` (novo — reorganização do
  projeto em biblioteca + binário, necessária para os testes de
  integração; nenhuma mudança de comportamento observável).
- Novas seções no `README.md` sobre como rodar e interpretar
  `cargo test`, e um capítulo novo no `WALKTHROUGH.md` sobre a porta
  serial e o executor de testes.
- Este arquivo (`CHANGELOG.md`).

### Alterado

- Versão do projeto: `0.1.0` → `0.2.0`.
- `README.md`: Marcos 1 e 2 marcados como concluídos na tabela de
  marcos, nos detalhes de cada marco e na seção Status; estrutura do
  projeto atualizada.

## [0.1.0] - 2026-09-23

Marco 0 (boot e texto VGA) e Marco 1 (interrupções, teclado e prompt de
comandos).

### Adicionado

- Boot via BIOS direto em um binário Rust `no_std`/`no_main`, sem
  nenhum sistema operacional por baixo.
- Escrita de texto no buffer de vídeo VGA (`0xb8000`): mensagem de
  boas-vindas, rolagem de tela, cursor de hardware.
- Tratamento de panic com mensagem legível na tela.
- IDT, handlers de breakpoint e double fault, reprogramação do PIC 8259
  com apenas a IRQ1 (teclado) habilitada.
- Tradução de scancodes (Scan Code Set 1, layout US QWERTY) e um prompt
  de comandos fixo: `help`, `clear`, `echo`, `sobre`, `panic`.
