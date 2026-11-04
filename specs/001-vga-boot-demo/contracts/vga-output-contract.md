# Contract: Saída de Texto VGA (tela vista pela plateia)

Este projeto não expõe uma API de código para outros sistemas — sua única
"interface externa" é a tela que a plateia vê no QEMU. Este documento fixa o
contrato observável dessa tela, para orientar tarefas de implementação e a
verificação manual em `quickstart.md`.

## Estado 1: Boot bem-sucedido (fluxo normal)

**Pré-condição**: `cargo run` executado em ambiente com pré-requisitos
instalados; QEMU inicia a máquina virtual.

**Comportamento garantido**:
1. A tela é limpa (sem lixo de boot BIOS remanescente) antes de qualquer
   texto do kernel aparecer.
2. Uma mensagem de boas-vindas fixa (definida em `src/main.rs` ou
   `src/vga_buffer.rs`, não configurável em tempo de execução) é escrita a
   partir da primeira posição da tela (linha 0, coluna 0).
3. A mensagem usa uma paleta de cores fixa: uma cor de primeiro plano e uma
   de fundo, definidas no código-fonte.
4. Se a mensagem ultrapassar 80 colunas, o texto continua na próxima linha
   automaticamente (sem cortar ou sobrescrever caracteres).
5. Se o conteúdo ultrapassar 25 linhas, a tela rola: a linha mais antiga
   "sai" por cima, todas as linhas sobem uma posição, e a nova linha aparece
   vazia na base antes de receber o próximo caractere.
6. Depois de escrita a mensagem, a tela permanece estática (o kernel não
   limpa nem re-escreve nada sozinho) até o QEMU ser fechado pelo instrutor.

**Garantia observável (o que a plateia vê)**: uma tela em modo texto, com
cores diferentes do preto-e-branco de um terminal comum, mostrando uma frase
fixa relacionada ao projeto (ex.: menção a "proto-os" e à ausência de sistema
operacional) — nunca uma tela preta permanente, nunca um terminal de
Linux/Windows reconhecível.

## Estado 2: Panic proposital

**Pré-condição**: o binário chega a um ponto de `panic!` (disparado
propositalmente para fins didáticos, ex.: `panic!("...")` em um trecho de
código de demonstração, ou uma condição real de erro irrecuperável).

**Comportamento garantido**:
1. A execução normal do kernel é interrompida imediatamente no ponto do
   panic.
2. Uma mensagem de erro legível é escrita na tela usando o mesmo mecanismo de
   texto VGA (não um código numérico opaco, não uma tela preta, não um
   reinício automático da máquina).
3. A mensagem inclui, no mínimo, texto humano-legível derivado do
   `PanicInfo` (o que causou o panic, e idealmente onde no código-fonte).
4. Após escrever a mensagem, o sistema para de forma segura (não entra em
   loop de reinício, não corrompe a tela com escrita repetida).

**Garantia observável**: a plateia vê uma mensagem de erro compreensível na
tela do QEMU, diferente da mensagem de boas-vindas, sem a janela travar de
forma não responsiva antes de mostrar o texto e sem reiniciar sozinha.

## Fora do contrato (explicitamente não garantido nesta versão)

- Qualquer resposta a entrada de teclado/mouse — não há entrada nesta
  versão; a tela nunca muda em reação a ações do usuário.
- Qualquer saída gráfica (imagens, formas, fontes customizadas) — apenas
  texto em modo VGA padrão.
- Persistência do conteúdo da tela entre execuções — cada `cargo run` começa
  do zero.
