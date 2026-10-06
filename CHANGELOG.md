# Changelog

Todas as mudanças notáveis deste projeto são registradas neste arquivo,
uma versão por vez. O formato segue, livremente,
[Keep a Changelog](https://keepachangelog.com/), e as versões seguem
[Versionamento Semântico](https://semver.org/lang/pt-BR/).

## [0.7.0] - 2026-10-06

Marco 7: multitarefa. O os-rust passa a rodar **vários programas de usuário
ao mesmo tempo**, com o kernel alternando a CPU entre eles: primeiro de forma
cooperativa (o programa cede a CPU por uma syscall nova) e depois de forma
preemptiva (um timer de hardware tira a CPU de quem não a cede). Cada programa
roda na sua própria memória. `run ping pong` mostra as linhas dos dois
alternadas; `run contador_a contador_b` mostra a saída dos dois intercalada
sem que nenhum peça a vez; `run falha_memoria contador_a` encerra só o que
falhou; `run eco eco2` entrega cada linha digitada ao programa que pediu
primeiro.

### Adicionado

- Syscall `SYS_YIELD` (5: cede a CPU ao próximo programa pronto) e o contrato
  de syscalls versão 3 (`SYSCALLS.md`): a syscall nova, as regras de execução
  simultânea (o que o programa pode e não pode assumir), a fatia de tempo, o
  máximo de 4 programas ao mesmo tempo e a regra do teclado com vários
  programas. Programas das versões 1 e 2 continuam funcionando.
- Uma tarefa por programa carregado (`src/task.rs`): o estado dele em ring 3
  (`TaskContext`, os 15 registradores gerais mais o frame de `iretq`), seu
  estado de execução e o pedido de teclado em andamento.
- Escalonador em rodízio (`src/scheduler.rs`): toda troca acontece na
  fronteira ring 3 → ring 0, com a pilha de entrada do kernel vazia, então não
  há pilha de kernel por tarefa; o kernel nunca é trocado.
- Espaço de endereçamento próprio para cada programa (`memory::AddressSpace`):
  tabela P4 e P3 próprias, copiadas das do kernel, com a região do usuário
  vazia; trocar de tarefa troca o `CR3`. Ao terminar, todos os frames do
  programa, inclusive os das tabelas de página, voltam ao alocador.
- Timer (`src/timer.rs`): PIT a 100 Hz pela IRQ0 do PIC 8259, fatia de 5 ticks
  (50 ms) e preempção. Um tick que interrompe ring 0 só envia o EOI e volta.
- `keyboard::LineEditor`: o montador de linha extraído de `read_line`, usado
  pelo escalonador para montar a linha de quem pediu primeiro.
- `run <nome> [<nome>...]`: carrega até 4 programas, tudo ou nada, e devolve o
  prompt quando o último termina. O mesmo nome pode se repetir.
- Biblioteca de runtime: `yield_now()`. Cada `print!`/`println!` passa a sair
  numa única chamada `write` (até 256 bytes), então uma linha não é partida
  pela saída de outro programa.
- Programas de exemplo `ping` e `pong` (se alternam com `yield_now`),
  `contador_a` e `contador_b` (contam em laço longo sem nunca ceder a CPU) e
  `eco2` (o irmão do `eco`, para `run eco eco2`).
- Seção "Multitarefa" no `GUIA_DO_PROGRAMADOR.md`, com o código completo dos
  cinco programas novos, e um capítulo no `WALKTHROUGH.md`.
- Testes: `tests/multitarefa.rs` (contexto cooperativo, alternância,
  preempção, isolamento de memória e de falhas, teclado com vários programas,
  vazamento em 100 execuções, recusas do `run`) e `tests/artefatos.rs`
  (higiene dos arquivos entregues, com a lista embutida pelo `build.rs`).
  Também `memory::frames_outstanding` e `scheduler::set_poll_hook`, que
  existem só para os testes.
- Crate `abi`: `SYS_YIELD`, `MAX_TASKS`, `TIMER_HZ` e `SLICE_TICKS`.

### Alterado

- O stub de `syscall` monta um `TaskContext` na pilha do kernel e volta ao
  programa por `iretq` (antes, por `sysretq`): `rcx` e `r11` voltam intactos,
  embora o contrato continue dizendo que são destruídos.
- `SYS_READ_LINE` bloqueia a tarefa em vez de esperar dentro da syscall: quem
  espera não gasta CPU, e com todos esperando o kernel dorme em `hlt`.
- A máscara do PIC libera a IRQ0 junto com a IRQ1.
- A mensagem de término de um programa aparece na hora em que ele termina, e
  o prompt volta quando o último termina.
- `run eco xyz` agora trata `xyz` como o nome de um segundo programa (antes, os
  argumentos depois do primeiro nome eram ignorados).
- A lista `disponiveis:` das mensagens de `run` quebra de linha entre os nomes,
  porque com mais programas ela passou de uma linha da tela.
- O alocador de frames só avança o contador quando entrega um frame.
- Versão do projeto: `0.6.0` → `0.7.0`.

## [0.6.0] - 2026-09-28

Marco 6: interface de programação. Escrever, compilar e rodar um programa
de usuário passa a ser possível sem conhecer o kernel por dentro: o
contrato de syscalls ganha a leitura de teclado e memória para o programa,
há uma biblioteca de runtime, e a garantia de que uma falha no programa
não derruba o kernel ganha um programa de demonstração. `run eco` lê uma
linha do teclado e responde; `run falha_memoria` é encerrado por acesso
inválido à memória sem derrubar o kernel.

### Adicionado

- Syscalls `SYS_READ_LINE` (3: espera uma linha do teclado, com eco e
  Backspace feitos pelo kernel, e a entrega ao programa) e `SYS_ALLOC` (4:
  amplia o heap do programa, numa janela de até 1 MiB em `0x6000_0000`), e o
  código de erro `ERR_NOMEM` (`-3`).
- Contrato de syscalls versão 2 (`SYSCALLS.md`): as duas syscalls, o erro
  novo, a janela do heap, a faixa de código e dados do ELF, a mensagem de
  `#PF` de programa e uma seção sobre o teclado enquanto o programa roda.
  Programas da versão 1 continuam funcionando.
- Crate `abi/`: as constantes do contrato (números, erros, limites),
  compartilhadas pelo kernel e pela biblioteca de runtime.
- Crate `runtime/`: a biblioteca de runtime dos programas (`entry!`,
  `print!`/`println!`, `read_line`, `exit`, alocador global sobre
  `SYS_ALLOC` com o mesmo `linked_list_allocator` do kernel, e um tratador de
  `panic!` que escreve `[panic] <mensagem>` e sai com o código 101).
- Programas de exemplo `eco` (lê uma linha, devolve o texto e conta as
  palavras com um `Vec`) e `falha_memoria` (escreve em `0xdead_beef`).
- `GUIA_DO_PROGRAMADOR.md`: o guia de quem escreve programas para o os-rust,
  com o código completo dos dois programas de exemplo.
- `interrupts::push_scancode` (pública, para os testes "digitarem") e
  `shell::poll_keyboard` (o laço de teclado do prompt, extraído de
  `main.rs`).
- `tests/user_runtime.rs` (29 testes de integração em ring 3: leitura de
  teclado, memória, `SYS_READ_LINE` inválida, isolamento de falhas, o prompt
  retomando o teclado, o guia batendo com o código) e testes de unidade para
  `keyboard::read_line`, o contrato v2, o limite do ELF, o texto do `#PF`
  de programa e o alinhamento da pilha de entrada.

### Alterado

- `hello` e `crash` passam a usar a biblioteca de runtime (mesmo
  comportamento observável). O `asm!` da instrução `syscall`, que o `hello`
  do Marco 5 trazia à mão, vive agora em `runtime/src/sys.rs`.
- A mensagem de um programa encerrado por `#PF` passa a dizer `encerrado por
  erro de memoria`; as demais exceções mantêm o texto do Marco 5.
- Os segmentos de um ELF ficam limitados a `[0x4000_0000, 0x6000_0000)`
  (antes iam até a pilha), para não cair em cima do heap do programa.
- O workspace ganha os membros `runtime` e `abi`; o kernel passa a depender
  de `abi`.
- Versão do projeto: `0.5.0` → `0.6.0`.

### Corrigido

- As pilhas do kernel para entradas vindas de ring 3 e para o double fault
  não tinham alinhamento declarado, e o topo da primeira podia cair num
  endereço que não é múltiplo de 16; a espera por uma tecla dentro de uma
  syscall expôs o problema. Ambas passam a ser `#[repr(align(16))]`.

## [0.5.0] - 2026-09-28

Marco 5: primeiro programa de usuário. O os-rust passa a executar código
que não é do kernel, em modo usuário (ring 3): o comando `run hello` do
prompt carrega um programa embutido na imagem de boot, que escreve na
tela por meio de uma chamada de sistema e devolve o controle ao prompt.

### Adicionado

- Comando `run <nome>` no prompt: executa um programa de usuário embutido;
  sem argumento, ou com um nome desconhecido, lista os programas
  disponíveis.
- Modo usuário (ring 3): segmentos de código e dados do usuário na GDT e
  entrada em ring 3 (`src/user.rs`), com volta ao prompt quando o programa
  termina, seja por `exit`, seja por uma falha.
- Mecanismo de chamada de sistema com a instrução `syscall`/`sysret`
  (`src/syscall.rs`) e as duas primeiras syscalls, `write` e `exit`.
- Contrato de syscalls versão 1 em um único arquivo, `SYSCALLS.md`
  (números, registradores, códigos de erro, formato do executável, região
  de carga, pilha inicial); testes automatizados conferem que o texto
  concorda com o código.
- Carregador de executáveis ELF64 estáticos escrito à mão (`src/elf.rs`),
  com permissões por segmento (W^X) e recusa de ELFs inválidos sem mapear
  nada.
- Programas de referência `hello` (escreve `Ola do ring 3!` e termina) e
  `crash` (executa uma instrução inválida de propósito), na nova crate
  `programs/`, compilada para o novo target `x86_64-os_rust_user.json`.
- O `build.rs` da raiz compila os programas de usuário e os embute no
  kernel dentro do mesmo `cargo run`/`cargo test`; um erro de compilação
  no programa para o build com o erro à vista.
- `tests/user_mode.rs` (16 testes de integração em ring 3), testes de
  unidade para o leitor de ELF e para o contrato, dois testes do comando
  `run` e um de `Writer::write_bytes`.

### Alterado

- A GDT e a TSS ganham segmentos de dados do kernel e de código e dados do
  usuário, e uma pilha do kernel para as entradas vindas de ring 3.
- O alocador de frames passa a ser global, com reciclagem dos frames dos
  programas encerrados.
- Os handlers de exceção distinguem ring 3 de ring 0: uma falha em programa
  de usuário encerra só o programa; em ring 0 continuam fatais, como antes.
- `#DE`, `#SS` e `#NP` passam a ter handler (sem eles, a CPU escalaria para
  double fault).
- O repositório vira um workspace Cargo (o kernel continua na raiz;
  `programs/` é o segundo membro).
- Versão do projeto: `0.4.2` → `0.5.0`.

## [0.4.2] - 2026-09-25

Ajuste cosmético: nenhuma capacidade nova do kernel, nenhuma mudança de
comportamento observável.

### Alterado

- Comentários de código em `src/` e `tests/` (GDT/TSS, interrupções,
  memória, heap, serial, teclado, prompt e todos os testes de
  integração): cada um agora declara por extenso, no próprio comentário,
  o motivo da decisão que ele documenta.
- Versão do projeto: `0.4.1` → `0.4.2`.

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
