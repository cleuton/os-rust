# Phase 1 Data Model: Demonstração de Boot Bare Metal em VGA (proto-os)

Este projeto não tem persistência nem entidades de negócio tradicionais — o
"modelo de dados" aqui é o estado em memória que representa a tela de texto
VGA e a mensagem de panic. Documentado para orientar a implementação de
`src/vga_buffer.rs` e `src/panic.rs`.

## ColorCode

Representa a cor de um caractere na tela: uma cor de primeiro plano e uma de
fundo, codificadas em um único byte (4 bits cada), conforme o hardware VGA em
modo texto.

- **foreground**: cor de primeiro plano fixa, definida no código-fonte (uma
  das 16 cores padrão do modo texto VGA).
- **background**: cor de fundo fixa, definida no código-fonte.
- **Regra de validação**: ambas as cores vêm de um conjunto fechado de 16
  valores conhecidos (enum), não de um valor arbitrário — não é
  configurável em tempo de execução (constituição / FR-002).

## ScreenChar

Uma posição individual na grade de texto da tela.

- **ascii_character**: byte ASCII a ser exibido.
- **color_code**: um `ColorCode` (ver acima).
- **Relacionamento**: é o elemento atômico armazenado em `Buffer`.

## Buffer

A representação em memória do hardware de texto VGA mapeado em `0xb8000`.

- **Dimensões**: 25 linhas × 80 colunas, fixo (tamanho do modo texto VGA
  padrão) — não configurável.
- **chars**: matriz `[[Volatile<ScreenChar>; 80]; 25]`, cada posição escrita
  via acesso volátil para refletir corretamente no hardware/QEMU.
- **Regra de validação**: toda escrita fora dos limites 0..25 / 0..80 é
  inválida por construção (índices controlados internamente pelo `Writer`,
  nunca expostos diretamente).

## Writer

Estado mutável que controla onde e como o próximo caractere é escrito.

- **column_position**: coluna atual do cursor lógico (0..80).
- **color_code**: `ColorCode` fixo usado para toda escrita desta versão (não
  varia por chamada — paleta única definida no código, FR-003).
- **buffer**: referência mutável ao `Buffer` mapeado em `0xb8000`.
- **Transições de estado**:
  - Escrever um caractere imprimível avança `column_position` em 1.
  - Escrever `\n` ou atingir `column_position == 80` move para a próxima
    linha (nova_linha), resetando `column_position` para 0.
  - Ao tentar avançar além da última linha (linha 24), todas as linhas são
    deslocadas uma posição para cima (rolagem simples) e a última linha é
    limpa antes de receber o novo conteúdo — implementa FR-004/Acceptance
    Scenario 3 do User Story 1.
  - "Limpar a tela" (no boot) é o caso inicial: todas as posições do
    `Buffer` são preenchidas com espaço (`b' '`) e `color_code` padrão antes
    da mensagem de boas-vindas ser escrita — implementa FR-002.

## PanicScreen (conceito, não um tipo novo)

Não é uma struct própria — é o reuso do `Writer`/`Buffer` acima a partir do
`#[panic_handler]` (ver `src/panic.rs`), escrevendo a mensagem derivada de
`PanicInfo` (localização no código-fonte + mensagem) como texto normal,
seguindo a mesma paleta de cores fixa. Não há novo estado a modelar; a
diferença é apenas o *caller* (código de panic em vez do fluxo normal de
boot) e o fato de que, após escrever, a execução para (`loop { hlt() }`) em
vez de continuar.
