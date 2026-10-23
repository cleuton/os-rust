[ENGLISH VERSION](SYSCALLS.en.md)

# Contrato de syscalls do os-rust

**Versão do contrato**: 8

**Válido a partir de**: os-rust 0.12.0 (Marco 12)

Este documento é tudo que um programa precisa saber para rodar no os-rust.
O kernel e os programas nunca dependem de nada que não esteja aqui. Uma
mudança **incompatível** aumenta a versão do contrato e exige atualizar a
biblioteca de runtime dos programas (`runtime/`) no mesmo marco. Quem
escreve programas em Rust normalmente não chama as syscalls diretamente:
usa a biblioteca (ver `GUIA_DO_PROGRAMADOR.md`).

## 1. Formato do executável

- ELF64, little-endian, `e_type = ET_EXEC` (2), `e_machine = EM_X86_64` (62).
  Sem relocação, sem PIE, sem bibliotecas dinâmicas.
- Cada `PT_LOAD` deve ter `p_vaddr` alinhado a 4 KiB, `p_filesz ≤ p_memsz`
  (o que sobra é zerado, como `.bss`), e **não compartilhar página** com
  outro `PT_LOAD`. Os demais tipos de segmento são ignorados.
- Todos os `PT_LOAD` devem caber em `[0x4000_0000, 0x6000_0000)` (a faixa
  de código e dados, seção 2). Um segmento fora dela é recusado.
- Permissões: página gravável só se o segmento tem `PF_W`; executável só
  se tem `PF_X` (W^X). A pilha e o heap são graváveis e nunca executáveis.
- `e_entry` deve estar dentro de um segmento executável.
- O executável é embutido na imagem de boot em tempo de compilação, ou
  lido de um arquivo de um volume (seção 9), e executado com
  `run <alvo> [<alvo>...]`: vários programas podem rodar ao mesmo tempo
  (seção 8). Um executável lido de arquivo tem no máximo **65536 bytes**
  (64 KiB) e passa pelas mesmas validações; um arquivo que não é um ELF64
  válido, ou que passa desse tamanho, é recusado antes de qualquer
  mapeamento de memória.

## 2. Onde o programa vive

| Item | Valor |
|------|-------|
| Região do usuário | `[0x4000_0000, 0x8000_0000)` (1 GiB a 2 GiB) |
| Código e dados | O que os `PT_LOAD` declaram, dentro de `[0x4000_0000, 0x6000_0000)`. Convenção do projeto: base `0x4000_0000`. |
| Heap | `[0x6000_0000, 0x6010_0000)` (1 MiB no máximo), começa vazio e cresce com `SYS_ALLOC` (seção 5). |
| Pilha | 4 páginas (16 KiB) no fim da região: `[0x7FFF_C000, 0x8000_0000)`; a página abaixo não é mapeada (guarda). |
| Fora disso | Qualquer acesso (kernel, VGA, memória física, heap ainda não pedido, além do fim do heap) causa uma exceção e o programa é encerrado. |

Como a região está abaixo de 2 GiB, o programa pode ser compilado com o
code model padrão do Rust (`R_X86_64_32S` cabe).

## 3. Estado na primeira instrução

| Registrador | Valor |
|-------------|-------|
| `rip` | `e_entry` |
| `rsp` | **`0x7FFF_FFF8`** (topo da região − 8): alinhamento de entrada de função da ABI System V, como se `_start` tivesse sido chamada; `[rsp]` **não** contém endereço de retorno válido. |
| `rflags` | `0x202` (`IF = 1`, resto zero) |
| `rcx` | `e_entry` (o kernel o coloca no contexto inicial, como se o programa tivesse acabado de voltar de um `syscall` em `e_entry`) |
| `r11` | `0x202` (idem: o `rflags` desse retorno) |
| Todos os demais (`rax`, `rbx`, `rdx`, `rsi`, `rdi`, `rbp`, `r8`–`r10`, `r12`–`r15`) | `0` |
| Segmentos | Código de usuário `0x23`, dados/pilha de usuário `0x1B` (definidos pelo kernel). |

Não há `argc`/`argv`/ambiente: nada na pilha inicial.
O programa **nunca retorna** de `_start`: deve chamar `exit`. (A
biblioteca de runtime cuida disso: o `main` do programa retorna e ela
chama `exit` com o valor devolvido.)

## 4. Como chamar

Instrução `syscall`. Convenção de registradores:

| Registrador | Na entrada | Na saída |
|-------------|-----------|----------|
| `rax` | número da syscall | resultado (ver §6) |
| `rdi` | argumento 1 | preservado |
| `rsi` | argumento 2 | preservado |
| `rdx` | argumento 3 | preservado |
| `rcx` | (ignorado) | **destruído** (a CPU guarda aqui o `rip` de retorno) |
| `r11` | (ignorado) | **destruído** (a CPU guarda aqui o `rflags`) |
| `r8`, `r9`, `r10` | (ignorados, reservados para argumentos 4 a 6) | preservados |
| `rbx`, `rbp`, `r12`–`r15`, `rsp` | | preservados |

Durante a syscall o kernel roda numa pilha própria; a pilha do programa
nunca é usada nem confiada. As interrupções ficam desligadas durante a
syscall: nenhuma troca de programa acontece no meio dela, e por isso cada
chamada é atômica em relação aos outros programas. O kernel pode devolver
`rcx` e `r11` intactos, mas o contrato não promete isso: um programa não
pode depender do valor deles na volta.

Exemplo, em Rust com `asm!`:

```rust
let ret: i64;
unsafe {
    core::arch::asm!(
        "syscall",
        inlateout("rax") 1u64 => ret,       // SYS_WRITE
        in("rdi") ptr, in("rsi") len,
        lateout("rcx") _, lateout("r11") _,
        options(nostack),
    );
}
```

## 5. Syscalls

Números do contrato v8. **Qualquer outro número** (inclusive `0`)
encerra o programa (§7).

| Nº | Nome | Argumentos | Resultado |
|----|------|-----------|-----------|
| 1 | `SYS_WRITE` | `rdi = ptr`, `rsi = len` | bytes escritos (`≥ 0`) ou erro (`< 0`) |
| 2 | `SYS_EXIT` | `rdi = code` | não retorna |
| 3 | `SYS_READ_LINE` | `rdi = ptr`, `rsi = len` | bytes escritos (`≥ 1`), `0` se `len == 0`, ou erro (`< 0`) |
| 4 | `SYS_ALLOC` | `rdi = size` | endereço do início da área nova (`> 0`) ou erro (`< 0`) |
| 5 | `SYS_YIELD` | nenhum | `0` |
| 6 | `SYS_OPEN` | `rdi = path_ptr`, `rsi = path_len` | descritor (`≥ 0`) ou erro (`< 0`) |
| 7 | `SYS_READ` | `rdi = fd`, `rsi = ptr`, `rdx = len` | bytes lidos (`≥ 0`; `0` = fim do arquivo) ou erro (`< 0`) |
| 8 | `SYS_CLOSE` | `rdi = fd` | `0` ou erro (`< 0`) |
| 9 | `SYS_READ_DIR` | `rdi = fd`, `rsi = ptr`, `rdx = len` | `1` (uma entrada escrita), `0` (fim do diretório) ou erro (`< 0`) |
| 10 | `SYS_TIME` | `rdi = ptr`, `rsi = len` | `0` (um `DateTime` escrito) ou erro (`< 0`) |
| 11 | `SYS_CREATE` | `rdi = path_ptr`, `rsi = path_len` | descritor para acrescentar (`≥ 0`) ou erro (`< 0`) |
| 12 | `SYS_OPEN_APPEND` | `rdi = path_ptr`, `rsi = path_len` | descritor para acrescentar (`≥ 0`) ou erro (`< 0`) |
| 13 | `SYS_WRITE_FILE` | `rdi = fd`, `rsi = ptr`, `rdx = len` | bytes gravados (`≥ 0`) ou erro (`< 0`) |
| 14 | `SYS_REMOVE` | `rdi = path_ptr`, `rsi = path_len` | `0` ou erro (`< 0`) |
| 15 | `SYS_MKDIR` | `rdi = path_ptr`, `rsi = path_len` | `0` ou erro (`< 0`) |
| 16 | `SYS_RMDIR` | `rdi = path_ptr`, `rsi = path_len` | `0` ou erro (`< 0`) |
| 17 | `SYS_TCP_LISTEN` | `rdi = port` | socket de escuta (`≥ 0`) ou erro (`< 0`) |
| 18 | `SYS_TCP_ACCEPT` | `rdi = sock` | socket novo (`≥ 0`) ou erro (`< 0`); bloqueia |
| 19 | `SYS_TCP_CONNECT` | `rdi = ip`, `rsi = port` | socket conectado (`≥ 0`) ou erro (`< 0`); bloqueia |
| 20 | `SYS_TCP_SEND` | `rdi = sock`, `rsi = ptr`, `rdx = len` | bytes aceitos (`≥ 0`) ou erro (`< 0`) |
| 21 | `SYS_TCP_RECV` | `rdi = sock`, `rsi = ptr`, `rdx = len` | bytes recebidos (`≥ 0`; `0` = o par fechou) ou erro (`< 0`) |
| 22 | `SYS_TCP_CLOSE` | `rdi = sock` | `0` ou erro (`< 0`) |
| 23 | `SYS_TCP_TIMEOUT` | `rdi = sock`, `rsi = ms` | `0` ou erro (`< 0`) |

### `SYS_WRITE` (1)

Escreve `len` bytes, a partir de `ptr`, na tela (VGA texto), no cursor
atual, pelo mesmo caminho do `print!` do kernel. Bytes ASCII imprimíveis
(`0x20`–`0x7e`) e `\n` aparecem como são; qualquer outro byte aparece como
o quadrado `0xfe`. Não exige UTF-8.

- `len == 0`: devolve `0` sem olhar `ptr`.
- `len > 4096`: devolve `ERR_INVAL`; nada é escrito.
- Todo o intervalo `[ptr, ptr + len)` deve estar dentro da região do
  usuário e cada página dele mapeada e acessível ao usuário; senão
  devolve `ERR_FAULT` e **nada é escrito**. O programa continua rodando.
- Sucesso: devolve `len`.

### `SYS_EXIT` (2)

Encerra o programa. `code` é registrado pelo kernel: `0` significa
sucesso e não mostra nada na tela; qualquer outro valor mostra
`[run] <nome> terminou com codigo <code>`. Só este programa termina: os
outros, se houver, continuam, e o prompt volta quando o **último** termina.
Todas as páginas do programa (código, dados, pilha e heap) são liberadas na
hora e nenhum estado do programa sobrevive à chamada.

### `SYS_READ_LINE` (3)

Espera a pessoa digitar uma linha no teclado e a entrega ao programa. O
programa fica **bloqueado** até o Enter e, enquanto espera, **não gasta
CPU**: os outros programas seguem rodando. Enquanto a linha é digitada, o
kernel mostra na tela o que é digitado (eco) e trata o Backspace. Com mais
de um programa esperando, a linha vai ao que pediu **primeiro** (seção 8).

- `len == 0`: devolve `0` sem esperar e sem olhar `ptr`.
- `len > 4096`: devolve `ERR_INVAL`; nenhuma tecla é consumida.
- Todo o intervalo `[ptr, ptr + len)` deve estar dentro da região do
  usuário e cada página dele mapeada, acessível ao usuário **e gravável**;
  senão devolve `ERR_FAULT` **antes de esperar**, sem consumir nenhuma
  tecla. O programa continua rodando.
- Aceita caracteres ASCII imprimíveis (`0x20`–`0x7e`), no máximo
  `min(len, 128) − 1`. O que passar disso é ignorado, sem eco (o prompt do
  kernel se comporta do mesmo jeito). Backspace apaga o último caractere,
  se houver. Qualquer outra tecla é ignorada.
- No Enter, o kernel avança a linha na tela e escreve em `ptr` os
  caracteres digitados **seguidos de `\n`** (`0x0a`). Devolve `n`, o total
  de bytes escritos: `1 ≤ n ≤ len`. Uma linha vazia devolve `1` (só o
  `\n`). Os bytes de `ptr + n` em diante não são tocados.
- Teclas digitadas que nenhum programa pediu ficam guardadas (até 16) e
  vão para o prompt quando o último programa termina (seção 8).

### `SYS_ALLOC` (4)

Dá memória ao programa: amplia o heap dele em `size` bytes.

- `size == 0`: devolve `ERR_INVAL`.
- `size` é arredondado para cima até um múltiplo de 4096. A área nova é
  zerada, legível e gravável, nunca executável.
- Devolve o endereço do início da área nova. As áreas de chamadas
  sucessivas são **contíguas**: a primeira começa em `0x6000_0000` e cada
  uma começa onde a anterior terminou.
- Se o total de páginas do programa passaria de 256 (1 MiB), ou se faltar
  memória física, devolve `ERR_NOMEM` e **nada é alocado** (uma chamada
  que falha não muda o heap). O programa continua rodando e pode tentar um
  pedido menor.
- Não existe liberar: a memória só volta ao kernel quando o programa
  termina. Reaproveitar blocos dentro do heap é trabalho do programa (a
  biblioteca de runtime faz isso).

### `SYS_YIELD` (5)

Cede a CPU voluntariamente. O kernel guarda o estado do programa e passa a
CPU ao próximo programa pronto, em rodízio; o programa volta a rodar, na
instrução seguinte ao `syscall`, quando chegar a vez dele. Se nenhum outro
programa está pronto, volta imediatamente ao próprio programa. Nunca falha e
não tem código de erro. Todos os registradores, exceto `rax` (que vale `0`
na volta), voltam com o valor que tinham.

Na biblioteca de runtime: `yield_now()`. Um programa não precisa chamá-la
para os outros rodarem: o timer interrompe quem não cede (seção 8).

### `SYS_OPEN` (6)

Abre um arquivo **ou diretório**, somente para leitura, e devolve o
descritor: um número de `0` a `3`, sempre o **menor livre**. A posição de
leitura começa em `0`. O caminho (formato na seção 9) é copiado pelo kernel
antes de ser interpretado.

- `path_len == 0`: devolve `ERR_INVAL`. `path_len > 64`: `ERR_NAMETOOLONG`.
  Nenhum dos dois olha `path_ptr`.
- O intervalo `[path_ptr, path_ptr + path_len)` deve estar dentro da região
  do usuário e mapeado; senão devolve `ERR_FAULT`. O programa continua.
- Caminho malformado (componente vazio, caractere inválido, nome fora de
  8.3, `.` ou `..`, ou sem `/` no começo): `ERR_INVAL`. Mais de 8
  componentes: `ERR_NAMETOOLONG`.
- Volume desconhecido ou indisponível: `ERR_NODEV`. Caminho inexistente:
  `ERR_NOENT`. Um arquivo no meio do caminho: `ERR_TYPE`.
- O programa já tem 4 arquivos abertos: `ERR_MFILE`.
- Erro ao ler o volume (disco que não responde, estrutura corrompida):
  `ERR_IO`.

Os descritores abertos por `SYS_CREATE` e `SYS_OPEN_APPEND` são **só para
acrescentar**: aceitam `SYS_WRITE_FILE` e `SYS_CLOSE`, e `SYS_READ` neles dá
`ERR_BADF`.

### `SYS_READ` (7)

Lê até `len` bytes do arquivo aberto em `fd`, a partir da posição dele, para
`ptr`, e avança a posição pelo que leu. Devolve o número de bytes lidos, que
só é menor que `len` no fim do arquivo (nunca passa do tamanho dele); `0`
quer dizer que o arquivo acabou. Um arquivo vazio devolve `0` já na primeira
chamada.

- `len == 0`: devolve `0` sem olhar `ptr`. `len > 4096`: `ERR_INVAL`.
- `[ptr, ptr + len)` deve estar dentro da região do usuário, mapeado e
  **gravável**; senão `ERR_FAULT`, antes de ler qualquer coisa do volume.
- `fd` fora de `0..=3`, nunca aberto ou já fechado: `ERR_BADF`.
- `fd` de um diretório: `ERR_TYPE`. `fd` aberto para acrescentar: `ERR_BADF`.
- Se a leitura falha depois de já ter entregue bytes nesta chamada, a chamada
  devolve os bytes entregues, e a **próxima** devolve o erro (`ERR_IO`).

### `SYS_CLOSE` (8)

Fecha o descritor e o libera para reuso. Fechar um descritor já fechado, ou
fora de `0..=3`: `ERR_BADF`.

### `SYS_READ_DIR` (9)

Escreve em `ptr` **uma** entrada do diretório aberto em `fd` (um
`DirEntryRaw`, 20 bytes, seção 9) e avança o cursor do diretório. Devolve `1`
quando escreveu uma entrada e `0` quando as entradas acabaram (nada é
escrito).

- `len < 20` ou `len > 4096`: `ERR_INVAL`.
- `[ptr, ptr + 20)` deve estar na região do usuário, mapeado e **gravável**;
  senão `ERR_FAULT`.
- `fd` inválido: `ERR_BADF`. `fd` de um arquivo (inclusive um aberto para
  acrescentar): `ERR_TYPE`.
- Entradas apagadas, de nome longo, rótulo de volume, `.` e `..` não
  aparecem.

### `SYS_TIME` (10)

Escreve em `ptr` a data e a hora atuais, **em UTC**, como um `DateTime`
(8 bytes, `repr(C)`). Devolve `0`. Cada chamada lê o relógio de novo; duas
chamadas seguidas nunca andam para trás.

Formato do buffer:

| Deslocamento | Tamanho | Campo |
|--------------|---------|-------|
| 0 | 2 | ano completo (por exemplo, 2026), little-endian |
| 2 | 1 | mês (1 a 12) |
| 3 | 1 | dia (1 até o último dia do mês, considerando ano bissexto) |
| 4 | 1 | hora (0 a 23) |
| 5 | 1 | minuto (0 a 59) |
| 6 | 1 | segundo (0 a 59) |
| 7 | 1 | zero (alinhamento) |

As verificações acontecem nesta ordem:

- `len != 8` (`TIME_SIZE`): `ERR_INVAL`.
- `[ptr, ptr + 8)` deve estar na região do usuário, mapeado e **gravável**;
  senão `ERR_FAULT`.
- O relógio devolveu valores impossíveis, não se estabilizou ou não
  respondeu: `ERR_CLOCK`. O programa não recebe data nenhuma nesse caso.

Não existe fuso horário: o resultado é sempre UTC. Na biblioteca de runtime:
`time::now()`.

### `SYS_CREATE` (11)

Cria um arquivo **vazio** e o abre para acrescentar, no menor descritor livre.
Os volumes `/ram` e `/disco` aceitam escrita. O caminho é tratado exatamente como em
`SYS_OPEN`; passadas as verificações de caminho, a ordem é:

1. Volume desconhecido ou indisponível: `ERR_NODEV`.
2. Volume que não aceita escrita (um volume com mais de 1 setor por cluster, ou
   um dispositivo que não sabe gravar): `ERR_ROFS`. Vem **antes** de olhar o
   caminho: qualquer caminho nesse volume dá `ERR_ROFS`, exista ou não.
3. Diretório pai (todos os componentes menos o último) inexistente:
   `ERR_NOENT`; um arquivo no meio do caminho: `ERR_TYPE`.
4. O nome já existe (arquivo, diretório ou a raiz do volume): `ERR_EXIST`.
5. O programa já tem 4 arquivos abertos: `ERR_MFILE`, **antes** de criar
   qualquer coisa: uma criação que falha nunca deixa um arquivo para trás.
6. O diretório pai não tem entrada livre (o diretório não cresce):
   `ERR_NOSPC`.
7. Erro ao ler ou gravar o volume, ou FAT inconsistente: `ERR_IO` (em qualquer
   passo).

A entrada nova tem atributo arquivo, nenhum cluster, tamanho `0` e data e hora
zeradas.

### `SYS_OPEN_APPEND` (12)

Abre um arquivo **existente** para acrescentar, no menor descritor livre.
Mesmas verificações de caminho, volume (`ERR_NODEV`, `ERR_ROFS`) e pai de
`SYS_CREATE`. Inexistente: `ERR_NOENT`. O alvo é um diretório (inclusive a raiz
do volume): `ERR_TYPE`. 4 arquivos abertos: `ERR_MFILE`. Erro de volume:
`ERR_IO`.

### `SYS_WRITE_FILE` (13)

Grava `len` bytes de `ptr` no **fim atual** do arquivo aberto em `fd`; o
tamanho é lido do volume a cada chamada. Verificações, nesta ordem:

- `len == 0`: devolve `0` sem olhar `ptr` nem `fd`. `len > 4096`: `ERR_INVAL`.
- `[ptr, ptr + len)` deve estar na região do usuário e mapeado (basta ser
  legível); senão `ERR_FAULT`, antes de gravar qualquer coisa.
- `fd` fora de `0..=3`, fechado, aberto só para leitura ou de um diretório:
  `ERR_BADF`.

Devolve o número de bytes gravados. É `len`, a menos que o volume acabe ou
falhe no meio: então devolve os bytes que gravou (`1 ≤ n < len`), e a
**próxima** chamada devolve o erro (`ERR_NOSPC` ou `ERR_IO`). Se nenhum byte
pôde ser gravado, a própria chamada devolve o erro. Ao voltar, os dados, a
cadeia da FAT e o tamanho já estão no volume: não há cache nem `flush`, e
`SYS_CLOSE` não grava nada.

### `SYS_REMOVE` (14)

Apaga um arquivo e devolve `0`. Mesmas verificações de caminho, volume e pai
de `SYS_CREATE`. Inexistente: `ERR_NOENT`. O alvo é um diretório (inclusive a
raiz): `ERR_TYPE`. Aberto por **qualquer** programa, inclusive o que chama:
`ERR_BUSY`, sem alterar nada. Cadeia de clusters inconsistente: `ERR_IO`, sem
alterar nada (um arquivo assim não pode ser apagado). Os clusters do arquivo
voltam a ficar livres.

### `SYS_MKDIR` (15)

Cria um diretório **vazio** e devolve `0`. Não abre nada: **não usa descritor**
e nunca devolve `ERR_MFILE`. O caminho é tratado exatamente como em
`SYS_CREATE`: `path_len == 0` é `ERR_INVAL` e `path_len > 64` é
`ERR_NAMETOOLONG` (nenhum dos dois olha o ponteiro); o intervalo precisa estar na
região do usuário e mapeado (`ERR_FAULT`); o caminho é copiado para a pilha antes
de interpretado; caminho malformado é `ERR_INVAL`, mais de 8 componentes
`ERR_NAMETOOLONG`, volume desconhecido ou indisponível `ERR_NODEV`. Depois, a
ordem é:

1. Volume que não aceita escrita: `ERR_ROFS`.
2. Diretório pai (todos os componentes menos o último) inexistente:
   `ERR_NOENT`; um arquivo no meio do caminho: `ERR_TYPE`.
3. O nome já existe (arquivo, diretório ou a raiz do volume): `ERR_EXIST`.
4. O pai não tem entrada livre (o diretório não cresce) ou o volume não tem
   cluster livre: `ERR_NOSPC`, **antes de qualquer escrita**.
5. Erro ao ler ou gravar o volume, ou FAT inconsistente: `ERR_IO` (em qualquer
   passo).

O diretório novo ocupa **1 cluster**: a entrada `.` (atributo diretório,
primeiro cluster = ele mesmo), a entrada `..` (atributo diretório, primeiro
cluster = o do pai, ou `0` se o pai é a raiz do volume) e o resto do setor
zerado, o que deixa 14 entradas livres. A entrada no pai tem atributo
diretório, tamanho `0` e data e hora zeradas. A listagem (`SYS_READ_DIR`) não
mostra `.` nem `..`.

### `SYS_RMDIR` (16)

Remove um diretório e devolve `0`. Mesmas regras de caminho e de volume (itens 1
e 2 acima) de `SYS_MKDIR`. Para o último componente, a ordem é:

1. Inexistente: `ERR_NOENT`.
2. É um arquivo: `ERR_TYPE`.
3. É a raiz de um volume (`/ram`, `/disco`): `ERR_BUSY`.
4. Algum programa o tem aberto, inclusive o que chama: `ERR_BUSY`.
5. Tem qualquer entrada além de `.`, `..`, entradas apagadas e o fim do
   diretório (nome longo e rótulo de volume contam como entrada):
   `ERR_NOTEMPTY`.
6. Cadeia de clusters inconsistente (ciclo, número fora do volume, entrada livre
   no meio): `ERR_IO`. Um diretório assim não pode ser removido.

Nenhuma dessas falhas altera o volume. `SYS_REMOVE` de um diretório continua
`ERR_TYPE`, e `SYS_RMDIR` de um arquivo também.

### `SYS_TCP_LISTEN` (17)

Abre um socket de escuta TCP em `port` (`rdi`, de 1 a 65535), em qualquer
endereço local, e devolve o número do socket. Ordem: `ERR_INVAL` (porta `0` ou
acima de 65535), `ERR_MFILE` (o programa já tem 4 sockets), `ERR_ADDRINUSE`
(já existe um `listen` nessa porta), `ERR_NOBUFS` (os 8 blocos de conexão do
kernel estão em uso). Não depende da placa. O socket novo tem tempo limite de
10 000 ms.

### `SYS_TCP_ACCEPT` (18)

`rdi = sock` (um socket de escuta). Espera uma conexão **estabelecida** na fila
do `listen` e devolve um socket novo para ela, também com tempo limite de
10 000 ms. `ERR_TYPE` se `sock` não é de escuta; `ERR_MFILE` se o programa já
tem 4 sockets (a conexão continua na fila). Com conexão na fila, devolve na
hora. Sem conexão, o programa **bloqueia sem gastar CPU** até chegar uma ou até o
tempo limite do **socket de escuta** (`ERR_TIMEDOUT`). Cada `listen` guarda no
máximo 2 conexões ainda não aceitas; um SYN além disso é ignorado (o par o
reenvia).

### `SYS_TCP_CONNECT` (19)

`rdi = ip`, `rsi = port`. Abre uma conexão e espera o aperto de mão de três
vias. O endereço IPv4 é o número `a<<24 | b<<16 | c<<8 | d` (para `10.0.2.2`,
`0x0A000202`). Ordem: `ERR_INVAL` (`ip` zero ou acima de `0xFFFF_FFFF`, porta
`0` ou acima de 65535), `ERR_MFILE`, `ERR_NOBUFS`, `ERR_NODEV` (destino fora de
`127.0.0.0/8` e sem placa de rede). A porta local sai de `49152..=65535`, em
sequência. Falhas: `ERR_CONNREFUSED` (o par respondeu RST, inclusive o
loopback sem servidor) e `ERR_TIMEDOUT` (3 SYN, um por segundo; o ARP conta
dentro desse tempo). O tempo de espera do `connect` é **fixo**, de 3 s, e não
depende de `SYS_TCP_TIMEOUT`.

### `SYS_TCP_SEND` (20)

`rdi = sock`, `rsi = ptr`, `rdx = len`. Copia até `len` bytes de `ptr` para o
buffer de envio (2048 bytes) e devolve quantos copiou (`1..=len`). **Não espera
a confirmação do par**: o kernel transmite e retransmite em segundo plano
(um segmento de até 1024 bytes por vez, retransmissão a cada 1 s, no máximo 5
vezes). Com o buffer cheio, bloqueia até haver espaço ou até o tempo limite do
socket (`ERR_TIMEDOUT`). `len == 0` devolve `0` sem olhar `ptr`; `len > 4096` é
`ERR_INVAL`. `ERR_TYPE` num socket de escuta. Depois que a conexão falha
(`ERR_CONNRESET`, ou `ERR_TIMEDOUT` por retransmissão), toda chamada devolve
esse erro até o `SYS_TCP_CLOSE`. Se o **par** fechou o lado dele, enviar continua
válido (meio fechamento).

### `SYS_TCP_RECV` (21)

`rdi = sock`, `rsi = ptr`, `rdx = len`. Copia até `len` bytes recebidos para
`ptr` (o intervalo precisa ser gravável) e devolve quantos copiou. Com dados,
devolve na hora. Se o par fechou (FIN) e não resta dado, devolve `0`. Senão
bloqueia até chegar dado, FIN ou erro, ou até o tempo limite do socket
(`ERR_TIMEDOUT`). Dado e FIN juntos: o dado vem primeiro, depois `0`. `len == 0`
devolve `0` sem olhar `ptr` e sem bloquear; `len > 4096` é `ERR_INVAL`;
`ERR_TYPE` num socket de escuta. Depois de uma falha da conexão, entrega o dado
que restava e então o erro.

### `SYS_TCP_CLOSE` (22)

`rdi = sock`. Fecha o socket e devolve `0`, sem esperar; o número deixa de valer
na hora. Numa conexão, o kernel envia o que falta do buffer de envio, depois o
FIN, e termina o fechamento em segundo plano (o estado final, `TIME_WAIT`, dura
1 s). Num socket de escuta, as conexões que ninguém aceitou são abortadas com
RST. Num socket cuja conexão já falhou, só libera.

### `SYS_TCP_TIMEOUT` (23)

`rdi = sock`, `rsi = ms`. Define o tempo limite, em milissegundos, das próximas
esperas do socket (`accept`, `send`, `recv`). `ms` vai de `0` a `60000`; acima
disso é `ERR_INVAL`. Com `0` nenhuma espera é feita: se a operação não pode
andar na hora, devolve `ERR_TIMEDOUT`. A resolução é de 10 ms (o valor é
arredondado para cima ao próximo tick). Não existe espera infinita.

**Ordem das verificações das syscalls de rede**: (1) `ERR_INVAL` para argumentos
fora dos limites; (2) `ERR_FAULT` para ponteiro inválido (`SYS_TCP_SEND` e
`SYS_TCP_RECV`), **antes** de qualquer efeito; (3) `ERR_BADF` e `ERR_TYPE` para o
socket; (4) o resto, como descrito em cada chamada.

## 6. Resultado e códigos de erro

O resultado em `rax` é um inteiro com sinal de 64 bits: `≥ 0` é sucesso
(o significado depende da syscall); `< 0` é erro, com o código abaixo.

| Constante | Valor | Quando |
|-----------|-------|--------|
| `ERR_FAULT` | `-1` | Ponteiro ou intervalo inválido (fora da região, não mapeado, cruzando páginas não mapeadas, ou não gravável em `SYS_READ_LINE`, `SYS_READ`, `SYS_READ_DIR` e `SYS_TIME`). |
| `ERR_INVAL` | `-2` | Argumento fora dos limites permitidos (`len > 4096`, `size == 0` em `SYS_ALLOC`, `path_len == 0`, caminho malformado, ou `len != 8` em `SYS_TIME`). |
| `ERR_NOMEM` | `-3` | `SYS_ALLOC` não pôde dar a memória pedida (limite de 1 MiB do heap ou falta de memória física). |
| `ERR_NOENT` | `-4` | O caminho, ou um componente dele, não existe. |
| `ERR_NODEV` | `-5` | Volume desconhecido, ou conhecido mas indisponível (sem disco, volume inválido); em rede, a placa não existe e o destino não é `127.0.0.0/8`. |
| `ERR_TYPE` | `-6` | Tipo errado: `SYS_READ` em diretório, `SYS_READ_DIR` em arquivo, arquivo no meio do caminho, acrescentar ou remover (`SYS_REMOVE`) um diretório, ou `SYS_RMDIR` de um arquivo; nas syscalls de rede, usar um socket de escuta para `SYS_TCP_SEND` ou `SYS_TCP_RECV`, ou um socket de conexão para `SYS_TCP_ACCEPT`. |
| `ERR_BADF` | `-7` | Descritor inválido: fora de `0..=3`, não aberto ou já fechado, ou usado no modo errado (`SYS_READ` em descritor de acrescentar, `SYS_WRITE_FILE` em descritor de leitura); nas syscalls de rede, socket fora de `0..=3`, não aberto ou já fechado. |
| `ERR_MFILE` | `-8` | O programa já tem 4 arquivos abertos (ou 4 sockets, nas syscalls de rede). |
| `ERR_NAMETOOLONG` | `-9` | Caminho com mais de 64 bytes, ou com mais de 8 componentes. |
| `ERR_IO` | `-10` | Erro ao ler o volume: disco que não responde, erro do disco ou estrutura FAT corrompida. |
| `ERR_CLOCK` | `-11` | `SYS_TIME`: o relógio devolveu valores impossíveis, não se estabilizou ou não respondeu. |
| `ERR_EXIST` | `-12` | `SYS_CREATE`: o nome já existe (arquivo, diretório ou a raiz do volume). |
| `ERR_NOSPC` | `-13` | Sem cluster livre, ou diretório sem entrada livre (também em `SYS_MKDIR`). |
| `ERR_ROFS` | `-14` | O volume existe, está montado e não aceita escrita (um volume com mais de 1 setor por cluster, ou um dispositivo que não sabe gravar: a gravação no `/disco` funciona). |
| `ERR_BUSY` | `-15` | `SYS_REMOVE` de um arquivo, ou `SYS_RMDIR` de um diretório, que algum programa tem aberto; `SYS_RMDIR` da raiz de um volume. |
| `ERR_NOTEMPTY` | `-16` | `SYS_RMDIR` de um diretório que tem qualquer entrada além de `.`, `..` e entradas apagadas. |
| `ERR_TIMEDOUT` | `-17` | O tempo limite do socket esgotou (`SYS_TCP_ACCEPT`, `SYS_TCP_RECV`, `SYS_TCP_SEND`), ou a conexão desistiu: SYN sem resposta em 3 tentativas (3 s), ou dado sem confirmação em 5 retransmissões. |
| `ERR_CONNREFUSED` | `-18` | `SYS_TCP_CONNECT`: o par respondeu RST ao SYN (inclusive o loopback sem servidor). |
| `ERR_CONNRESET` | `-19` | A conexão foi reiniciada pelo par (RST) depois de estabelecida. |
| `ERR_ADDRINUSE` | `-20` | `SYS_TCP_LISTEN` numa porta que já tem `listen`. |
| `ERR_NOBUFS` | `-21` | Já existem 8 conexões TCP no kernel (blocos em `TIME_WAIT` são reaproveitados, o mais antigo primeiro). |

Não há errno: o programa recebe só o valor em `rax`.

## 7. Como um programa termina

| Motivo | Mensagem em tela e serial | Depois |
|--------|---------------------------|--------|
| `SYS_EXIT(0)` | nada na tela (a serial registra) | o prompt volta quando o último programa termina |
| `SYS_EXIT(n)`, `n ≠ 0` | `[run] <nome> terminou com codigo <n>` | idem |
| Acesso inválido à memória (`#PF`) em ring 3 | `[run] <nome> encerrado por erro de memoria: #PF (Page Fault) em <rip>` + `codigo de erro: <código>` + `endereco de falha: <endereço>` | idem |
| Outra exceção de CPU em ring 3 (`#DE`, `#UD`, `#GP`, `#SS`, `#NP`) | `[run] <nome> encerrado por erro: <sigla> (<nome da exceção>) em <rip>` (+ código de erro, quando houver) | idem |
| Número de syscall inexistente (qualquer um fora de 1 a 23) | `[run] <nome> encerrado: syscall inexistente (<n>)` | idem |

A mensagem aparece **na hora** em que aquele programa termina, em tela e
serial, mesmo que outros continuem rodando. Uma falha do programa **nunca**
derruba o kernel nem os outros programas: só ele é encerrado e suas páginas
são liberadas; o prompt volta a responder quando o último termina, e o
programa seguinte roda normalmente. Os erros das syscalls de arquivo (caminho inexistente, volume indisponível,
descritor inválido, ponteiro inválido…) **não** encerram o programa: ele
recebe o código de erro e continua. Os arquivos que o programa deixou abertos
são fechados quando ele termina, por qualquer um dos motivos acima; os sockets também (como `SYS_TCP_CLOSE`). Tudo que
não seja uma das linhas acima
(por exemplo, um laço infinito) fica fora do contrato: não há limite de
tempo total, só a fatia de cada vez (seção 8).

## 8. Vários programas ao mesmo tempo

`run <alvo> [<alvo>...]` carrega todos os programas pedidos antes de começar
qualquer um e só devolve o prompt quando o último termina. Um `<alvo>` que
começa com `/` é o caminho de um arquivo executável (seção 9); qualquer outro
é o nome de um programa embutido, e os dois tipos se misturam na mesma linha. Os programas
começam na ordem dada. O mesmo programa pode aparecer mais de uma vez:
cada ocorrência é uma instância independente.

O que o programa **pode** assumir:

- Tem a memória só dele: o mesmo mapa de endereços da seção 2, mas ninguém
  mais enxerga nem altera essas páginas, e ele não enxerga as dos outros.
- Retoma exatamente de onde parou, com todos os registradores, a pilha e a
  memória intactos, depois de qualquer troca (por `SYS_YIELD`, por timer ou
  por espera de teclado).
- Cada `SYS_WRITE` é entregue inteiro na tela, sem ser partido por saída de
  outro programa. (A biblioteca de runtime junta cada `print!`/`println!`
  numa única chamada de até 256 bytes, então uma linha curta nunca é
  partida.)
- `SYS_YIELD`, com mais programas prontos, passa a vez em rodízio, na ordem
  em que foram dados a `run`.

O que o programa **não** pode assumir:

- Nenhuma ordem de execução entre programas além do rodízio de
  `SYS_YIELD`. Um programa que cede a CPU logo (escreve uma linha e cede,
  como `ping`) só é interrompido pelo timer se calcular por 40 ms ou mais, e
  então a ordem dele é a do rodízio. Um programa que nunca cede a CPU é
  interrompido pelo timer em qualquer instrução e retomado depois.
- Nenhum tempo exato: a fatia é de **5 ticks** e o timer roda a **100 Hz**
  (um tick a cada 10 ms). O timer só tira a CPU de um programa depois que
  cinco ticks foram contados desde que ele foi colocado na CPU, isto é,
  depois de 40 a 50 ms seguidos de computação. Os dois valores podem mudar
  em marcos futuros.
- Nenhum meio de comunicação direta entre programas. Dois programas só conversam pela rede: por sockets TCP no loopback (`127.0.0.1`), por exemplo (seção 10).

Limite: no máximo **4** programas ao mesmo tempo. Pedir mais, um nome
que não existe, um caminho que não pôde ser lido, um arquivo grande demais
ou que não é um ELF64 válido recusa o `run` inteiro e **nenhum** programa é
iniciado.

### Espera de rede

Um programa que espera a rede (`SYS_TCP_ACCEPT`, `SYS_TCP_CONNECT`,
`SYS_TCP_RECV`, `SYS_TCP_SEND` com o buffer cheio) fica **bloqueado**: o
escalonador não o escolhe e os outros programas seguem. Se todos esperam o
teclado ou a rede, o kernel dorme (`hlt`) até a próxima interrupção; ele não gira
num laço ocupado. O resultado da syscall chega quando o programa volta a rodar.
Cada espera termina no máximo no tempo limite do socket (seção 10).

### Teclado

O teclado é entregue ao programa que está esperando uma linha em
`SYS_READ_LINE`. Se mais de um espera, recebe o que pediu **primeiro**; os
demais continuam esperando e só começam a receber depois. Uma tecla nunca vai
a dois programas nem se perde. Um programa esperando não consome CPU.

O prompt não lê o teclado enquanto há algum programa vivo. O que nenhum
programa pediu não é perdido: fica na fila do teclado (16 posições; se
encher, a tecla mais antiga é descartada) e o prompt o recebe quando o
último programa termina, por `exit` ou por erro. Um programa não termina no
meio de uma linha (fica bloqueado em `SYS_READ_LINE` até o Enter); o que
sobra é digitação antecipada, que o prompt recebe intacta, e o que for
digitado depois aparece nele sem perdas.

## 9. Sistema de arquivos

Há dois volumes, e os dois são de **leitura e escrita**: dá para criar
arquivos, acrescentar bytes ao fim deles, apagá-los e criar e remover
diretórios. O `/ram` volta ao conteúdo original a cada boot. O `/disco` guarda o
que foi gravado **na imagem em que o QEMU está rodando**: no `cargo run` e no
`cargo test` (`snapshot=on`) isso é descartado ao fechar o QEMU, e só persiste
entre boots rodando o QEMU à mão sobre uma cópia da imagem (README, "Preservando
o disco entre boots"). Não existe syscall para renomear, mover, posicionar,
sobrescrever no meio nem truncar.

### Volumes

| Prefixo | O que é | Disponível | Escrita |
|---------|---------|------------|---------|
| `/ram` | ramdisk FAT16: cópia, em memória, da imagem embutida na imagem de boot | sempre | **sim** (volta ao conteúdo original ao reiniciar) |
| `/disco` | disco ATA (canal primário, escravo) com um volume FAT16 | só se o QEMU anexou um disco com um volume válido | **sim** (persiste só na imagem do QEMU rodando sem `snapshot=on`) |

Um volume indisponível (sem disco, dispositivo que não é um disco ATA, setor
de boot inválido, erro de leitura) faz as operações sobre ele devolverem
`ERR_NODEV`; o outro volume segue funcionando e o kernel não é afetado.

### Escrever

- **Só pelo fim.** Um arquivo nasce vazio (`SYS_CREATE`) e só cresce por
  `SYS_WRITE_FILE`, sempre no fim atual. Quem quiser trocar o conteúdo apaga e
  cria de novo.
- **Sem cache.** Cada escrita termina com os dados, a FAT e o tamanho já no
  volume. Se a operação for interrompida entre dois setores, o volume continua
  legível; no pior caso sobram clusters usados sem dono.
- **Arquivo aberto não se apaga.** `SYS_REMOVE` de um arquivo aberto por
  qualquer programa (em qualquer modo) dá `ERR_BUSY`. Ao terminar (por `exit`
  ou por falha), o programa deixa de segurar os arquivos.
- **Tamanho visto na abertura.** Um arquivo aberto para leitura mostra o
  tamanho que tinha quando foi aberto; o que for acrescentado depois só
  aparece numa nova abertura.
- **Listagem durante escrita.** Um diretório aberto durante uma criação ou
  remoção nele pode pular ou repetir uma entrada (a listagem anda por posição).
- **Volume cheio.** Os bytes que couberam ficam gravados; o erro
  (`ERR_NOSPC`) vem na chamada seguinte.
- **Diretório não cresce.** Sem entrada livre nos setores que ele já tem, o
  erro é `ERR_NOSPC`. Um diretório novo (`SYS_MKDIR`) ocupa 1 cluster e tem 14
  entradas livres. Só se escreve em volume de 1 setor por cluster (o único
  layout que o projeto gera); outro valor continua legível, mas a escrita é
  `ERR_ROFS` (é o único caso em que o volume é **somente leitura**).
- **No `/disco`, cada setor gravado é descarregado do cache do disco antes do
  próximo**, então a ordem das escritas vale na mídia. Uma escrita grande faz
  dezenas de gravações dentro da syscall, com as interrupções desligadas.
- **Diretório aberto não se remove.** `SYS_RMDIR` de um diretório aberto por
  qualquer programa dá `ERR_BUSY`; só se remove diretório vazio.
- Nada é protegido: qualquer arquivo, ou diretório vazio, de qualquer dos dois
  volumes pode ser apagado, inclusive `/disco/bin/visita`.

### Caminhos

- Formato: `/<volume>/<componente>[/<componente>...]`. `/ram` e `/ram/` são a
  raiz do volume.
- Não diferencia maiúsculas de minúsculas, nem no prefixo do volume:
  `/DISCO/OLA.TXT` e `/disco/ola.txt` são o mesmo arquivo.
- Cada componente é um nome **8.3**: de 1 a 8 caracteres, e opcionalmente um
  ponto e até 3 de extensão, com letras, dígitos, `_`, `-`, `~` e `$`. Não
  há nomes longos: as entradas de nome longo do FAT são ignoradas.
- `.` e `..` são inválidos (`ERR_INVAL`): nenhum caminho sai do volume.

### Limites

| Limite | Valor |
|--------|-------|
| Tamanho do caminho | **64 bytes** |
| Componentes por caminho | **8** |
| Arquivos abertos por programa | **4** |
| Executável lido de arquivo | **65536 bytes** |
| Entradas percorridas por diretório | 512 (um diretório maior, ou com ciclo, é erro de leitura) |

Os nomes devolvidos por `SYS_READ_DIR` vêm em minúsculas. O volume é FAT16
(512 bytes por setor); a FAT é lida como dado não confiável: uma cadeia de
clusters com ciclo, que sai do volume, ou mais curta que o tamanho do arquivo
é `ERR_IO`, nunca um laço infinito.

### `DirEntryRaw`

`SYS_READ_DIR` escreve 20 bytes (`#[repr(C)]`, `size` em little-endian):

| Offset | Tamanho | Campo |
|-------:|--------:|-------|
| 0 | 12 | `name`: `nome.ext` em minúsculas, terminado e preenchido com zeros |
| 12 | 1 | `kind`: `1` arquivo, `2` diretório |
| 13 | 3 | zeros |
| 16 | 4 | `size`: bytes (`0` para diretório) |

### Cada programa tem a sua tabela

Descritores e posições de leitura são **do programa**: dois programas que
abrem o mesmo arquivo têm posições independentes, e um programa nunca enxerga
os descritores de outro. Na biblioteca de runtime: `File::open`, `File::read`,
`Dir::open`, `Dir::next`, `FileWriter::create`, `FileWriter::append`,
`FileWriter::write`, `FileWriter::write_all`, `remove`, `mkdir` e `rmdir` (os
descritores são fechados automaticamente).

## 10. Rede

O kernel fala TCP/IP básico por uma placa Realtek RTL8139 (PCI), com Ethernet II,
ARP, IPv4, ICMP (só o eco) e TCP, mais um **loopback** interno. Os programas usam
**sockets TCP** pelas syscalls 17 a 23 (na biblioteca de runtime: `TcpListener`,
`TcpStream`, `Ipv4`, `NetError`).

- **Endereço**: estático, `10.0.2.15`, máscara `255.255.255.0`, gateway
  `10.0.2.2` (os valores da rede de usuário do QEMU). O comando `rede` os troca,
  só em memória. Pacotes para `127.0.0.0/8` voltam para a própria pilha e
  funcionam **sem placa**; para qualquer outro endereço é preciso a placa
  (`ERR_NODEV` em `SYS_TCP_CONNECT` se ela não existe).
- **Socket**: um número de `0` a `3`, **do programa** (nenhum outro programa o
  enxerga), separado dos descritores de arquivo. No máximo 4 por programa
  (`ERR_MFILE`) e 8 conexões TCP no kernel inteiro (`ERR_NOBUFS`).
- **TCP simplificado**: no máximo um segmento sem confirmação por conexão
  (para-e-espera), segmentos de até 1024 bytes, buffers de 2048 bytes por
  sentido, recepção só em ordem, retransmissão a cada 1 s. Sem UDP, sem
  fragmentação de IP, sem controle de congestionamento.
- **Esperas**: `SYS_TCP_ACCEPT`, `SYS_TCP_CONNECT`, `SYS_TCP_RECV` e
  `SYS_TCP_SEND` (com o buffer cheio) **bloqueiam sem gastar CPU**, como
  `SYS_READ_LINE`. Toda espera tem tempo limite (padrão 10 s, de 0 a 60 s com
  `SYS_TCP_TIMEOUT`; o do `connect` é fixo, 3 s). Não existe espera infinita.
- **Fechamento**: `SYS_TCP_CLOSE` não espera. O kernel termina o fechamento em
  segundo plano, e o estado final (`TIME_WAIT`) dura 1 s em vez de minutos. Ao
  terminar (por `exit` ou por falha), o programa tem os sockets fechados como
  por `SYS_TCP_CLOSE`.
- **Não há** UDP, DNS, DHCP, IPv6 nem TLS; só a primeira placa RTL8139 é usada.

## 11. Histórico

| Versão | Marco | Mudança |
|--------|-------|---------|
| 1 | 5 (os-rust 0.5.0) | Primeira versão: `SYS_WRITE`, `SYS_EXIT`, `syscall`/`sysret`, ELF64 estático. |
| 2 | 6 (os-rust 0.6.0) | `SYS_READ_LINE` (3) e `SYS_ALLOC` (4); `ERR_NOMEM` (`-3`); janela do heap `[0x6000_0000, 0x6010_0000)`; segmentos do ELF limitados a `[0x4000_0000, 0x6000_0000)`; mensagem de `#PF` mostra "erro de memoria"; seção sobre o teclado. Programas v1 continuam funcionando. |
| 3 | 7 (os-rust 0.7.0) | `SYS_YIELD` (5); vários programas ao mesmo tempo (até 4, cada um na sua memória, fatia de 5 ticks, timer a 100 Hz); mensagem de término na hora em que o programa termina; teclado para o programa que pediu primeiro. Programas v1 e v2 continuam funcionando. |
| 4 | 8 (os-rust 0.8.0) | `SYS_OPEN` (6), `SYS_READ` (7), `SYS_CLOSE` (8), `SYS_READ_DIR` (9); erros `ERR_NOENT` (`-4`) a `ERR_IO` (`-10`); volumes `/ram` e `/disco` (FAT16, somente leitura); tabela de arquivos por programa (até 4); `run` aceita caminhos de arquivo (executável de até 64 KiB). Programas v1, v2 e v3 continuam funcionando. |
| 5 | 9 (os-rust 0.9.0) | `SYS_TIME` (10); erro `ERR_CLOCK` (`-11`); `DateTime` de 8 bytes, sempre em UTC. Programas v1 a v4 continuam funcionando. |
| 6 | 10 (os-rust 0.10.0) | `SYS_CREATE` (11), `SYS_OPEN_APPEND` (12), `SYS_WRITE_FILE` (13), `SYS_REMOVE` (14); erros `ERR_EXIST` (`-12`) a `ERR_BUSY` (`-15`); `/ram` passa a aceitar escrita (só pelo fim); `/disco` continua somente leitura; `ERR_TYPE` e `ERR_BADF` cobrem também o modo errado do descritor. Programas v1 a v5 continuam funcionando. |
| 7 | 11 (os-rust 0.11.0) | `SYS_MKDIR` (15), `SYS_RMDIR` (16); erro `ERR_NOTEMPTY` (`-16`); `/disco` passa a aceitar escrita (cada setor é descarregado do cache do disco); `ERR_BUSY` cobre também diretório aberto e a raiz de um volume, `ERR_NOSPC` cobre `SYS_MKDIR`, e `ERR_ROFS` só vale para o volume que não aceita escrita. Programas v1 a v6 continuam funcionando. |
| 8 | 12 (os-rust 0.12.0) | `SYS_TCP_LISTEN` (17), `SYS_TCP_ACCEPT` (18), `SYS_TCP_CONNECT` (19), `SYS_TCP_SEND` (20), `SYS_TCP_RECV` (21), `SYS_TCP_CLOSE` (22), `SYS_TCP_TIMEOUT` (23); erros `ERR_TIMEDOUT` (`-17`) a `ERR_NOBUFS` (`-21`); sockets TCP por programa (até 4, 8 conexões no kernel), esperas de rede sem gastar CPU com tempo limite, loopback `127.0.0.0/8`; `ERR_NODEV`, `ERR_MFILE`, `ERR_TYPE` e `ERR_BADF` cobrem também as syscalls de rede. Programas v1 a v7 continuam funcionando. |
