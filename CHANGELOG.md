# Changelog

Todas as mudanças notáveis deste projeto são registradas neste arquivo,
uma versão por vez. O formato segue, livremente,
[Keep a Changelog](https://keepachangelog.com/), e as versões seguem
[Versionamento Semântico](https://semver.org/lang/pt-BR/).

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
- `proto_os::init` passa a receber o `BootInfo` entregue pelo
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
