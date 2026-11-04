# Phase 0 Research: Demonstração de Boot Bare Metal em VGA (proto-os)

Nenhum `NEEDS CLARIFICATION` restou no Technical Context do `plan.md` — as
decisões abaixo resolvem as escolhas técnicas necessárias para transformar a
spec em um plano concreto, com alternativas consideradas e rejeitadas.

## 1. Caminho de boot: `bootloader` 0.9.x + `bootimage` vs. `bootloader` 0.11+ (`BiosBoot`)

**Decision**: Usar a linha `bootloader` 0.9.x combinada com a ferramenta
`bootimage` (instalada via `cargo install bootimage`) como *runner* configurado
em `.cargo/config.toml`, de forma que `cargo run` compile o kernel, gere a
imagem de boot e suba o QEMU automaticamente.

**Rationale**: Satisfaz diretamente FR-006/SC-001 (um único comando, do
checkout à tela) com o menor número de peças móveis possível — uma única
crate de kernel, um `Cargo.toml`, um `.cargo/config.toml`. É o fluxo
documentado e testado pela literatura de referência para exatamente este tipo
de demonstração didática (boot BIOS mínimo com saída VGA), o que reduz risco
de quebrar durante uma aula ao vivo. Alinha com o Princípio III da
constituição (simplicidade sobre generalidade).

**Validação empírica durante `/speckit-implement`** (2026-09-22): esta
decisão foi testada na prática, incluindo uma tentativa real de migrar para
`bootloader` 0.11+ com `bootloader_api`/`BiosBoot` (a alternativa rejeitada
abaixo). Essa tentativa revelou um motivo **funcional**, não só de
simplicidade, para ficar em 0.9.x: `bootloader_api::BootInfo` (0.11+) expõe
apenas um `framebuffer` de pixels — não existe mais um conceito de "buffer
de texto VGA em 0xb8000" nessa API. Usar 0.11+ exigiria desenhar glifos
bitmap em um framebuffer gráfico, o que **viola diretamente** a restrição
técnica fixa da constituição ("saída: buffer de texto VGA em 0xb8000, sem
driver gráfico") e FR-002/FR-003. `bootloader` 0.9.x, por preservar o modo
texto VGA legado do BIOS sem alterá-lo, é a única das duas opções
compatível com o requisito funcional do projeto — a alternativa 0.11+ foi
descartada por violação de requisito, não apenas por complexidade.

A build real contra um toolchain nightly de 2026 também exigiu ajustes que
não estavam previstos originalmente (documentados na íntegra na Decisão 1.1
abaixo): o formato do *target spec* JSON usado tanto pelo nosso
`x86_64-proto_os.json` quanto pelo `x86_64-bootloader.json` interno da
crate `bootloader` já não é o mesmo de quando os tutoriais clássicos desse
padrão (ex.: "Writing an OS in Rust", de Philipp Oppermann) foram escritos.

**Alternatives considered**:
- `bootloader` 0.11+ com `bootloader_api`/`BiosBoot` em `build.rs`: suporta
  BIOS e UEFI, é mais flexível e ativamente desenvolvido, mas exige separar o
  kernel em uma crate e o "montador de imagem" em outra (via
  *artifact-dependencies* do Cargo), além de um `build.rs` próprio. Mais
  peças móveis e mais conceitos novos para apresentar em 60 minutos — já
  seria rejeitado só por conflitar com o Princípio III e com FR-006. **Mas o
  motivo definitivo da rejeição é funcional**: `bootloader_api::BootInfo`
  (confirmado lendo `bootloader_api-0.11.17/src/info.rs`) só expõe um
  `framebuffer` de pixels, sem nenhum equivalente a "buffer de texto VGA em
  0xb8000" — testado de ponta a ponta durante a implementação (kernel
  migrado para um workspace de 2 crates, buildado e bootado com sucesso),
  mas a tela mostrada seria necessariamente modo gráfico (glifos bitmap
  desenhados no framebuffer), o que viola a restrição técnica fixa da
  constituição ("sem driver gráfico") e FR-002/FR-003. Rejeitado por
  violação de requisito funcional, confirmada empiricamente, não apenas por
  complexidade.
- Escrever um bootloader próprio a partir de um setor de boot de 512 bytes em
  assembly: didaticamente interessante, mas está fora do escopo por si só —
  a demonstração é sobre "Rust sem sistema operacional", não sobre "escrever
  um bootloader do zero", e adicionaria risco desnecessário à demo ao vivo.
  Rejeitado por violar o Princípio I (escopo mínimo).

## 1.1. Pin exato do toolchain nightly

**Decision**: Pinar `rust-toolchain.toml` em uma **data exata** de nightly
(`nightly-2026-08-01`), não em um canal flutuante (`channel = "nightly"`), e
adicionar `json-target-spec = true` à tabela `[unstable]` de
`.cargo/config.toml`. Pinar também `bootimage` em uma versão exata
(`cargo install bootimage --version 0.10.5 --locked`), documentada no
README.

**Rationale**: Validado empiricamente durante `/speckit-implement`. O
formato do *target spec* JSON (usado tanto pelo nosso
`x86_64-proto_os.json` quanto pelo `x86_64-bootloader.json` interno da
crate `bootloader`) é **não-estável** e mudou de forma incompatível várias
vezes só nos nightlies testados nesta sessão:
- `target-pointer-width` mudou de string (`"64"`) para número (`64`) e
  depois voltou a mudar exigências de tipo entre snapshots.
- O campo `executable` (booleano) foi renomeado para `executables`.
- O campo `rustc-abi` foi introduzido, depois passou a exigir o prefixo
  `"x86-"` (`"x86-softfloat"`), depois voltou a aceitar o valor sem prefixo
  (`"softfloat"`).
- A flag `-Z json-target-spec` alternou entre "não reconhecida" e
  "obrigatória" em diferentes datas de nightly — em `nightly-2026-08-01`
  ela é **obrigatória** mesmo para o build direto do nosso próprio kernel,
  por isso a entrada `json-target-spec = true` em `.cargo/config.toml`.

Sem um pin exato, `cargo run` funcionar ou não passa a depender de qual
nightly o `rustup` instala por padrão na máquina de quem clona o
repositório — o oposto de FR-011/SC-003 (funcionar identicamente em
qualquer máquina). `nightly-2026-08-01` foi a primeira data testada, nesta
sessão, em que: (a) o schema do target JSON bate com o que a crate
`bootloader` 0.9.35 já publicada espera, e (b) a build completa (kernel +
bootloader interno + bootimage) funciona sem nenhum patch em nenhuma
dependência.

**Alternatives considered**:
- `channel = "nightly"` flutuante: mais simples de escrever, mas
  demonstrado nesta própria sessão como frágil — a mesma configuração
  quebrou de formas diferentes em nightlies de 2023, 2025 e início de 2026
  antes de `nightly-2026-08-01` funcionar. Rejeitado por violar
  Reprodutibilidade (Princípio VI).
- Vendorizar (copiar para `vendor/` e patchear) a crate `bootloader` para
  remover/ajustar a flag `-Zjson-target-spec` e os campos de target JSON
  problemáticos: chegou a ser implementado e funcionou, mas é uma
  complexidade permanente (uma cópia local de ~300 KB de uma dependência
  externa) para contornar um problema que um pin de data resolve de forma
  muito mais simples. Removido depois de confirmar que o pin de data
  sozinho já resolve tudo. Preferir sempre o pin de data a um patch
  vendorizado, a menos que uma data que funcione simplesmente não exista.

## 2. Escrita no buffer de texto VGA

**Decision**: Acessar o buffer de texto VGA diretamente em `0xb8000` como uma
matriz fixa de 25 linhas × 80 colunas de `ScreenChar` (byte ASCII + byte de
cor), usando a crate `volatile` para garantir que o compilador não elimine ou
reordene as escritas (o hardware lê essa memória fora do modelo de
otimização do compilador).

**Rationale**: É o mecanismo de saída mais simples e direto disponível sem
sistema operacional, exatamente o que a spec pede (FR-002/FR-003) e o que a
constituição fixa como restrição técnica (sem driver gráfico). `volatile`
evita um bug sutil e didaticamente relevante (escritas "desaparecendo" por
otimização), reforçando a explicação sobre por que `unsafe` isolado e
comentado é necessário (Princípio V / FR-009).

**Alternatives considered**:
- Modo gráfico (framebuffer linear, VBE/GOP): fora de escopo — a constituição
  fixa explicitamente "sem driver gráfico" e a spec não pede nada além de
  texto. Rejeitado.
- Escrever sem `volatile` usando ponteiros brutos e `write_bytes`/atribuição
  direta: funciona na prática na maioria dos casos, mas é uma cilada didática
  (o compilador pode otimizar a escrita) e um exemplo ruim de `unsafe`
  justificado. Rejeitado em favor de `volatile`, que documenta a intenção.

## 3. Sincronização de estado global (`Writer`)

**Decision**: Usar `spin::Mutex` (crate `spin`) para proteger um `Writer`
global inicializado com `lazy_static!` (feature `spin_no_std`), fornecendo
macros `print!`/`println!` como nas edições anteriores de Rust, mas
operando sobre o buffer VGA.

**Rationale**: Sem sistema operacional não há `std::sync::Mutex` nem alocação
lazy de `std`. `spin`+`lazy_static` é o par mínimo necessário para ter um
"escritor" global seguro e ergonômico (`println!("...")` dentro do código de
boot e do handler de panic), sem introduzir um alocador de heap completo.

**Alternatives considered**:
- Passar o `Writer` explicitamente por parâmetro entre funções, sem estado
  global: elimina a necessidade de `spin`/`lazy_static`, mas complica a API
  interna sem ganho didático — o objetivo da aula é boot/VGA/panic, não
  gerenciamento de estado. Rejeitado por simplicidade líquida menor, não
  maior.
- Implementar um alocador de heap mínimo para usar `std::sync`-like
  primitives: viola a restrição "sem alocador de heap sofisticado" da
  constituição. Rejeitado.

## 4. Tratamento de panic

**Decision**: Implementar `#[panic_handler]` em `src/main.rs` delegando a
formatação/escrita para `src/panic.rs`, que reusa o `Writer` do
`vga_buffer` para imprimir a mensagem do `PanicInfo` (localização + mensagem)
em modo texto, e então entra em `loop { hlt() }` para parar a CPU sem travar
de forma "ocupada" nem reiniciar. A instrução `hlt` é emitida diretamente via
`core::arch::asm!("hlt", options(nomem, nostack))` dentro de um bloco
`unsafe` comentado (FR-009) — **sem** adicionar a crate `x86_64` como
dependência só para isso, o que manteria a lista de dependências do Setup
(`bootloader`, `volatile`, `spin`, `lazy_static`) sem alteração.

**Rationale**: Atende FR-005/User Story 2 — mensagem legível em vez de tela
preta ou reinício silencioso — reaproveitando a mesma infraestrutura de saída
de texto já construída para a mensagem de boas-vindas (sem duplicar lógica).
Usar `core::arch::asm!` diretamente evita uma dependência inteira só para uma
instrução, o que é mais simples de explicar em aula do que introduzir a crate
`x86_64` (Princípio III da constituição).

**Alternatives considered**:
- `loop {}` vazio após o panic (busy-loop): funciona, mas gasta CPU
  desnecessariamente e não é o padrão idiomático recomendado; `hlt()` é
  trivialmente mais correto e ainda simples o bastante para explicar em aula.
  Rejeitado como escolha principal, mantido apenas como fallback conceitual.
- Adicionar a crate `x86_64` só para usar `x86_64::instructions::hlt()`: mais
  idiomático em projetos maiores, mas é uma dependência inteira para uma
  única instrução — rejeitado por simplicidade líquida menor (Princípio III).
- Reiniciar a máquina após o panic (triple fault proposital ou instrução de
  reset): explicitamente rejeitado pela spec (FR-005 exige mensagem visível,
  não reinício silencioso).

### 4.1. Segurança contra panic reentrante (panic dentro do panic handler)

**Decision**: Proteger `#[panic_handler]` com uma trava simples de
reentrância: um `static PANICKING: AtomicBool` (em `src/panic.rs`,
`core::sync::atomic::AtomicBool`/`Ordering::SeqCst`). No início do handler,
`PANICKING.swap(true, Ordering::SeqCst)` — se já era `true` (ou seja, o
próprio tratamento de panic causou um novo panic, por exemplo por um
`Mutex` de `WRITER` já travado ou por uma falha ao formatar a mensagem), o
handler **não tenta escrever na tela de novo** e vai direto para
`loop { hlt() }`, preservando a última mensagem já escrita. Se era `false`
(primeiro panic), o handler segue o fluxo normal: escreve a mensagem e para.

**Rationale**: Resolve diretamente o Edge Case da spec ("o que acontece se
ocorrer mais de um panic em sequência, ou o panic ocorrer durante a própria
rotina de tratamento de panic?") — o comportamento exigido é "parar de forma
segura mostrando a última mensagem de erro disponível, em vez de reiniciar em
loop". Sem essa trava, um segundo panic dentro do handler tentaria travar
(`lock()`) o `spin::Mutex` do `WRITER` que o primeiro panic já pode estar
segurando, causando um deadlock (a CPU gira infinitamente sem nunca chegar ao
`hlt()`) — o que do ponto de vista da plateia parece exatamente um travamento
sem explicação, o oposto do que FR-005 exige. Um `AtomicBool` é a primitiva
mais simples possível para essa proteção, sem alocador e sem nova dependência
(Princípio III).

**Alternatives considered**:
- Nenhuma proteção (confiar que o handler nunca panica): rejeitado — viola
  diretamente o Edge Case já documentado na spec; um bug simples (ex.: um
  `unwrap()` esquecido na formatação da mensagem) travaria a demonstração ao
  vivo sem explicação.
- Usar `WRITER.force_unlock()` antes de cada tentativa de escrita no handler
  de panic, sem flag de reentrância: evita o deadlock no `Mutex`, mas não
  evita uma segunda formatação/escrita sobrescrever ou corromper a mensagem
  do primeiro panic (a spec pede "mostrando a última mensagem de erro
  disponível", que a trava por `AtomicBool` preserva de forma mais direta).
  Rejeitado em favor da trava de reentrância, que é mais simples de explicar
  e cobre os dois problemas (deadlock e corrupção da mensagem) de uma vez.

## 5. Estratégia de testes

**Decision**: Validação primária é manual/visual no QEMU, documentada em
`quickstart.md` (rodar `cargo run`, observar mensagem; provocar panic,
observar mensagem de erro). Nenhum framework de teste dentro do kernel é
obrigatório para o v1.

**Rationale**: Consistente com o Princípio IV da constituição ("Testável em
QEMU, sem hardware real" — QEMU já é o ambiente de validação por definição) e
com o orçamento de 60 minutos de aula (FR/SC de tamanho de código). Testes
automatizados de kernel via `custom_test_frameworks` + exit code do QEMU são
um padrão conhecido, mas adicionam configuração e conceitos que não servem
diretamente ao objetivo didático desta v1.

**Alternatives considered**:
- `custom_test_frameworks` com testes de integração rodando dentro do QEMU e
  saindo via porta de I/O especial (padrão `isa-debug-exit`): mais robusto
  para evolução futura do projeto, mas fora de escopo para uma demo de 60
  minutos cujo critério de aceite já é observação visual (SC-002, SC-004).
  Fica como nota para uma eventual extensão futura, não um requisito da v1.
