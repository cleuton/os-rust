# Contrato de syscalls do os-rust

**Versão do contrato**: 2

**Válido a partir de**: os-rust 0.6.0 (Marco 6)

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
- O executável é embutido na imagem de boot em tempo de compilação
  (não há sistema de arquivos) e executado com `run <nome>`.

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
| `rcx` | `e_entry` (efeito colateral de `sysretq`, que deixa `rcx` como estava) |
| `r11` | `0x202` (idem: é onde `sysretq` lê as flags) |
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
syscall, com uma única exceção: `SYS_READ_LINE` as liga enquanto espera
uma tecla (seção 5) e as desliga de novo antes de voltar ao programa.

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

Números do contrato v2. **Qualquer outro número** (inclusive `0`)
encerra o programa (§7).

| Nº | Nome | Argumentos | Resultado |
|----|------|-----------|-----------|
| 1 | `SYS_WRITE` | `rdi = ptr`, `rsi = len` | bytes escritos (`≥ 0`) ou erro (`< 0`) |
| 2 | `SYS_EXIT` | `rdi = code` | não retorna |
| 3 | `SYS_READ_LINE` | `rdi = ptr`, `rsi = len` | bytes escritos (`≥ 1`), `0` se `len == 0`, ou erro (`< 0`) |
| 4 | `SYS_ALLOC` | `rdi = size` | endereço do início da área nova (`> 0`) ou erro (`< 0`) |

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
`[run] <nome> terminou com codigo <code>`. O controle volta ao prompt,
todas as páginas do programa (código, dados, pilha e heap) são liberadas e
nenhum estado do programa sobrevive à chamada.

### `SYS_READ_LINE` (3)

Espera a pessoa digitar uma linha no teclado e a entrega ao programa. O
programa fica **bloqueado** até o Enter. Enquanto espera, o kernel mostra
na tela o que é digitado (eco) e trata o Backspace.

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
- Como o único leitor do teclado durante a execução é o programa, teclas
  digitadas e ainda não lidas ficam guardadas (até 16) e vão para o prompt
  quando o programa termina (seção 8).

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

## 6. Resultado e códigos de erro

O resultado em `rax` é um inteiro com sinal de 64 bits: `≥ 0` é sucesso
(o significado depende da syscall); `< 0` é erro, com o código abaixo.

| Constante | Valor | Quando |
|-----------|-------|--------|
| `ERR_FAULT` | `-1` | Ponteiro ou intervalo inválido (fora da região, não mapeado, cruzando páginas não mapeadas, ou não gravável em `SYS_READ_LINE`). |
| `ERR_INVAL` | `-2` | Argumento fora dos limites permitidos (`len > 4096`, ou `size == 0` em `SYS_ALLOC`). |
| `ERR_NOMEM` | `-3` | `SYS_ALLOC` não pôde dar a memória pedida (limite de 1 MiB do heap ou falta de memória física). |

Não há errno: o programa recebe só o valor em `rax`.

## 7. Como um programa termina

| Motivo | Mensagem em tela e serial | Depois |
|--------|---------------------------|--------|
| `SYS_EXIT(0)` | nada na tela (a serial registra) | volta ao prompt |
| `SYS_EXIT(n)`, `n ≠ 0` | `[run] <nome> terminou com codigo <n>` | volta ao prompt |
| Acesso inválido à memória (`#PF`) em ring 3 | `[run] <nome> encerrado por erro de memoria: #PF (Page Fault) em <rip>` + `codigo de erro: <código>` + `endereco de falha: <endereço>` | volta ao prompt |
| Outra exceção de CPU em ring 3 (`#DE`, `#UD`, `#GP`, `#SS`, `#NP`) | `[run] <nome> encerrado por erro: <sigla> (<nome da exceção>) em <rip>` (+ código de erro, quando houver) | volta ao prompt |
| Número de syscall inexistente | `[run] <nome> encerrado: syscall inexistente (<n>)` | volta ao prompt |

Uma falha do programa **nunca** derruba o kernel (Princípio VII): o
programa é encerrado, suas páginas são liberadas e o prompt volta a
responder; o programa seguinte roda normalmente. Tudo que não seja uma das
linhas acima (por exemplo, um laço infinito) fica fora do contrato: não há
limite de tempo (Marco 7 em diante).

## 8. Teclado enquanto o programa roda

Só um programa roda por vez e o prompt não lê o teclado enquanto ele roda:
o programa é o único leitor, através de `SYS_READ_LINE`. O que o programa
não leu não é perdido: fica na fila do teclado (16 posições; se encher, a
tecla mais antiga é descartada) e o prompt o recebe quando o programa
termina, por `exit` ou por erro. Um programa não termina no meio de uma
linha (fica bloqueado em `SYS_READ_LINE` até o Enter); o que sobra é
digitação antecipada, que o prompt recebe intacta, e o que for digitado
depois aparece nele sem perdas.

## 9. Histórico

| Versão | Marco | Mudança |
|--------|-------|---------|
| 1 | 5 (os-rust 0.5.0) | Primeira versão: `SYS_WRITE`, `SYS_EXIT`, `syscall`/`sysret`, ELF64 estático. |
| 2 | 6 (os-rust 0.6.0) | `SYS_READ_LINE` (3) e `SYS_ALLOC` (4); `ERR_NOMEM` (`-3`); janela do heap `[0x6000_0000, 0x6010_0000)`; segmentos do ELF limitados a `[0x4000_0000, 0x6000_0000)`; mensagem de `#PF` mostra "erro de memoria"; seção sobre o teclado. Programas v1 continuam funcionando. |
