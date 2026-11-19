[ENGLISH VERSION](WALKTHROUGH-NANORUST.en.md)

# Como o `nanorust` funciona, por dentro

Este documento explica `programs/src/bin/nanorust.rs`, o compilador do os-rust, para quem sabe programar mas nunca escreveu um compilador. O manual de uso está em [`NANORUST.md`](NANORUST.md); aqui o assunto é o código. O arquivo está na mesma ordem deste texto, em seções numeradas. Todo nome de função citado entre crases com parênteses, como `lexer()`, existe no arquivo.

## 1. Por que é um programa de usuário

Um compilador não precisa de privilégio nenhum: lê um arquivo, calcula e grava outro. O kernel já oferece isso a qualquer programa, então o `nanorust` **não mudou o kernel** (nem o contrato de syscalls, versão 9, nem a biblioteca de runtime). Roda em ring 3, como o `hello`, e um erro dele só encerra ele.

Do runtime o programa usa `args::read` e `args::words` (os argumentos do `exec`), `File::open` e `read` (o fonte), `FileWriter::overwrite` e `write_all` (o executável), `remove` e `println!`. Ele usa `extern crate alloc` para ter `Vec`, e `entry!(main)` liga o `main` ao ponto de entrada.

O **programa gerado** é outra coisa: não usa o runtime. Ele só fala com o kernel por três syscalls do contrato, `SYS_WRITE`, `SYS_EXIT` e `SYS_READ_LINE`, escritas direto em código de máquina.

## 2. As fases

`Comp::compila()` roda quatro etapas, nesta ordem, e **para no primeiro erro**:

1. `lexer()` separa todo o fonte em tokens (erros M01, M02 e M03 de texto mal formado);
2. `Comp::passada1()` registra o nome, os parâmetros e o retorno de cada função, **pulando os corpos** pelo casamento de `{` e `}` (assim uma função pode ser chamada antes de definida);
3. `Comp::passada2()` confere os tipos **e gera o código no mesmo passo**, sem árvore sintática: o código de máquina sai enquanto o fonte é lido;
4. `Comp::fecha()` junta as rotinas de apoio e os textos, resolve os endereços que faltavam e escreve o cabeçalho ELF.

Só depois, se nada falhou, `main()` chama `grava()`. Por isso um erro de compilação nunca cria nem altera arquivo: tudo acontece na memória.

## 3. Erros são valores

Um `Erro` é uma struct pequena de números (o código de M01 a M17, o deslocamento do byte, e até três dados). Toda função do compilador devolve `R<T>`, que é `Result<T, Erro>`, e propaga com `?`. O **texto** só é montado em `mensagem()`, chamada por `main()` com a pilha quase vazia: nenhum `format!` roda dentro da recursão, porque a pilha do programa é de 16 KiB. A linha e a coluna só são calculadas aqui, por `linha_coluna()`, contando os `\n` até o deslocamento.

## 4. O léxico

Um `Token` tem 24 bytes (um `const _` confere): o tipo (`Tk`), o deslocamento e o tamanho no fonte e o valor, se for um inteiro. Não guarda linha nem coluna. As palavras reservadas são os primeiros valores de `Tk`, de `Fn` a `Res`, então `Tk::eh_reservada()` é uma comparação só; `reservada()` decide se uma palavra é reservada. O `lexer()` é um laço sobre os bytes: espaço, comentário `//`, identificador, inteiro (que confere o limite e a letra colada, M02), texto (que confere os escapes, M03) e os símbolos, o mais longo primeiro (`->` antes de `-`).

O vetor de tokens é reservado com o tamanho do fonte mais 1 (o token de fim): nunca há mais tokens que bytes.

## 5. Tipos e o que cada expressão devolve

`Ty` tem `I64`, `Bool`, `Unit` e `Nunca` (o `!`, que serve a qualquer tipo). Compilar uma expressão devolve um `Val`: o tipo, o índice do **primeiro token** da expressão (onde M08 e M11 apontam), e dois sinais: `solto` (só literais e operações: o Rust daria `i32`) e `zero` (é um literal 0, para M12). `exige_ty()` faz a conferência de tipo e monta o M08.

## 6. Memória fixa

O heap é de 1 MiB, e nenhuma tabela cresce: `Comp::novo()` reserva tudo de uma vez, com `Vec::with_capacity`, na ordem do maior para o menor (tokens, executável, textos, remendos, funções, parâmetros e variáveis). As capacidades vêm dos limites: `fn f(){}` tem 6 tokens, então no máximo `MAX_FONTE / 6` funções; cada remendo é de uma instrução de 5 bytes ou mais, então no máximo `MAX_EXE / 5` remendos. A soma fica perto de 690 KB.

## 7. Passada 1

`passada1()` percorre o fonte só olhando os cabeçalhos. `cabecalho()` lê `fn nome(params) [-> tipo]`, confere o nome da função (M07, M15) e **aceita qualquer palavra como nome de parâmetro** (a passada 2 confere, M07), e depois pula o corpo contando chaves. Ao fim, `main` precisa existir (M14). A tabela de funções guarda o índice do `{` do corpo, para a passada 2 recomeçar dali.

## 8. Passada 2: código de máquina de pilha

A convenção é simples e é o que torna o gerador pequeno: **o resultado de toda expressão fica em `rax`**; um operando em espera fica **na pilha**, nunca em registrador (o `syscall` destrói `rcx` e `r11`, e o código nunca depende deles).

`expr_prec()` trata todos os operadores binários, numa função só, por tabela de precedência (`Comp::prec()`: `||` 1, `&&` 2, comparações 3, `+ -` 4, `* / %` 5). Para `a + b`: compila `a` (em `rax`), emite `push rax`, compila `b`, e emite `mov rcx, rax; pop rax; add rax, rcx; jo estouro`. Uma cadeia `a + b + c` é um **laço**, não recursão. Uma função só para todos os níveis é o que mantém poucos quadros de pilha por nível de aninhamento. `unario()` trata `-` e `!`; `primario()` trata inteiro, `true`/`false`, variável, parênteses, `if`, `print!`/`println!`, `return` e a chamada.

A aritmética usa `jo` depois de `add`, `sub`, `imul` e `neg` para o panic de estouro; a divisão confere o divisor zero e o `MIN / -1` antes de `idiv`. O `solto` e o `zero` de cada `Val` são atualizados aqui, e é aqui que saem M08, M11 e M12.

### O aninhamento e a pilha de 16 KiB

`entra()` e `sai()` contam o aninhamento: blocos `{}`, parênteses, operadores unários, `return` e a condição de um `if`/`while` enquanto é lida. Passou de 10, M05. É esse limite que garante que a recursão do compilador cabe na pilha. Medido no build de release: `expr_prec` e `bloco` usam 128 bytes de quadro, `unario` e `se` 112 (o resto é incorporado a eles); o pior nível, com os cinco níveis de precedência cheios, custa cerca de 1 KiB, então 10 níveis mais a base ficam em cerca de 11 KiB, dentro dos 12 KiB de orçamento.

## 9. Variáveis, blocos e funções

`funcao()` valida os nomes dos parâmetros, emite o prólogo `push rbp; mov rbp, rsp; sub rsp, 8*L` (o `L`, a quantidade de locais, só se conhece no fim, então o `sub` é preenchido depois), o teste de pilha, o corpo e o epílogo. O **teste de pilha** compara `rsp` com `0x7FFFC800` e, se menor, **salta** (não chama: a pilha pode estar no fim) para o panic `pilha esgotada`. Parâmetro `i` de `n` fica em `[rbp+16+8*(n-1-i)]` e a local `k` em `[rbp-8*(k+1)]`.

`bloco()` é um laço de instruções. Cada `let` (`let_()`) ganha uma posição nova, mesmo se sombreia outro. O inicializador é compilado **antes** de a variável entrar na tabela, e é por isso que `let x: i64 = x + 1;` usa o `x` antigo. Ao fechar o bloco, a tabela de variáveis é cortada de volta ao tamanho que tinha. `atribuicao()` confere `let mut` (M10).

## 10. Controle de fluxo

`se()` emite `test rax, rax; jz` para o `else`. Os saltos para frente são **remendados** quando o destino aparece: `salto()` emite o `rel32` vazio e devolve a posição, e `aqui()` o corrige. A cadeia `else if` é um laço em `se()`; os saltos para o fim formam uma **lista encadeada dentro do próprio código** (cada `rel32` guarda a posição do anterior), então não há tabela e a cadeia não cresce a pilha. `junta_ramo()` junta o tipo de cada ramo. `enquanto()` faz o salto para trás com `salto_volta()`. `ret()` emite o epílogo direto.

## 11. Chamadas e `print!`

`chamada()` avalia os argumentos da esquerda para a direita, empilha cada um, emite `call` e o chamador remove os argumentos. Como a função chamada pode ser definida depois, o `call` vai com um **remendo** (`rem()`), resolvido em `fecha()`.

`macro_()` avalia **todos** os argumentos e os empilha, e só então `emite_escrita()` escreve: cada trecho de texto é um `SYS_WRITE` (`escreve_trecho()`, em pedaços de até 4096 bytes), e cada `{}` carrega o valor da pilha e chama a rotina `imprime`. `confere_formato()` valida as chaves (M13). Os textos ficam no fim do arquivo e são referenciados por endereço relativo (`lea rdi, [rip+...]`).

## 12. O fim: rotinas de apoio, remendos e ELF

As rotinas de apoio (`ROT_IMPRIME`, `ROT_LE_I64` e os três panics, por `emite_panic()`) são **tabelas de bytes escritas à mão**, cada instrução comentada, e só entram se foram usadas. `fecha()` soma os tamanhos, confere contra 65536 (M17), emite as rotinas, os textos, resolve todos os remendos (`rel32 = alvo - (posição + 4)`) e escreve o cabeçalho: um ELF64 `ET_EXEC` com **um** `PT_LOAD` leitura e execução em `0x4000_0000`, entrada em `0x4000_0078` (logo depois dos 120 bytes de cabeçalho). Cada byte emitido passa por `Comp::b()`, que conta o tamanho e dá M17 quando passa de 65536.

A saída é **determinística**: sem tabela de dispersão, sem endereço de heap, sem data. O mesmo fonte gera sempre os mesmos bytes.

## 13. Gravar

`le_fonte()` lê o fonte em pedaços e o **fecha** antes de qualquer gravação. `grava()` abre a saída com `FileWriter::overwrite` e grava tudo: se não consegue abrir, nada foi alterado; se a gravação falha depois (o arquivo já foi zerado), apaga o arquivo quebrado e mostra o erro (M19).

## 14. Como se testa

O compilador roda dentro do QEMU, e `tests/nanorust.rs` o usa de verdade pelo prompt: compila, confere o ELF com o `elf::parse` do kernel, roda e compara a tela. Os seis exemplos de `programs/nano/exemplos/` foram conferidos também com o `rustc` (`cat programs/nano/prelude.rs exemplo.rs`), e os testes guardam a saída esperada.
