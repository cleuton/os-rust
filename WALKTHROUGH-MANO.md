[ENGLISH VERSION](WALKTHROUGH-MANO.en.md)

# Como o `mano` funciona, por dentro

Este documento explica `programs/src/bin/mano.rs`, o editor de textos do os-rust, para quem
sabe programar mas nunca escreveu um editor. O manual de uso está em [`MANO.md`](MANO.md);
aqui o assunto é o código. O arquivo tem cerca de mil linhas e está na mesma ordem deste
texto. Todo nome de função citado entre crases com parênteses, como `divide()`, existe no
arquivo.

## 1. Por que é um programa de usuário

Um editor não precisa de nenhum privilégio: ele lê teclas, escreve na tela e lê e grava
arquivos, e tudo isso o kernel já oferece a qualquer programa pelas syscalls do contrato
(`SYSCALLS.md`, versão 9). Por isso o `mano` **não mudou o kernel**: roda em ring 3, como o
`hello`, e um erro dele (um `panic`, uma falta de memória) encerra só ele.

O programa usa, pela biblioteca de runtime (`runtime/`), estas syscalls:

| Para quê | Na biblioteca | Syscall |
|---|---|---|
| esperar uma tecla (setas e Ctrl inclusive), sem eco | `read_key` | `SYS_READ_KEY` |
| limpar a tela, mover o cursor, escolher a cor | `screen::clear`, `screen::move_to`, `screen::set_color` | `SYS_SCREEN` |
| escrever os bytes de uma linha de tela | `runtime::sys::write` | `SYS_WRITE` |
| ler os argumentos do `exec` | `args::read` | `SYS_ARGS` |
| abrir e ler um arquivo | `File::open`, `File::read` | `SYS_OPEN`, `SYS_READ`, `SYS_CLOSE` |
| gravar um arquivo do zero | `FileWriter::overwrite`, `write_all` | `SYS_OPEN_WRITE`, `SYS_WRITE_FILE` |

O programa começa com `#![no_std]` e `#![no_main]`, usa `extern crate alloc` para ter `Vec` e
`String` (o runtime fornece o alocador) e liga o `main` ao ponto de entrada com `entry!(main)`.

## 2. O estado do editor

Tudo o que o editor sabe cabe em uma `struct Editor`:

- `linhas`: o texto, como uma lista de linhas (`Vec<Vec<u8>>`), cada uma **sem** o `\n`.
  Nunca está vazia: um texto sem nada é uma lista com uma linha vazia. As linhas guardam
  **bytes**, não caracteres, para o editor aguentar qualquer arquivo.
- `soma`: a soma dos tamanhos das linhas, mantida a cada edição, para medir o texto sem
  somar tudo de novo.
- `cy` e `cx`: a linha e a coluna (em bytes) do cursor. `cx` pode valer o tamanho da linha,
  que é o cursor logo depois do último byte.
- `desejada`: a coluna a que `Up` e `Down` tentam voltar. Se você está na coluna 10 e desce
  para uma linha de 3 colunas, o cursor vai à coluna 3; ao descer de novo, ele volta à 10.
  Só as teclas de movimento horizontal, as de edição, a busca e o salto de linha a mudam.
- `topo` e `esquerda`: a primeira linha e a primeira coluna visíveis (as duas janelas).
- `nome`, `modificado` e `mensagem`: o caminho atual (se há), o aviso `Modificado` e o texto
  da linha 22, que dura até a próxima tecla.
- `ultimo`: o último termo buscado ou substituído, lembrado até o fim do programa.
- `area` e `corte_seguido`: a área de transferência (as linhas cortadas) e a marca de que a
  tecla anterior foi um `Ctrl+K` que cortou.
- `destaque`: o trecho (linha, coluna, tamanho) que aparece em vídeo reverso.

Os demais tipos são pequenos: `Fluxo` (continua ou sai), `Limite` (qual limite do texto uma
operação estouraria), `Resposta` (`Sim`, `Nao`, `Todos`, `Cancela`), `Rodape` (qual quadro de
atalhos desenhar) e `ErroAbrir` (por que um arquivo não abriu).

`novo()` monta um editor com o texto dado, o cursor no começo e nada modificado.

## 3. O laço principal

O `main` faz, nesta ordem:

1. Lê os argumentos com `args::read`. Nenhuma palavra: texto novo. Uma: é o arquivo. Mais de
   uma: erro de uso. Os erros de uso (`uso()` escreve `mano: <mensagem>` e devolve o código 1)
   são decididos **antes** de qualquer chamada de tela, para o prompt voltar limpo.
2. Abre o arquivo com `abre()`. Se ele não existe, começa um texto vazio com aquele nome e a
   mensagem `Novo arquivo` (como o `nano`); o arquivo só passa a existir quando for gravado.
3. Limpa a tela e desenha.
4. Repete: espera uma tecla (`read_key` bloqueia sem gastar CPU), passa a tecla a `trata()`,
   ajusta as janelas com `rola()` e redesenha com `redesenha()`.
5. Ao sair, `screen::reset_color` e `screen::clear` deixam a tela limpa e na cor padrão, e o
   `main` devolve 0.

Os campos de digitação e as perguntas de uma tecla **não são estados**: `campo()` e
`pergunta()` são laços aninhados que também chamam `read_key` e desenham, e só voltam quando há
uma resposta. Assim `Ctrl+O` é, em essência, "chame `campo()`, e com o que voltar, chame
`grava()`", sem máquina de estados.

## 4. Desenhar a tela, linha a linha

O kernel **não apaga nada sozinho** e só mostra ASCII imprimível. Por isso o editor
redesenha as 25 linhas inteiras a cada tecla, sem nenhuma otimização, e cada linha é
completada com espaços até a coluna 79 para não sobrar resto da tecla anterior.

`desenha()` pinta, de cima para baixo:

- `desenha_titulo()`: a linha 0, 80 bytes em vídeo reverso: ` mano  `, o nome (ou `Novo
  buffer`) e, se o texto mudou, `Modificado` nas colunas 69 a 78.
- `desenha_texto()`: as linhas 1 a 21. Cada uma é uma janela de 80 bytes na pilha
  (`[u8; 80]`), preenchida com espaços (`em_branco()`) e com o pedaço da linha do texto a
  partir de `esquerda` (`copia()`), e escrita **de uma vez**. Se a linha tem o `destaque`, ela
  sai em até três escritas: antes, o trecho em vídeo reverso, e depois.
- `desenha_mensagem()`: a linha 22, em amarelo.
- `desenha_rodape()`: as linhas 23 e 24, dois quadros de atalhos. `desenha_quadro()` escreve
  cada atalho em 13 colunas, a tecla em vídeo reverso (`^O`) e o rótulo em cinza claro.

Por fim `desenha()` leva o cursor de hardware ao lugar (`vai_para()`): no texto, ou no fim do
campo, na linha 22.

**Por que a linha 24 nunca passa da coluna 79.** O kernel só quebra a linha quando chega um
caractere novo com a coluna já em 80. Preencher a coluna 79 da linha 24 não rola a tela, mas o
81º byte rolaria, e a tela inteira subiria uma linha. Por isso toda linha tem **exatamente** 80
bytes e `desenha_quadro()` completa com espaços até a coluna 79, nunca além.

**Bytes que não são texto.** O arquivo pode ter qualquer byte (tabulação, o `\r` de um arquivo
CRLF, UTF-8). O editor escreve os bytes crus com `escreve()` (a syscall `write`, e não o
`print!`, que só aceita texto) e quem troca o que não é ASCII imprimível pelo quadrado `0xfe` é o
kernel. Cada byte ocupa uma coluna, então as contas de coluna do editor continuam certas.

As cores são os três pares de constantes `CORPO`, `REVERSO` e `MENSAGEM`, escolhidas com `cor()`.

## 5. O despacho de teclas e como o Ctrl chega

`read_key` devolve um `KeyEvent`: a tecla (`Key::Char('a')`, `Key::Enter`, `Key::Left`, ...) e um
booleano `ctrl`. **O Ctrl chega como uma letra minúscula mais um bit**: `Ctrl+X` é
`Key::Char('x')` com `ctrl: true`. Por isso `Ctrl+H` e `Backspace` nunca se confundem (uma é
`Char('h')` com `ctrl`, a outra é `Key::Backspace`), e `Ctrl+\`, `Ctrl+_` e `Ctrl+/` chegam como
`Char('\\')`, `Char('_')` e `Char('/')` com `ctrl`.

`trata()` é um `match` sobre o par (tecla, ctrl), um grupo de teclas por vez: texto, movimento,
cortar e colar, busca e substituição, arquivo. Tudo o que não está na tabela cai em `_ => {}`:
**não faz nada**. Antes do `match`, `trata()` limpa a mensagem e o destaque (duram até a próxima
tecla) e guarda se a tecla anterior foi um corte (só o `Ctrl+K` continua uma sequência de cortes).

## 6. Editar linhas: inserir, partir, juntar

- `insere()`: `Vec::insert` do byte em `cx`, e o cursor avança.
- `tabula()`: insere de 1 a 4 espaços, até a próxima coluna múltipla de 4. O editor nunca insere
  o byte `0x09`, porque o kernel o mostraria como `0xfe`.
- `parte_linha()` (`Enter`): `split_off(cx)` corta a linha no cursor e o resto vira uma linha nova
  logo abaixo.
- `apaga_esquerda()` e `apaga_direita()`: apagam um byte; no começo (ou no fim) da linha,
  **juntam** com a vizinha (`remove` da linha e `extend_from_slice` na outra). No começo (ou no
  fim) do arquivo não fazem nada, e **não** marcam o texto como modificado.

O texto só fica `Modificado` quando a lista de linhas muda de verdade.

## 7. Mover o cursor e rolar

`esquerda()`, `direita()`, `inicio()` e `fim()` mexem em `cx` (e em `cy` nas bordas) e definem a
coluna desejada. `cima()`, `baixo()` e `pagina()` mexem em `cy` e levam `cx` a
`min(desejada, tamanho da linha)` **sem mudar `desejada`**; `PageUp` e `PageDown` andam 20
linhas, limitados ao primeiro e ao último.

`rola()` anda as janelas **o mínimo**: se o cursor saiu por cima, `topo = cy`; se saiu por baixo,
`topo = cy - 20`; o mesmo, na horizontal, com `esquerda` e a coluna 79. Não há quebra de linha:
uma linha de 200 colunas só aparece pedaço por pedaço.

`mostra_posicao()` (`Ctrl+C`) e `vai_para_linha()` (`Ctrl+_`) completam o grupo.

## 8. Cortar e colar com acúmulo

`corta_linha()` tira a linha do cursor e a põe em `area`. Se a tecla anterior também foi um corte
(`corte_seguido`), a linha **acrescenta**; senão, `area` é esvaziada antes: é isso que faz três
`Ctrl+K` seguidos formarem uma sequência só. `cola()` **clona** as linhas de `area` para cima da
linha do cursor e não esvazia a área, então dá para colar várias vezes.

## 9. Buscar

`busca()` pede o termo com `pede_termo()` (que mostra o último termo entre colchetes e, com o campo
vazio, o reusa) e procura com `acha()`, que acha o termo numa linha a partir de uma coluna. A busca
começa **um byte depois do cursor** (senão o `Ctrl+W` repetido acharia sempre o mesmo trecho): o
resto da linha atual, depois as linhas seguintes e, se nada foi achado, uma volta a partir do
começo, com a mensagem `Busca reiniciada`. Achou: o cursor vai ao começo do trecho e o `destaque`
o mostra em vídeo reverso até a próxima tecla.

## 10. Substituir

`substitui()` percorre o texto **uma vez, do começo ao fim**, com um par (linha, coluna). Em cada
ocorrência, o cursor vai a ela, o `destaque` a marca e `pergunta()` decide. Depois de uma troca, a
próxima busca na mesma linha começa em `coluna + tamanho do texto novo`: **o texto novo nunca é
buscado de novo**. É isso que faz `a` por `aa` terminar (cada `a` original vira `aa` uma vez) e que
mantém certas várias ocorrências na mesma linha, mesmo que o texto novo tenha outro tamanho. `A`
liga um indicador que dispensa as perguntas seguintes. Antes de cada troca o editor confere o
limite do texto (seção 12): se estourar, o percurso **termina** com a mensagem de limite.

## 11. Abrir e gravar arquivos

**Abrir.** `le_arquivo()` lê o arquivo **inteiro** em pedaços de 4096 bytes (o máximo de um `read`)
e o fecha ao sair da função, pois fechar antes de gravar é obrigatório: gravar num arquivo ainda
aberto dá `ERR_BUSY`. Depois `divide()` corta os bytes em cada `\n`, **sem descartar nada**:
`a\nb\n` vira `a`, `b` e uma linha vazia; um arquivo vazio vira uma linha vazia. `abre()` junta as
duas coisas e confere os limites.

**Gravar.** `grava()` valida o nome (`nome_valido()`: começa com `/`) antes de abrir qualquer coisa;
se o nome é diferente do atual, abre o arquivo para ver se existe (e pergunta antes de
sobrescrever); monta os bytes com `junta()` (as linhas unidas por `\n`, e um `\n` no fim se a
última linha não é vazia) e grava com `grava_arquivo()`, que usa `FileWriter::overwrite` e
`write_all`. `grava_pedindo_nome()` (`Ctrl+O`) e `grava_direto()` (`Ctrl+S`) escolhem se pergunta o
nome. `sai()` (`Ctrl+X`) pergunta antes de descartar alterações.

**O `\n` final e o arquivo vazio.** Como `divide()` e `junta()` são inversas, todo arquivo vazio ou
terminado em `\n` volta idêntico; um que não terminava em `\n` ganha um. Um texto de uma linha
vazia grava 0 bytes.

**O risco de gravar sobre um arquivo zerado.** `FileWriter::overwrite` zera o arquivo **antes** de
gravar, e o os-rust não tem gravação atômica (arquivo temporário e troca). Se a gravação falhar no
meio, o arquivo fica menor ou vazio. A defesa do editor é manter o texto inteiro na memória e
**não** limpar a marca `Modificado` enquanto a gravação não terminar bem.

## 12. Os limites e por que existem

O texto tem no máximo 65536 bytes **gravados** e 4096 linhas (as constantes `MAX_BYTES` e
`MAX_LINHAS`). Existem para o editor caber no heap de 1 MiB do programa com folga (o texto, a área
de transferência e os buffers de leitura e gravação juntos somam bem menos que isso) e para um
arquivo nunca ser aberto sem poder ser regravado.

`medida()` calcula o tamanho gravado (a soma das linhas, um `\n` entre elas e outro no fim se a
última linha não é vazia), e `cabe()` o confere. **Toda operação que aumenta o texto** (digitar,
`Enter`, `Tab`, colar, inserir arquivo, substituir) calcula o tamanho do texto que **resultaria** e
chama `cabe()` **antes** de mudar qualquer coisa; se estourar, `recusa()` mostra a mensagem (`Limite
de 65536 bytes atingido` ou `Limite de 4096 linhas atingido`) e nada muda.

## 13. Mapa função por função

| Função | O que faz |
|---|---|
| `divide()` | corta os bytes de um arquivo em linhas, em cada `\n` |
| `junta()` | une as linhas por `\n`, mais um `\n` final se a última não é vazia |
| `medida()` | o tamanho que o texto teria gravado |
| `cabe()` | confere os dois limites sobre o texto que resultaria |
| `acha()` | acha o termo numa linha, a partir de uma coluna |
| `nome_valido()` | o caminho começa com `/` |
| `texto_do_limite()` | a mensagem de um limite estourado |
| `le_arquivo()` | lê o arquivo inteiro e o fecha |
| `abre()` | lê, divide e confere os limites |
| `grava_arquivo()` | grava os bytes do zero |
| `escreve()`, `vai_para()`, `cor()` | os três gestos de tela: escrever bytes, mover o cursor, escolher a cor |
| `em_branco()`, `copia()` | a janela de 80 bytes e o preenchimento dela |
| `desenha_mensagem()`, `desenha_quadro()`, `desenha_rodape()` | a linha 22 e os quadros de atalhos |
| `desenha_linha_de_ajuda()`, `desenha_ajuda()` | a tela de `Ctrl+G` |
| `uso()` | escreve um erro de uso antes da tela e devolve 1 |
| `novo()` | cria o editor |
| `ultima_vazia()` | a última linha do texto é vazia? |
| `rola()` | anda as janelas o mínimo |
| `desenha_titulo()`, `desenha_texto()`, `desenha()`, `redesenha()` | as 25 linhas da tela |
| `campo()`, `pergunta()`, `pede_termo()` | os prompts: campo de texto, pergunta de uma tecla, termo de busca |
| `recusa()` | mostra qual limite foi atingido |
| `insere()`, `tabula()`, `parte_linha()` | caractere, `Tab`, `Enter` |
| `apaga_esquerda()`, `apaga_direita()` | `Backspace`, `Delete` (juntam linhas) |
| `esquerda()`, `direita()`, `cima()`, `baixo()`, `inicio()`, `fim()`, `pagina()` | movimento |
| `mostra_posicao()`, `vai_para_linha()` | `Ctrl+C`, `Ctrl+_` |
| `corta_linha()`, `cola()`, `insere_arquivo()` | `Ctrl+K`, `Ctrl+U`, `Ctrl+R` |
| `busca()`, `substitui()` | `Ctrl+W`, `Ctrl+\` e `Ctrl+T` |
| `grava()`, `grava_pedindo_nome()`, `grava_direto()`, `sai()` | gravar e sair |
| `ajuda()` | `Ctrl+G`: mostra a ajuda e espera uma tecla |
| `trata()` | o despacho de teclas |
| `main()` | argumentos, abrir, laço principal e saída |
