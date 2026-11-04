# Feature Specification: Demonstração de Boot Bare Metal em VGA (proto-os)

**Feature Branch**: `001-vga-boot-demo`

**Created**: 2026-09-22

**Status**: Draft

**Input**: User description: "Demonstração de curso ou palestra sobre programação bare metal em Rust, chamada proto-os. Motivação: em uma aula sobre Rust de baixo nível é fácil falar sobre 'programação sem sistema operacional', mas é muito mais convincente mostrar ao vivo. O sistema existe para dar boot ao vivo em QEMU, com a plateia vendo a tela preta virar uma tela de texto controlada inteiramente pelo código Rust escrito na aula. Usuário: instrutor de curso ou palestrante, ao vivo ou gravando material de curso. [descrição completa fornecida pelo usuário, incluindo cenário principal, requisitos funcionais e não funcionais, fora de escopo, critérios de aceite, métricas de sucesso e decisões já tomadas]"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Boot ao vivo mostra mensagem de boas-vindas em modo texto (Priority: P1)

Um instrutor, em sala de aula ou gravando material de curso, executa o comando de build/execução do projeto. Uma janela do QEMU abre, mostra o processo de boot via BIOS e, em poucos segundos, exibe uma mensagem de boas-vindas fixa em modo texto VGA, com paleta de cores definida no código-fonte, evidenciando que nenhum sistema operacional está presente.

**Why this priority**: É o núcleo da demonstração — sem isso não há como provar visualmente "programação sem sistema operacional". Todo o resto do projeto existe para sustentar este momento.

**Independent Test**: Pode ser testado de forma isolada executando o comando documentado de build/execução a partir de um checkout limpo e observando que a janela do QEMU mostra a mensagem de boas-vindas fixa, com paleta de cores fixa, sem depender de nenhuma outra funcionalidade do projeto.

**Acceptance Scenarios**:

1. **Given** um ambiente com os pré-requisitos instalados (Rust nightly, ferramenta de geração de imagem de boot, QEMU) e um checkout limpo do repositório, **When** o instrutor roda o comando de execução documentado, **Then** o QEMU abre, completa o boot via BIOS e em poucos segundos mostra a mensagem de boas-vindas fixa em modo texto, com a paleta de cores definida no código, sem tela preta permanente.
2. **Given** o sistema já exibindo a mensagem de boas-vindas, **When** a plateia observa a tela sem receber explicação adicional, **Then** a plateia reconhece que aquela não é uma tela de terminal comum de Linux ou Windows.
3. **Given** uma mensagem de boas-vindas mais longa que uma linha, **When** o texto é escrito no buffer de vídeo, **Then** o sistema avança linha e cursor automaticamente e, se a mensagem ultrapassar a altura da tela, rola o conteúdo de forma simples em vez de sobrescrever ou travar.

---

### User Story 2 - Panic exibe mensagem legível em vez de travar silenciosamente (Priority: P2)

Durante a aula, o instrutor provoca (ou o próprio código dispara) uma condição de erro irrecuperável (panic). Em vez de a tela travar sem explicação ou o sistema reiniciar sem aviso, uma mensagem de erro legível é exibida na tela, permitindo ao instrutor usar o momento como parte didática sobre tratamento de erros em ambiente sem sistema operacional.

**Why this priority**: Reforça o valor didático do projeto (mostrar como um "sistema operacional mínimo" trata falhas fatais) e evita que uma falha durante a demonstração ao vivo pareça um bug sem explicação, o que prejudicaria a credibilidade da aula.

**Independent Test**: Pode ser testado isoladamente provocando um panic proposital no binário e verificando que a tela do QEMU mostra uma mensagem de erro legível, sem depender de interação do usuário ou de outras funcionalidades.

**Acceptance Scenarios**:

1. **Given** o sistema em execução no QEMU, **When** ocorre um panic proposital no código Rust, **Then** a tela exibe uma mensagem de erro legível em modo texto em vez de travar sem explicação ou reiniciar silenciosamente.
2. **Given** a mensagem de panic exibida, **When** o instrutor lê a tela, **Then** a mensagem é suficientemente clara para ser usada como material explicativo durante a aula (não é um código de erro numérico opaco).

---

### User Story 3 - Build e execução reproduzíveis com um único comando (Priority: P3)

Alguém (instrutor, aluno replicando em casa, ou o próprio instrutor em uma máquina diferente) faz um checkout limpo do repositório, segue o README para instalar os pré-requisitos e roda um único comando documentado para compilar e subir o sistema no QEMU, sem passos manuais adicionais.

**Why this priority**: Garante que a demonstração seja replicável (pelo instrutor em outra máquina, ou por alunos interessados em revisar o material depois da aula), mas não é o momento central da demonstração ao vivo — é o que garante que o momento central (User Story 1) sempre funcione de forma confiável.

**Independent Test**: Pode ser testado isoladamente clonando o repositório em uma máquina limpa com os pré-requisitos do README instalados e confirmando que o único comando documentado leva do checkout ao boot funcional, sem etapas manuais extras.

**Acceptance Scenarios**:

1. **Given** um checkout limpo do repositório e uma máquina com os pré-requisitos do README instalados, **When** a pessoa roda o comando único documentado (`cargo run` ou equivalente), **Then** o sistema compila, gera a imagem de boot e sobe no QEMU sem exigir passos manuais adicionais.
2. **Given** o README do projeto, **When** uma pessoa sem conhecimento prévio de QEMU o segue do início, **Then** ela consegue instalar todas as dependências necessárias (incluindo QEMU) e chegar ao ponto de rodar o comando de execução.

---

### Edge Cases

- O que acontece se a mensagem de boas-vindas for mais longa que a área visível da tela? O sistema deve rolar o texto de forma simples, sem travar nem sobrescrever de forma ilegível (coberto no User Story 1).
- O que acontece se ocorrer mais de um panic em sequência, ou o panic ocorrer durante a própria rotina de tratamento de panic? O comportamento esperado é que o sistema pare de forma segura mostrando a última mensagem de erro disponível, em vez de reiniciar em loop sem explicação.
- O que acontece se a pessoa não tiver QEMU ou a toolchain nightly instalados ao rodar o comando de execução? O comando deve falhar com uma mensagem de erro compreensível (mesmo que gerada pelas próprias ferramentas de build), e o README deve cobrir a instalação desses pré-requisitos para evitar essa situação.
- O que acontece se a tela do QEMU for fechada pelo instrutor no meio da execução? Isso é uma finalização normal da demonstração, não uma condição de erro do sistema.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: O sistema MUST dar boot via BIOS em uma máquina virtual QEMU x86_64, sem depender de nenhum sistema operacional previamente instalado no ambiente de execução.
- **FR-002**: Ao dar boot, o sistema MUST limpar a tela e escrever uma mensagem de boas-vindas fixa, definida diretamente no código-fonte (não configurável em tempo de execução), usando o buffer de texto VGA em `0xb8000`.
- **FR-003**: O sistema MUST escrever texto em modo VGA usando uma paleta de cores fixa definida no código (uma cor de primeiro plano e uma de fundo), avançando linha e cursor automaticamente conforme o texto é escrito.
- **FR-004**: O sistema MUST suportar rolagem simples da tela de texto quando o conteúdo escrito ultrapassar a altura visível, sem travar nem corromper o conteúdo exibido.
- **FR-005**: O sistema MUST tratar condições de erro irrecuperável (panic) exibindo uma mensagem legível na tela, em vez de travar silenciosamente ou reiniciar sem aviso.
- **FR-006**: O projeto MUST poder ser compilado e executado com um único comando documentado (`cargo run` ou equivalente), sem exigir passos manuais adicionais além dos pré-requisitos descritos no README.
- **FR-007**: O projeto MUST incluir um `README.md` que ensine, do zero, a instalar todos os pré-requisitos (incluindo QEMU e a toolchain Rust nightly) e a compilar/executar o projeto, sem assumir conhecimento prévio do leitor sobre esse ambiente.
- **FR-008**: O projeto MUST incluir um `WALKTHROUGH.md` voltado a quem nunca viu sistemas operacionais antes, explicando em linguagem acessível o que acontece por trás do processo de boot e do comportamento do código durante a demonstração.
- **FR-009**: Todo trecho de código que exija operações inerentemente inseguras (acesso direto a memória/hardware) MUST ter um comentário explicando por que aquela operação é segura naquele ponto específico.
- **FR-010**: Nomes de funções e módulos MUST refletir o conceito didático que representam (por exemplo, um módulo dedicado ao buffer de vídeo e outro dedicado ao tratamento de panic), para apoiar a explicação em aula.
- **FR-011**: O sistema MUST funcionar de forma idêntica em qualquer máquina que tenha os pré-requisitos documentados instalados (toolchain Rust nightly, ferramenta de geração de imagem de boot, QEMU), sem depender de hardware físico específico.

*Fora do escopo desta versão (explicitamente adiado)*: leitura de teclado via porta I/O e qualquer forma de interação do usuário com o sistema em execução — fica reservado para uma extensão futura com especificação própria. Também fora de escopo: sistema de arquivos, rede, multitarefa, alocador de heap sofisticado, suporte a mouse, interface gráfica, boot via UEFI e boot em hardware físico (apenas QEMU é obrigatório para esta versão).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A partir de um checkout limpo com os pré-requisitos instalados, o comando único documentado leva do início da execução até a mensagem de boas-vindas visível na tela do QEMU em até 30 segundos, em uma máquina de desenvolvimento razoável.
- **SC-002**: Sem receber explicação adicional, observadores da demonstração identificam corretamente que a tela exibida não é um terminal comum de Linux ou Windows.
- **SC-003**: A demonstração funciona de forma idêntica (mesmo resultado visual e mesmo comportamento) em qualquer máquina de desenvolvimento diferente que tenha apenas QEMU e os demais pré-requisitos instalados, sem depender de hardware específico.
- **SC-004**: Ao provocar um panic proposital, 100% das execuções de teste mostram uma mensagem de erro legível na tela em vez de travar sem explicação ou reiniciar silenciosamente.
- **SC-005**: Uma pessoa nova no projeto, seguindo apenas o `README.md`, consegue chegar do checkout limpo até o sistema rodando no QEMU sem precisar de nenhuma etapa ou informação não documentada.
- **SC-006**: O volume total de código-fonte do projeto permanece pequeno o bastante (ordem de poucas centenas de linhas) para ser lido e explicado integralmente em uma aula ou palestra de até 60 minutos.

## Assumptions

- A mensagem de boas-vindas exata e a paleta de cores exata (cor de primeiro plano/fundo) ficam a critério da implementação, desde que fixas no código-fonte e claramente diferentes de uma tela de terminal comum — não são tratadas como decisão de escopo, apenas como detalhe de conteúdo.
- "Poucos segundos" (cenário principal) e "até 30 segundos" (métrica de sucesso) referem-se ao mesmo orçamento de tempo entre o comando de execução e a mensagem visível; 30 segundos é o limite máximo aceitável, sendo o caso comum mais rápido.
- "Máquina de desenvolvimento razoável" assume um computador com especificações típicas de 2020 em diante, sem necessidade de hardware dedicado ou aceleração de virtualização garantida.
- O tratamento de panic (FR-005) cobre o caso didático de uma falha provocada deliberadamente durante a aula; não há requisito de recuperação automática ou reinício do sistema após o panic.
- A leitura de teclado e qualquer interação do usuário ficam completamente fora desta versão e serão tratadas em uma especificação futura separada, conforme já decidido pelo usuário.
- O público-alvo do `WALKTHROUGH.md` não tem conhecimento prévio de sistemas operacionais, então a explicação assume esse nível de entrada, diferente do `README.md`, que assume apenas a necessidade de instalar ferramentas.
