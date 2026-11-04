# Quickstart: Validando a Demonstração de Boot VGA (proto-os)

Guia de verificação end-to-end desta feature. Não é o README do projeto (esse
ensina instalação de pré-requisitos do zero) — este documento assume o
ambiente já configurado e serve para confirmar que a implementação atende à
spec e ao [contrato de saída VGA](./contracts/vga-output-contract.md).

## Pré-requisitos

- Toolchain Rust nightly pinada pelo `rust-toolchain.toml` do projeto.
- `bootimage` instalado (`cargo install bootimage`) e componente
  `llvm-tools-preview` do rustup adicionado.
- QEMU instalado (`qemu-system-x86_64` no PATH).
- Checkout limpo deste repositório.

(Instalação detalhada de cada item fica em `README.md`, não aqui.)

## Cenário 1 — Boot mostra a mensagem de boas-vindas (User Story 1)

1. Na raiz do repositório, rode:
   ```sh
   cargo run
   ```
2. **Esperado**: uma janela do QEMU abre, mostra rapidamente o processo de
   boot via BIOS, e em até 30 segundos (tipicamente poucos segundos) exibe
   uma tela em modo texto com:
   - a tela limpa (sem lixo de boot remanescente),
   - uma mensagem de boas-vindas fixa mencionando o projeto,
   - cores diferentes do preto-e-branco padrão de um terminal.
3. **Verificação de rolagem**: se a mensagem configurada for propositalmente
   maior que 25 linhas (ajuste temporário para teste), confirmar que a tela
   rola em vez de travar ou corromper o conteúdo.
4. Feche a janela do QEMU para encerrar — isso é o fim normal da
   demonstração, não uma condição de erro.

**Critério de sucesso** (SC-001, SC-002): a tela chega ao estado acima sem
intervenção manual além do `cargo run`, e alguém observando sem explicação
prévia reconhece que não é um terminal comum.

## Cenário 2 — Panic mostra mensagem legível (User Story 2)

1. Provocar um panic proposital no binário (ex.: uma chamada temporária a
   `panic!("teste de demonstração")` inserida no fluxo de boot, ou uma
   condição real que dispare panic).
2. Rodar `cargo run` novamente.
3. **Esperado**: em vez da mensagem de boas-vindas normal (ou logo após,
   dependendo de onde o panic foi inserido), a tela mostra uma mensagem de
   erro legível derivada do `PanicInfo`, e a execução para de forma estável
   (sem loop de reinício, sem tela preta).

**Critério de sucesso** (SC-004): a mensagem de panic é legível e a tela não
trava sem explicação nem reinicia sozinha.

## Cenário 3 — Reprodutibilidade a partir de checkout limpo (User Story 3)

1. Em uma máquina (ou diretório) limpa, clonar o repositório.
2. Seguir apenas o `README.md` para instalar pré-requisitos.
3. Rodar o único comando documentado (`cargo run`).

**Critério de sucesso** (SC-005): chega-se ao Cenário 1 sem nenhum passo,
comando ou informação que não esteja no `README.md`.

## Notas

- Não há suíte de testes automatizada obrigatória nesta versão (ver decisão
  de Testing em `research.md`) — este `quickstart.md` **é** o método de
  aceitação.
- Qualquer falha em um dos três cenários acima indica que a implementação
  ainda não satisfaz a spec e não deve ser considerada pronta para a
  demonstração ao vivo.
