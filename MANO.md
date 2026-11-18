[ENGLISH VERSION](MANO.en.md)

# `mano`: o editor de textos do os-rust

O `mano` é um editor de textos de tela cheia, simples, no jeito do `nano`. Ele é um
**programa de usuário** (roda em ring 3, como `hello` ou `teclas`) e vem embutido na
imagem de boot: `cargo run` e ele já está pronto, sem copiar arquivo, sem FTP e sem
depender do `/disco`. Para entender como ele foi escrito, leia o
[`WALKTHROUGH-MANO.md`](WALKTHROUGH-MANO.md).

## O que ele é, e o que não é

O `mano` faz o básico: digitar, apagar, mover o cursor, cortar e colar linhas, buscar,
buscar e substituir, inserir um arquivo, gravar e sair. Os atalhos são os do `nano`
sempre que o teclado do os-rust permite.

Ele **não** tem desfazer, seleção de texto, copiar, justificar, corretor ortográfico,
vários arquivos ao mesmo tempo, busca para trás, busca sem diferenciar maiúsculas,
expressões regulares, recuo automático nem quebra de linhas longas. Não usa Alt, F1 a
F12 nem Caps Lock, porque o teclado do kernel não entrega essas teclas.

## Como abrir

```text
os-rust> run mano                    (texto novo, sem nome)
os-rust> exec mano                   (o mesmo)
os-rust> exec mano /ram/nota.txt     (abre o arquivo; se não existe, começa um novo com esse nome)
```

- O `run` não passa argumentos; para abrir um arquivo use o `exec`.
- O caminho é **absoluto** (começa com `/`), tem até **40 bytes** e segue o formato do
  sistema de arquivos do os-rust: nomes 8.3, no máximo 8 componentes
  (`/ram/nota.txt`, `/disco/docs/leiame.txt`). Não existe diretório atual.
- Se o arquivo não existe, o `mano` abre um texto vazio **com aquele nome** e mostra
  `Novo arquivo`. O arquivo só passa a existir quando você grava.
- Uma palavra só: `exec mano /ram/a /ram/b` dá `mano: uso: exec mano [arquivo]`.
- Rode o `mano` **sozinho**: ele usa a tela inteira, e o kernel só tem um cursor.
  Dois programas que desenham ao mesmo tempo misturam a tela.

Erros de uso (caminho relativo, caminho maior que 40 bytes, diretório, arquivo grande
demais, caminho fora do formato) aparecem como **uma linha** `mano: <mensagem>`, o
kernel mostra `[run] mano terminou com codigo 1`, e o prompt volta normal. Nenhum
arquivo é criado nem alterado.

## A tela

```text
 mano  /ram/nota.txt                                              Modificado        <- linha 0: título
um                                                                                  <- linhas 1 a 21:
dois                                                                                   o texto
tres
...
                                                                                    <- linha 22: mensagens
^G Ajuda     ^O Gravar    ^W Buscar    ^K Cortar    ^C Posicao   ^X Sair            <- linha 23: atalhos
^R Ler       ^\ Subst.    ^U Colar     ^_ Linha     ^Y Pag ant   ^V Pag seg         <- linha 24: atalhos
```

| Região | Linhas | O que mostra |
|---|---|---|
| Barra de título | 0 | ` mano  ` e o nome do arquivo (ou `Novo buffer`), em vídeo reverso; `Modificado` à direita quando o texto mudou desde a última gravação |
| Texto | 1 a 21 | 21 linhas de 80 colunas; o trecho achado numa busca aparece em vídeo reverso |
| Mensagens | 22 | a mensagem do momento (some na próxima tecla), ou a pergunta e o campo que você digita |
| Atalhos | 23 e 24 | seis atalhos por linha, com a tecla em vídeo reverso |

As linhas longas **não** quebram: se o cursor passa da coluna 79, a janela anda na
horizontal o mínimo necessário (todas as linhas juntas). Ao voltar para o começo da
linha, ela volta.

## Os atalhos

`Ctrl+X` significa "segure Ctrl e aperte X". Com Ctrl, a letra vale como minúscula.

| Tecla | O que faz |
|---|---|
| caractere imprimível | insere na posição do cursor |
| `Enter` | parte a linha no cursor (sem recuo automático) |
| `Backspace`, `Ctrl+H` | apaga à esquerda; no começo de uma linha junta com a anterior |
| `Delete`, `Ctrl+D` | apaga sob o cursor; no fim de uma linha junta com a seguinte |
| `Tab` | insere espaços até a próxima coluna múltipla de 4 (o `mano` nunca grava o byte `0x09`) |
| setas, `Ctrl+B`, `Ctrl+F`, `Ctrl+P`, `Ctrl+N` | movem o cursor (esquerda, direita, acima, abaixo) |
| `Home`, `Ctrl+A` | começo da linha |
| `End`, `Ctrl+E` | fim da linha |
| `PageUp`, `Ctrl+Y` | 20 linhas para cima |
| `PageDown`, `Ctrl+V` | 20 linhas para baixo |
| `Ctrl+_`, `Ctrl+/` | ir para uma linha (pergunta o número) |
| `Ctrl+C` | mostra `linha L/T, coluna C/N`; dentro de um campo ou pergunta, cancela |
| `Ctrl+K` | corta a linha inteira |
| `Ctrl+U` | cola as linhas cortadas, acima da linha do cursor |
| `Ctrl+W` | buscar |
| `Ctrl+\`, `Ctrl+T` | buscar e substituir |
| `Ctrl+R` | inserir um arquivo na posição do cursor |
| `Ctrl+O` | gravar (pergunta o nome) |
| `Ctrl+S` | gravar (sem perguntar, se o texto já tem nome) |
| `Ctrl+X` | sair |
| `Ctrl+G` | ajuda |

Qualquer outra tecla (`Insert`, `Esc` fora de um campo, `Ctrl+Z`, ...) **não faz nada**.

**Por que `Ctrl+T`?** `Ctrl+\` exige a barra invertida, que fica em lugares diferentes
conforme o teclado físico, e o kernel só entende o layout US, por posição de tecla. O
`Ctrl+T` faz o mesmo e está sempre à mão. (No `nano`, `Ctrl+T` faz outra coisa, que o
`mano` não tem.) Para ir a uma linha há `Ctrl+_` (Shift e hífen) e `Ctrl+/`.

### Cortar e colar

`Ctrl+K` corta a linha do cursor. Vários `Ctrl+K` **seguidos** acumulam as linhas na
área de transferência, na ordem; qualquer outra tecla encerra a sequência, e o próximo
`Ctrl+K` começa uma área nova. `Ctrl+U` cola as linhas acima da linha do cursor e **não
esvazia** a área: dá para colar várias vezes. Cortar a única linha de um texto a deixa
vazia; `Ctrl+K` num texto que já é uma única linha vazia não faz nada.

## Buscar

1. `Ctrl+W`. A linha 22 mostra `Buscar: `.
2. Digite o termo e aperte `Enter`.

A busca procura o texto **exato** (maiúscula e minúscula são diferentes), para a frente,
a partir de **um byte depois do cursor**, dando no máximo uma volta no arquivo. Achou: o
cursor vai ao começo do trecho, que fica em vídeo reverso até a próxima tecla. Passou do
fim e voltou ao começo: `Busca reiniciada`. Não achou: `"<termo>" nao encontrado`.

Para achar a **próxima** ocorrência, aperte `Ctrl+W` e `Enter` com o campo vazio: o
prompt mostra o último termo (`Buscar [dois]: `) e `Enter` o repete.

Exemplo, com `um`, `dois`, `tres`, `dois`: `Ctrl+W`, `dois`, `Enter` leva à linha 2;
`Ctrl+W`, `Enter` leva à linha 4; de novo, volta à linha 2 com `Busca reiniciada`.

## Buscar e substituir

1. `Ctrl+\` (ou `Ctrl+T`).
2. O termo (o último aparece entre colchetes; `Enter` vazio o usa).
3. `Substituir por: `, o texto novo (pode ser vazio).

O `mano` percorre o arquivo **inteiro, do começo ao fim, uma vez**, não importa onde
estava o cursor. Em cada ocorrência, mostra-a em vídeo reverso e pergunta:

```text
Substituir este? (S sim, N nao, A todos, Ctrl+C cancela)
```

`S` troca esta; `N` pula; `A` troca esta e todas as seguintes sem perguntar; `Ctrl+C` ou
`Esc` param (o que já foi trocado **fica**). O texto novo **nunca é buscado de novo**: `a`
por `aa` termina. No fim: `Substituicoes: N` ou `Nenhuma ocorrencia`.

Exemplo, com `gato`, `casa gato`, `rato`: `Ctrl+\`, `gato`, `Enter`, `cao`, `Enter`, `S`,
`N` troca só a primeira e mostra `Substituicoes: 1`.

## Gravar e sair

- **`Ctrl+O`** pergunta o nome (já preenchido com o atual). Se o nome é **diferente do
  atual** e o arquivo **já existe**, pergunta `Arquivo existe. Sobrescrever? (S sim, N
  nao)`. Ao gravar mostra `Gravadas N linhas`, onde N é o número de quebras de linha
  gravadas. **`Ctrl+S`** grava direto, sem perguntar, se o texto já tem nome.
- **`Ctrl+X`** sai na hora se nada mudou. Se mudou, pergunta `Gravar o texto alterado? (S
  sim, N nao, Ctrl+C cancela)`: `S` grava e sai (se o texto não tem nome, pergunta o nome;
  só sai se gravou), `N` sai sem gravar, `Ctrl+C` ou `Esc` voltam a editar.
- Ao sair, o `mano` limpa a tela e o prompt volta como depois de `clear`.

**O `\n` do fim.** Ao abrir, o arquivo é dividido em cada `\n`. Ao gravar, as linhas são
unidas por `\n` e, se a última não é vazia, ganha um `\n` no fim (como no `nano`). Por isso
um arquivo vazio ou terminado em `\n` volta **idêntico**, byte a byte, e um arquivo que
não terminava em `\n` ganha um. Um arquivo de 0 bytes continua com 0 bytes
(`Gravadas 0 linhas`). Um texto que tem só `ola` grava `ola\n`.

**Gravar zera o arquivo antes de escrever.** O sistema de arquivos do os-rust não tem
gravação atômica: se a gravação falhar no meio (por exemplo, volume cheio), o arquivo pode
ficar menor ou vazio. O texto continua inteiro na tela e continua `Modificado`, então
você pode tentar de novo, em outro lugar.

## Limites

- O texto tem no máximo **65536 bytes** (contando como o arquivo ficaria gravado, com um
  `\n` por linha) e **4096 linhas**. Um arquivo maior é recusado ao abrir. Uma digitação,
  colagem, inserção de arquivo ou substituição que passaria do limite é recusada com
  `Limite de 65536 bytes atingido` (ou `... 4096 linhas ...`) e **nada muda**.
- Caminhos de até **40 bytes**, absolutos, nomes 8.3.
- Pelo teclado só entra **ASCII imprimível**. Os bytes do arquivo que o kernel não imprime
  (tabulação, o `\r` de um arquivo CRLF, acentos em UTF-8) **são preservados**, ocupam uma
  coluna cada e aparecem como o quadrado `■` (o byte `0xfe`); voltam iguais ao gravar.
- Sem Alt, sem desfazer, sem seleção.
- Um arquivo aberto por outro programa não pode ser gravado (`em uso`).

## Diferenças em relação ao `nano`

| `nano` | `mano` |
|---|---|
| desfazer, refazer, seleção, copiar | não existem |
| `Alt+...`, `F1` a `F12` | não existem (o kernel não entrega) |
| `Ctrl+T` executa comandos | `Ctrl+T` faz o mesmo que `Ctrl+\` (buscar e substituir) |
| busca com opções (maiúsculas, para trás, expressões regulares) | só texto exato, para a frente |
| substituir pergunta uma a uma, ou todas | igual (`S`, `N`, `A`) |
| quebra suave de linhas longas | a janela rola na horizontal |
| vários arquivos abertos | um só |
| abre qualquer tamanho | até 65536 bytes e 4096 linhas |

## Problemas comuns

- **`mano: use um caminho absoluto, como /ram/nota.txt`**: não há diretório atual. Escreva
  o caminho inteiro, começando com `/`.
- **`Erro ao gravar: em uso`**: outro programa tem o arquivo aberto. Espere ele terminar.
- **`Erro ao gravar: sem espaco`**: o volume está cheio. O arquivo pode ter ficado menor;
  o texto continua na tela.
- **`Erro ao gravar: caminho invalido`**: o nome tem componente maior que 8 caracteres,
  extensão maior que 3 ou caractere inválido.
- **O `/disco` esquece o que se grava.** O `cargo run` roda o QEMU com `snapshot=on`: o que
  se grava no `/disco` some quando o QEMU fecha. Para preservar, veja "Preservando o disco
  entre boots" no [README](README.md).
- **O `Ctrl+\` não funciona no meu teclado.** O kernel entende o layout US, por posição de
  tecla. Use `Ctrl+T`.
- **Os acentos viram `■`.** O VGA em modo texto só mostra ASCII. Os bytes ficam intactos no
  arquivo.

## Levar um arquivo editado para fora

O `/ram` volta ao inicial a cada boot. Para tirar um arquivo do os-rust, use o servidor FTP:
a seção "Servidor FTP" do [README](README.md) mostra como baixar `/ram/nota.txt` com `curl`.
