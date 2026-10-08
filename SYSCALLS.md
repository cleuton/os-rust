[ENGLISH VERSION](SYSCALLS.en.md)

# Contrato de syscalls do os-rust

**Versão do contrato**: 5

**Válido a partir de**: os-rust 0.9.0 (Marco 9)

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

Números do contrato v5. **Qualquer outro número** (inclusive `0`)
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
- `fd` de um diretório: `ERR_TYPE`.
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
- `fd` inválido: `ERR_BADF`. `fd` de um arquivo: `ERR_TYPE`.
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

## 6. Resultado e códigos de erro

O resultado em `rax` é um inteiro com sinal de 64 bits: `≥ 0` é sucesso
(o significado depende da syscall); `< 0` é erro, com o código abaixo.

| Constante | Valor | Quando |
|-----------|-------|--------|
| `ERR_FAULT` | `-1` | Ponteiro ou intervalo inválido (fora da região, não mapeado, cruzando páginas não mapeadas, ou não gravável em `SYS_READ_LINE`, `SYS_READ`, `SYS_READ_DIR` e `SYS_TIME`). |
| `ERR_INVAL` | `-2` | Argumento fora dos limites permitidos (`len > 4096`, `size == 0` em `SYS_ALLOC`, `path_len == 0`, caminho malformado, ou `len != 8` em `SYS_TIME`). |
| `ERR_NOMEM` | `-3` | `SYS_ALLOC` não pôde dar a memória pedida (limite de 1 MiB do heap ou falta de memória física). |
| `ERR_NOENT` | `-4` | O caminho, ou um componente dele, não existe. |
| `ERR_NODEV` | `-5` | Volume desconhecido, ou conhecido mas indisponível (sem disco, volume inválido). |
| `ERR_TYPE` | `-6` | Tipo errado: `SYS_READ` em diretório, `SYS_READ_DIR` em arquivo, ou arquivo no meio do caminho. |
| `ERR_BADF` | `-7` | Descritor inválido: fora de `0..=3`, não aberto ou já fechado. |
| `ERR_MFILE` | `-8` | O programa já tem 4 arquivos abertos. |
| `ERR_NAMETOOLONG` | `-9` | Caminho com mais de 64 bytes, ou com mais de 8 componentes. |
| `ERR_IO` | `-10` | Erro ao ler o volume: disco que não responde, erro do disco ou estrutura FAT corrompida. |
| `ERR_CLOCK` | `-11` | `SYS_TIME`: o relógio devolveu valores impossíveis, não se estabilizou ou não respondeu. |

Não há errno: o programa recebe só o valor em `rax`.

## 7. Como um programa termina

| Motivo | Mensagem em tela e serial | Depois |
|--------|---------------------------|--------|
| `SYS_EXIT(0)` | nada na tela (a serial registra) | o prompt volta quando o último programa termina |
| `SYS_EXIT(n)`, `n ≠ 0` | `[run] <nome> terminou com codigo <n>` | idem |
| Acesso inválido à memória (`#PF`) em ring 3 | `[run] <nome> encerrado por erro de memoria: #PF (Page Fault) em <rip>` + `codigo de erro: <código>` + `endereco de falha: <endereço>` | idem |
| Outra exceção de CPU em ring 3 (`#DE`, `#UD`, `#GP`, `#SS`, `#NP`) | `[run] <nome> encerrado por erro: <sigla> (<nome da exceção>) em <rip>` (+ código de erro, quando houver) | idem |
| Número de syscall inexistente (qualquer um fora de 1 a 10) | `[run] <nome> encerrado: syscall inexistente (<n>)` | idem |

A mensagem aparece **na hora** em que aquele programa termina, em tela e
serial, mesmo que outros continuem rodando. Uma falha do programa **nunca**
derruba o kernel nem os outros programas: só ele é encerrado e suas páginas
são liberadas; o prompt volta a responder quando o último termina, e o
programa seguinte roda normalmente. Os erros das syscalls de arquivo (caminho inexistente, volume indisponível,
descritor inválido, ponteiro inválido…) **não** encerram o programa: ele
recebe o código de erro e continua. Os arquivos que o programa deixou abertos
são fechados quando ele termina, por qualquer um dos motivos acima. Tudo que
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
- Nenhum meio de comunicação entre programas (não existe nesta versão).

Limite: no máximo **4** programas ao mesmo tempo. Pedir mais, um nome
que não existe, um caminho que não pôde ser lido, um arquivo grande demais
ou que não é um ELF64 válido recusa o `run` inteiro e **nenhum** programa é
iniciado.

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

O sistema de arquivos é **somente leitura**. Não existe syscall para criar,
escrever, renomear nem apagar: nenhum arquivo muda enquanto o os-rust roda.

### Volumes

| Prefixo | O que é | Disponível |
|---------|---------|------------|
| `/ram` | ramdisk FAT16 embutido na imagem de boot | sempre |
| `/disco` | disco ATA (canal primário, escravo) com um volume FAT16 | só se o QEMU anexou um disco com um volume válido |

Um volume indisponível (sem disco, dispositivo que não é um disco ATA, setor
de boot inválido, erro de leitura) faz as operações sobre ele devolverem
`ERR_NODEV`; o outro volume segue funcionando e o kernel não é afetado.

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
`Dir::open`, `Dir::next` (os descritores são fechados automaticamente).

## 10. Histórico

| Versão | Marco | Mudança |
|--------|-------|---------|
| 1 | 5 (os-rust 0.5.0) | Primeira versão: `SYS_WRITE`, `SYS_EXIT`, `syscall`/`sysret`, ELF64 estático. |
| 2 | 6 (os-rust 0.6.0) | `SYS_READ_LINE` (3) e `SYS_ALLOC` (4); `ERR_NOMEM` (`-3`); janela do heap `[0x6000_0000, 0x6010_0000)`; segmentos do ELF limitados a `[0x4000_0000, 0x6000_0000)`; mensagem de `#PF` mostra "erro de memoria"; seção sobre o teclado. Programas v1 continuam funcionando. |
| 3 | 7 (os-rust 0.7.0) | `SYS_YIELD` (5); vários programas ao mesmo tempo (até 4, cada um na sua memória, fatia de 5 ticks, timer a 100 Hz); mensagem de término na hora em que o programa termina; teclado para o programa que pediu primeiro. Programas v1 e v2 continuam funcionando. |
| 4 | 8 (os-rust 0.8.0) | `SYS_OPEN` (6), `SYS_READ` (7), `SYS_CLOSE` (8), `SYS_READ_DIR` (9); erros `ERR_NOENT` (`-4`) a `ERR_IO` (`-10`); volumes `/ram` e `/disco` (FAT16, somente leitura); tabela de arquivos por programa (até 4); `run` aceita caminhos de arquivo (executável de até 64 KiB). Programas v1, v2 e v3 continuam funcionando. |
| 5 | 9 (os-rust 0.9.0) | `SYS_TIME` (10); erro `ERR_CLOCK` (`-11`); `DateTime` de 8 bytes, sempre em UTC. Programas v1 a v4 continuam funcionando. |
