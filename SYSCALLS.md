# Contrato de syscalls do os-rust

**Versão do contrato**: 1

**Válido a partir de**: os-rust 0.5.0 (Marco 5)

Este documento é tudo que um programa precisa saber para rodar no os-rust.
O kernel e os programas nunca dependem de nada que não esteja aqui. Uma
mudança **incompatível** aumenta a versão do contrato e exige atualizar a
biblioteca de runtime dos programas (Marco 6 em diante) no mesmo marco.

## 1. Formato do executável

- ELF64, little-endian, `e_type = ET_EXEC` (2), `e_machine = EM_X86_64` (62).
  Sem relocação, sem PIE, sem bibliotecas dinâmicas.
- Cada `PT_LOAD` deve ter `p_vaddr` alinhado a 4 KiB, `p_filesz ≤ p_memsz`
  (o que sobra é zerado, como `.bss`), e **não compartilhar página** com
  outro `PT_LOAD`. Os demais tipos de segmento são ignorados.
- Permissões: página gravável só se o segmento tem `PF_W`; executável só
  se tem `PF_X` (W^X). A pilha é gravável e nunca executável.
- `e_entry` deve estar dentro de um segmento executável.
- O executável é embutido na imagem de boot em tempo de compilação
  (não há sistema de arquivos) e executado com `run <nome>`.

## 2. Onde o programa vive

| Item | Valor |
|------|-------|
| Região do usuário | `[0x4000_0000, 0x8000_0000)` (1 GiB a 2 GiB) |
| Endereço de carga | O que os `PT_LOAD` declaram, dentro da região. Convenção do projeto: base `0x4000_0000`. |
| Pilha | 4 páginas (16 KiB) no fim da região: `[0x7FFF_C000, 0x8000_0000)`; a página abaixo não é mapeada (guarda). |
| Fora da região | Qualquer acesso (kernel, VGA, memória física) causa uma exceção e o programa é encerrado. |

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

Não há `argc`/`argv`/ambiente: nada na pilha inicial (Marco 6 estenderá).
O programa **nunca retorna** de `_start`: deve chamar `exit`.

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

Durante a syscall o kernel roda com interrupções desligadas e numa pilha
própria; a pilha do programa nunca é usada nem confiada.

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

Números do contrato v1. **Qualquer outro número** (inclusive `0`)
encerra o programa (§7).

| Nº | Nome | Argumentos | Resultado |
|----|------|-----------|-----------|
| 1 | `SYS_WRITE` | `rdi = ptr`, `rsi = len` | bytes escritos (`≥ 0`) ou erro (`< 0`) |
| 2 | `SYS_EXIT` | `rdi = code` | não retorna |

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
todas as páginas do programa são liberadas e nenhum estado do programa
sobrevive à chamada.

## 6. Resultado e códigos de erro

O resultado em `rax` é um inteiro com sinal de 64 bits: `≥ 0` é sucesso
(o significado depende da syscall); `< 0` é erro, com o código abaixo.

| Constante | Valor | Quando |
|-----------|-------|--------|
| `ERR_FAULT` | `-1` | Ponteiro ou intervalo inválido (fora da região, não mapeado, cruzando páginas não mapeadas). |
| `ERR_INVAL` | `-2` | Argumento fora dos limites permitidos (`len > 4096`). |

Não há errno: o programa recebe só o valor em `rax`.

## 7. Como um programa termina

| Motivo | Mensagem em tela e serial | Depois |
|--------|---------------------------|--------|
| `SYS_EXIT(0)` | nada na tela (a serial registra) | volta ao prompt |
| `SYS_EXIT(n)`, `n ≠ 0` | `[run] <nome> terminou com codigo <n>` | volta ao prompt |
| Exceção de CPU em ring 3 (`#DE`, `#UD`, `#GP`, `#PF`, `#SS`, `#NP`) | `[run] <nome> encerrado por erro: <sigla> (<nome da exceção>) em <rip>` (+ código de erro e endereço de falha, quando houver) | volta ao prompt |
| Número de syscall inexistente | `[run] <nome> encerrado: syscall inexistente (<n>)` | volta ao prompt |

Uma falha do programa **nunca** derruba o kernel (Princípio VII). Tudo
que não seja uma das linhas acima (por exemplo, um laço infinito) fica
fora do contrato v1: não há limite de tempo (Marco 7 em diante).

## 8. Histórico

| Versão | Marco | Mudança |
|--------|-------|---------|
| 1 | 5 (os-rust 0.5.0) | Primeira versão: `SYS_WRITE`, `SYS_EXIT`, `syscall`/`sysret`, ELF64 estático. |
