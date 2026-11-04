---

description: "Task list template for feature implementation"
---

# Tasks: Demonstração de Boot Bare Metal em VGA (proto-os)

**Input**: Design documents from `/specs/001-vga-boot-demo/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/vga-output-contract.md, quickstart.md

**Tests**: Nenhum teste automatizado foi solicitado na spec — validação é feita via execução manual em QEMU (`quickstart.md`), conforme decisão de Testing em `research.md` e Princípio IV da constituição.

**Organization**: Tarefas agrupadas por user story para permitir implementação e teste independentes de cada uma.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Pode rodar em paralelo (arquivos diferentes, sem dependência)
- **[Story]**: A qual user story a tarefa pertence (US1, US2, US3)
- Caminhos de arquivo exatos incluídos em cada descrição

## Path Conventions

Binário Rust bare-metal único na raiz do repositório (sem workspace), conforme
`plan.md` → Project Structure: `src/main.rs`, `src/vga_buffer.rs`,
`src/panic.rs`, `Cargo.toml`, `.cargo/config.toml`, `x86_64-proto_os.json`,
`rust-toolchain.toml`, `README.md`, `WALKTHROUGH.md`.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Inicialização do projeto Rust bare-metal e da toolchain de build

- [X] T001 Criar o esqueleto do projeto: `Cargo.toml` (crate binário `proto-os`), e stubs `src/main.rs` (`#![no_std]`, `#![no_main]`), `src/vga_buffer.rs` e `src/panic.rs`, conforme `plan.md` → Project Structure
- [X] T002 [P] Criar `rust-toolchain.toml` pinando a toolchain nightly exigida e o componente `llvm-tools-preview`, conforme `research.md` decisão 1 — **nota de implementação**: pinado em data exata `nightly-2026-08-01` (não canal flutuante) + componente `rust-src` adicional; ver `research.md` Decisão 1.1 para o porquê
- [X] T003 [P] Criar `x86_64-proto_os.json` com a especificação do target custom `x86_64` `no_std`/`no_main` (bare-metal, sem SO subjacente)
- [X] T004 Configurar `.cargo/config.toml`: target padrão = `x86_64-proto_os.json`, `build-std` habilitado, e `bootimage runner` como runner de `cargo run` (depende de T003) — **nota de implementação**: também adicionado `json-target-spec = true` em `[unstable]`, exigido pelo nightly pinado (ver `research.md` Decisão 1.1)
- [X] T005 Adicionar dependências ao `Cargo.toml`: `bootloader` (0.9.x, BIOS), `volatile`, `spin`, `lazy_static` (feature `spin_no_std`), conforme `research.md` decisões 1-3 (depende de T001) — resolvido para `bootloader 0.9.35`; `bootimage` pinado separadamente em `0.10.5` (`cargo install bootimage --version 0.10.5 --locked`)

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Infraestrutura de escrita em VGA e entry point de boot que TODAS as user stories exigem

**⚠️ CRITICAL**: Nenhuma user story pode ser implementada antes desta fase estar completa

- [X] T006 Definir `ColorCode` e `ScreenChar` em `src/vga_buffer.rs`, conforme `data-model.md` (paleta fixa de 16 cores, sem configuração em tempo de execução)
- [X] T007 Definir `Buffer` (grade 25×80 de `Volatile<ScreenChar>` mapeada em `0xb8000`) em `src/vga_buffer.rs` (depende de T006)
- [X] T008 Implementar `Writer` em `src/vga_buffer.rs`: `column_position`, `color_code`, escrita de byte, avanço de linha (`new_line`), rolagem simples ao ultrapassar 25 linhas, e rotina de limpar a tela (`clear_screen`) — FR-002, FR-003, FR-004 (depende de T007)
- [X] T009 Criar `WRITER` global via `lazy_static!` + `spin::Mutex<Writer>` com a paleta de cores fixa escolhida (primeiro plano/fundo), e macros `print!`/`println!` em `src/vga_buffer.rs` (depende de T008)
- [X] T010 Conectar o entry point de boot em `src/main.rs`: macro `bootloader::entry_point!` chamando `kernel_main(boot_info)`, chamando `clear_screen()` logo no início do boot (depende de T009, T004, T005)

**Checkpoint**: Fundação pronta — as user stories podem começar

---

## Phase 3: User Story 1 - Boot ao vivo mostra mensagem de boas-vindas em modo texto (Priority: P1) 🎯 MVP

**Goal**: Ao rodar `cargo run`, o QEMU sobe, faz boot via BIOS e exibe a mensagem de boas-vindas fixa em modo texto VGA, com paleta de cores fixa, avanço de linha/cursor e rolagem simples.

**Independent Test**: Rodar `cargo run` a partir de um checkout com pré-requisitos instalados e observar a tela do QEMU, conforme `quickstart.md` → Cenário 1.

### Implementation for User Story 1

- [X] T011 [US1] Escrever a mensagem de boas-vindas fixa (ex.: "proto-os - sem sistema operacional embaixo") via `println!` em `kernel_main`, logo após `clear_screen()`, em `src/main.rs` — FR-002, FR-003 (depende de T010)
- [X] T012 [US1] Validar rolagem: alongar temporariamente a mensagem em `src/main.rs` para ultrapassar 25 linhas, rodar `cargo run`, confirmar rolagem simples sem travar/corromper conteúdo (conforme `contracts/vga-output-contract.md`), depois restaurar a mensagem final — FR-004 (depende de T011)
- [X] T013 [US1] Rodar `quickstart.md` → Cenário 1 de ponta a ponta no QEMU e confirmar critérios SC-001 (até 30s) e SC-002 (reconhecível como não-terminal) (depende de T012)

**Checkpoint**: User Story 1 (MVP) funcional e testável de forma independente

---

## Phase 4: User Story 2 - Panic exibe mensagem legível em vez de travar (Priority: P2)

**Goal**: Um panic proposital durante a execução mostra uma mensagem de erro legível na tela em vez de travar sem explicação ou reiniciar silenciosamente.

**Independent Test**: Provocar um panic proposital e rodar `cargo run`, observando a tela, conforme `quickstart.md` → Cenário 2.

### Implementation for User Story 2

- [X] T014 [US2] Implementar em `src/panic.rs` a formatação e escrita da mensagem de `PanicInfo` (local + mensagem) reutilizando o `Writer` de `src/vga_buffer.rs`, seguida de um loop de parada segura (`loop { hlt() }`, com `hlt` emitido via `core::arch::asm!("hlt", options(nomem, nostack))` em bloco `unsafe` comentado — sem adicionar a crate `x86_64`, conforme `research.md` Decisão 4) — FR-005, FR-009 (depende de T009)
- [X] T014a [US2] Adicionar trava de reentrância em `src/panic.rs`: `static PANICKING: AtomicBool`; no início do handler, `PANICKING.swap(true, Ordering::SeqCst)` — se já era `true`, ir direto para `loop { hlt() }` sem tentar escrever na tela de novo; se era `false`, seguir o fluxo normal de T014. Resolve o Edge Case de spec.md ("panic durante o próprio panic handler") evitando deadlock no `Mutex` do `WRITER` e preservando a última mensagem já escrita, conforme `research.md` § 4.1 (depende de T014)
- [X] T015 [US2] Conectar `#[panic_handler]` em `src/main.rs` delegando para `panic::handle` (depende de T014a)
- [X] T016 [US2] Rodar `quickstart.md` → Cenário 2: inserir um `panic!` proposital temporário, rodar `cargo run`, confirmar mensagem legível e parada estável (SC-004); em seguida, inserir um segundo `panic!` proposital dentro do próprio código do panic handler (ex.: temporariamente forçar `panic::handle` a chamar a si mesmo ou a travar o `Mutex` do `WRITER` antes de escrever) e confirmar que o sistema ainda para de forma segura mostrando a última mensagem válida, sem deadlock e sem reiniciar — depois remover os dois gatilhos temporários (depende de T015)

**Checkpoint**: User Stories 1 e 2 funcionam de forma independente

---

## Phase 5: User Story 3 - Build e execução reproduzíveis com um único comando (Priority: P3)

**Goal**: A partir de um checkout limpo, seguindo apenas o `README.md`, qualquer pessoa com os pré-requisitos consegue compilar e rodar o sistema com um único comando documentado.

**Independent Test**: Clonar o repositório em um ambiente limpo, seguir só o `README.md`, rodar o comando único, conforme `quickstart.md` → Cenário 3.

### Implementation for User Story 3

- [X] T017 [P] [US3] Escrever `README.md`: instalação de pré-requisitos do zero (toolchain Rust nightly + `llvm-tools-preview`, `bootimage`, QEMU) e instrução de execução via `cargo run` — FR-006, FR-007
- [X] T018 [P] [US3] Escrever `WALKTHROUGH.md`: explicação, para quem nunca viu sistemas operacionais, do que acontece no boot via BIOS e no comportamento do código (buffer VGA, panic) — FR-008
- [X] T019 [US3] Rodar `quickstart.md` → Cenário 3 em um checkout limpo seguindo apenas o `README.md`, confirmando SC-005 (depende de T017)

**Checkpoint**: Todas as user stories funcionam de forma independente

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Conformidade final com a constituição e validação de aceite completa

- [X] T020 [P] Revisar todos os blocos `unsafe` em `src/vga_buffer.rs`, `src/main.rs` e `src/panic.rs`, garantindo comentário explicando por que cada um é seguro naquele ponto — FR-009 / Princípio V da constituição
- [X] T021 [P] Revisar `src/` em busca de `unwrap`/`expect` sem justificativa em código sem rede de segurança — Princípio V da constituição
- [X] T022 Verificar se o volume total de código-fonte permanece na ordem de poucas centenas de linhas (SC-006) e revisar se os nomes de módulos/funções em `src/` refletem o conceito didático de cada um (FR-010: `vga_buffer` para saída de texto, `panic` para tratamento de erro irrecuperável, etc.); simplificar/renomear se necessário
- [X] T023 Rodar `quickstart.md` completo (Cenários 1, 2 e 3) como passagem final de aceite antes de considerar a feature pronta
- [X] T024 [P] Confirmar independência de hardware (FR-011/SC-003): revisar `x86_64-proto_os.json`, `Cargo.toml` e `src/` em busca de qualquer detecção de CPU/host ou caminho de código condicionado a hardware específico; confirmar que a única variável de execução é o QEMU instalado, não o computador hospedeiro

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: Sem dependências — pode começar imediatamente
- **Foundational (Phase 2)**: Depende da conclusão do Setup — BLOQUEIA todas as user stories
- **User Stories (Phase 3-5)**: Todas dependem da conclusão da fase Foundational
  - Podem prosseguir em paralelo (se houver mais de uma pessoa) ou sequencialmente em ordem de prioridade (P1 → P2 → P3)
- **Polish (Phase 6)**: Depende da conclusão das user stories desejadas

### User Story Dependencies

- **User Story 1 (P1)**: Pode começar após a fase Foundational — sem dependência de outras stories
- **User Story 2 (P2)**: Pode começar após a fase Foundational — reutiliza o `Writer` de `vga_buffer.rs` (Foundational), mas é testável de forma independente da US1
- **User Story 3 (P3)**: Pode começar após a fase Foundational — documenta o resultado das US1/US2, mas os documentos em si (`README.md`/`WALKTHROUGH.md`) podem ser escritos em paralelo a elas

### Within Each User Story

- Implementação segue a ordem: tipos/estado (Foundational) → lógica específica da story → validação via `quickstart.md`
- Cada story termina com uma tarefa de validação manual em QEMU antes de ser considerada completa

### Parallel Opportunities

- T002 e T003 (Setup) podem rodar em paralelo — arquivos diferentes, sem dependência entre si
- T017 e T018 (US3) podem rodar em paralelo — `README.md` e `WALKTHROUGH.md` são arquivos independentes
- T020, T021 e T024 (Polish) podem rodar em paralelo — revisões independentes sobre arquivos/preocupações distintas
- Tarefas dentro da fase Foundational (T006-T010) são sequenciais (mesmo arquivo `src/vga_buffer.rs` evoluindo passo a passo)
- T014 e T014a (US2) são sequenciais (mesmo arquivo `src/panic.rs`, T014a depende da estrutura criada por T014)

---

## Parallel Example: Setup

```bash
# Rodar em paralelo durante o Setup:
Task: "Criar rust-toolchain.toml pinando a toolchain nightly exigida e llvm-tools-preview"
Task: "Criar x86_64-proto_os.json com a especificação do target custom x86_64 no_std/no_main"
```

## Parallel Example: User Story 3

```bash
# Rodar em paralelo na User Story 3:
Task: "Escrever README.md com instalação de pré-requisitos e instrução de execução via cargo run"
Task: "Escrever WALKTHROUGH.md explicando o boot e o código para quem nunca viu sistemas operacionais"
```

---

## Implementation Strategy

### MVP First (User Story 1 apenas)

1. Completar Phase 1: Setup
2. Completar Phase 2: Foundational (CRÍTICO — bloqueia todas as stories)
3. Completar Phase 3: User Story 1
4. **PARAR e VALIDAR**: testar User Story 1 de forma independente via `quickstart.md` Cenário 1
5. Já é demonstrável em aula neste ponto (mensagem de boas-vindas em VGA)

### Incremental Delivery

1. Completar Setup + Foundational → fundação pronta
2. Adicionar User Story 1 → testar independentemente → demonstrável (MVP!)
3. Adicionar User Story 2 → testar independentemente → panic tratado
4. Adicionar User Story 3 → testar independentemente → documentação e reprodutibilidade completas
5. Cada story adiciona valor sem quebrar as anteriores

---

## Notes

- [P] = arquivos diferentes, sem dependências entre as tarefas
- [Story] mapeia a tarefa à user story correspondente para rastreabilidade
- Sem testes automatizados nesta versão — `quickstart.md` é o método de validação (ver `research.md`)
- Fazer commit após cada tarefa ou grupo lógico de tarefas
- Parar em cada checkpoint para validar a story de forma independente antes de seguir
- Evitar: tarefas vagas, conflitos no mesmo arquivo marcados como [P], dependências entre stories que quebrem a independência
