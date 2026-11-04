# Como o proto-os funciona (para quem nunca viu um sistema operacional)

Este texto explica, sem pressupor nenhum conhecimento prévio de sistemas
operacionais, o que está acontecendo quando você roda `cargo run` neste
projeto e vê uma tela colorida aparecer no QEMU.

## O que é "dar boot sem sistema operacional"?

Todo programa que você já rodou no seu computador — um navegador, um editor
de texto, um jogo — roda **em cima** de um sistema operacional (Windows,
Linux, macOS). O sistema operacional é quem:

- decide qual programa usa o processador em cada instante,
- controla o acesso a arquivos, tela, teclado, rede,
- protege um programa de bagunçar a memória de outro.

Quando o computador liga, só existe hardware — não existe programa nenhum
"orquestrando" nada ainda. É o **BIOS** (um pequeno programa gravado no
hardware da placa-mãe) que roda primeiro e faz a primeira tarefa: encontrar
um dispositivo bootável (aqui, o disco virtual criado pelo `bootimage`) e
carregar o primeiro pedaço de código dele para a memória — é esse código
que "dá boot" no sistema operacional.

O `proto-os` faz algo incomum: em vez de dar boot em um Windows ou Linux,
ele dá boot **direto no nosso próprio binário Rust**. Não existe Windows,
Linux ou qualquer outro sistema operacional rodando por baixo — só o BIOS,
depois o nosso código, e mais nada. É por isso que a tela que aparece no
QEMU não se parece com um terminal comum: **não há terminal nenhum** ali,
só a tela de texto que o próprio binário desenhou byte a byte.

## Da BIOS até a nossa mensagem na tela: o caminho completo

1. **QEMU liga uma máquina virtual** e simula o BIOS de um PC comum.
2. **O BIOS carrega o *bootloader*** — um pequeno programa (gerado pela
   crate `bootloader`, que o `bootimage` empacota no disco de boot) cuja
   única tarefa é colocar o processador no modo certo (modo 64-bit) e
   carregar o nosso binário Rust na memória, no endereço combinado entre
   ele e o nosso `Cargo.toml`/target customizado.
3. **O bootloader transfere o controle para `kernel_main`** — a função em
   `src/main.rs` marcada com a macro `entry_point!`. A partir daqui, o
   código que está rodando é **o nosso**, linha por linha, sem nenhuma
   camada de sistema operacional no meio.
4. **`kernel_main` limpa a tela e escreve a mensagem de boas-vindas**
   chamando `println!`, que por baixo dos panos escreve diretamente em um
   endereço de memória especial: `0xb8000`.

## Por que `0xb8000`? A tela como memória

Em modo texto VGA (um modo de vídeo muito antigo, mas ainda suportado por
praticamente todo PC/QEMU), a placa de vídeo lê continuamente um bloco fixo
de memória a partir do endereço `0xb8000` e desenha na tela o que encontra
lá: 25 linhas por 80 colunas, e cada posição da tela corresponde a **2
bytes** nessa memória — um byte com o caractere ASCII, outro com a cor
(primeiro plano + fundo). Ou seja: **escrever na tela, neste modo, é
literalmente a mesma coisa que escrever em um endereço de memória**. Não
existe "API de desenho", não existe driver gráfico — é só memória.

É por isso que `src/vga_buffer.rs` trata a tela como uma matriz de 25×80
posições (`Buffer`) e por que cada escrita usa a crate `volatile`: sem ela,
o compilador Rust poderia "otimizar" e simplesmente não escrever de verdade
naquele endereço (afinal, do ponto de vista do compilador, ninguém "lê" o
valor de volta) — mas o hardware de vídeo **lê** esse endereço o tempo
todo, então a escrita precisa acontecer de verdade, na ordem certa. Isso é
também por que essa parte do código precisa de `unsafe`: o compilador não
tem como saber, sozinho, que `0xb8000` é um endereço válido e especial —
somos nós que garantimos isso, e por isso comentamos cada bloco `unsafe`
explicando exatamente por quê ele é seguro naquele ponto.

## Por que o texto rola quando passa de uma tela?

A tela só tem 25 linhas. Quando o texto ultrapassa a última linha, em vez
de travar ou escrever fora dos limites da memória de vídeo, o código copia
cada linha uma posição para cima (a linha 2 vira a linha 1, a linha 3 vira
a linha 2, e assim por diante) e limpa a última linha — dando a impressão
de que o texto "sobe" na tela, exatamente como em um terminal comum.

## O que acontece quando dá errado: panic

Em um programa Rust comum (rodando sobre um sistema operacional), um
`panic!` normalmente imprime uma mensagem de erro no terminal e o sistema
operacional então encerra o processo. Mas aqui **não existe sistema
operacional para encerrar o processo** — o `proto-os` *é* tudo o que está
rodando na máquina virtual. Se simplesmente parássemos o processador sem
fazer nada, a plateia veria uma tela travada sem explicação nenhuma, o que
pareceria um defeito.

Por isso, `src/panic.rs` intercepta qualquer `panic!` do programa (via
`#[panic_handler]`, o mecanismo do Rust para dizer "é isto que deve
acontecer quando um panic ocorre, já que não há sistema operacional para
decidir por nós") e usa a mesma infraestrutura de escrita em `0xb8000` para
mostrar uma mensagem de erro legível na tela, antes de parar a CPU de forma
segura (instrução `hlt`, que literalmente pausa o processador). O código
também se protege contra o caso raro de um panic acontecer *durante* o
próprio tratamento de outro panic — nesse caso, ele nem tenta escrever de
novo, só para a CPU, para nunca travar em um loop sem saída.


