# Como o os-rust funciona (para quem nunca viu um sistema operacional)

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

O `os-rust` faz algo incomum: em vez de dar boot em um Windows ou Linux,
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
operacional para encerrar o processo** — o `os-rust` *é* tudo o que está
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

## O caminho de uma tecla: do teclado até a tela

A v1 só escrevia na tela. Esta versão responde à pergunta natural que vem
depois: "e dá para digitar?". Para isso, o `os-rust` precisa reagir a um
evento que pode acontecer a qualquer momento — uma tecla sendo pressionada
— sem ficar perguntando "chegou alguma tecla? e agora? e agora?" o tempo
todo (o que gastaria o processador à toa). A solução do hardware para isso
se chama **interrupção**: o processador simplesmente para o que está
fazendo, atende ao evento, e volta para onde estava. É um mecanismo
completamente diferente de "escrever na tela" (que é só memória) — aqui
existe um fluxo de controle real acontecendo fora da nossa função
`kernel_main`.

O caminho completo, de uma ponta a outra:

1. **Você pressiona uma tecla.** O teclado (emulado pelo QEMU como um
   teclado PS/2) envia um código para um pequeno chip da placa-mãe chamado
   **controlador 8042**. Esse código não é a letra em si — é um número que
   identifica *qual tecla física* mudou de estado, chamado **scancode**. O
   8042 deixa esse scancode disponível para leitura em uma porta de
   entrada/saída do processador, a porta `0x60`.
2. **O 8042 avisa o PIC 8259** ("Programmable Interrupt Controller", o chip
   que existe desde o PC original para gerenciar interrupções de hardware)
   de que há um evento de teclado pendente. Esse aviso é a **IRQ1** — a
   linha de interrupção número 1, reservada ao teclado desde os primeiros
   PCs.
3. **O PIC 8259 sinaliza o processador.** Antes disso poder funcionar sem
   confusão, `src/interrupts.rs` reprogramou o PIC para que os números
   (vetores) que ele usa para avisar o processador não colidam com os
   números que o próprio processador já reserva para seus próprios erros
   internos (como "instrução inválida" ou "divisão por zero") — e mascarou
   todas as linhas de interrupção exceto a IRQ1, para que só o teclado
   consiga interromper o processador nesta demonstração.
4. **O processador consulta a IDT** ("Interrupt Descriptor Table"): uma
   tabela, também montada em `src/interrupts.rs`, que diz "quando a
   interrupção de número X acontecer, desvie a execução para esta função
   específica". Para a IRQ1, essa função é o nosso próprio
   `keyboard_interrupt_handler`, escrito em Rust.
5. **O handler lê o scancode da porta `0x60` e devolve o controle
   rapidinho.** De propósito, ele não faz mais nada além disso — nem
   traduz o scancode, nem escreve na tela. Ele só guarda o scancode em uma
   fila pequena e avisa o PIC "atendido" (sem esse aviso, chamado *EOI* —
   *end of interrupt* —, o PIC nunca mais deixaria outra tecla interromper
   o processador). Manter o handler curto evita um problema sutil: se ele
   tentasse escrever na tela bem no meio de uma escrita que o resto do
   programa já estivesse fazendo, os dois poderiam travar um esperando o
   outro para sempre.
6. **De volta ao fluxo principal**, o laço ocioso de `kernel_main` (em
   `src/main.rs`) periodicamente esvazia essa fila e chama
   `src/keyboard.rs` para **traduzir** cada scancode em um caractere ASCII,
   de acordo com o layout de teclado US QWERTY (a mesma tecla física, em um
   teclado ABNT2 brasileiro, produziria um símbolo diferente — por isso o
   projeto documenta o layout no README em vez de tentar adivinhar).
7. **O caractere traduzido chega a `src/shell.rs`**, que decide o que
   fazer com ele: se for uma letra ou símbolo comum, guarda no buffer da
   linha atual e ecoa na tela (reusando o mesmo `Writer` de `0xb8000` da
   v1); se for Enter, interpreta a linha inteira como um comando; se for
   Backspace, apaga o último caractere.

Enquanto nenhuma tecla é pressionada, o processador não fica girando em um
laço vazio consumindo energia à toa: a instrução `hlt` o coloca para
"dormir" até a próxima interrupção — exatamente a mesma IRQ1 que acabamos
de descrever é o que o acorda de novo.

## A porta serial: um segundo canal de texto, só para o host

Até aqui, a única forma de "ver" o que o `os-rust` está fazendo era olhar
a tela do QEMU. Isso funciona bem para uma demonstração ao vivo, mas tem
um problema para quem está depurando um erro: a tela do QEMU não tem
histórico rolável de verdade fora do que já está nela, não dá para
copiar texto dela facilmente, e ela também é a tela que a "plateia" vê —
não queremos poluí-la com mensagens técnicas de diagnóstico.

A solução é um segundo canal de comunicação, completamente separado da
tela: a **porta serial**. Antes de existir rede, todo PC já tinha uma
porta serial (também chamada de "porta COM") — um conector físico que
manda e recebe um byte de cada vez, um cabo simples que ligava dois
computadores (ou um computador e uma impressora) diretamente. O QEMU
emula essa porta e a conecta, do outro lado, ao terminal onde você digitou
`cargo run` ou `cargo test` — é por isso que `Cargo.toml` já tinha, desde
antes deste marco, a configuração `-serial stdio` para o QEMU.

Assim como a tela em modo texto (explicada lá em cima) é só um endereço de
memória especial, a porta serial é controlada através de **portas de
entrada e saída** (*I/O ports*): endereços especiais do processador,
diferentes dos endereços de memória comum, acessados com instruções
específicas (`in`/`out` em assembly; em Rust, o tipo `Port` da crate
`x86_64` esconde esse detalhe). O chip que implementa a porta serial se
chama **UART 16550**, e ele está sempre no mesmo endereço de I/O em um PC:
`0x3F8`. `src/serial.rs` usa a crate `uart_16550` para conversar com esse
chip: construir um `SerialPort` apontando para `0x3F8`, chamar `.init()`
uma vez, logo no início do boot, antes de qualquer outra mensagem de
diagnóstico, e, a partir daí, escrever texto nele é tão parecido com
`println!` quanto possível — por isso as macros se chamam
`serial_print!`/`serial_println!`.

Tem um detalhe importante escondido aí: e se o handler de uma interrupção
(por exemplo, o de breakpoint) precisar escrever na serial *exatamente*
no meio de uma escrita que o fluxo principal do kernel já estava fazendo?
Sem cuidado, os dois ficariam brigando pelo mesmo recurso e travariam um
esperando o outro para sempre (um *deadlock*). A solução, em
`src/serial.rs`, é desabilitar as interrupções durante toda escrita na
serial: se o fluxo principal está no meio de uma escrita, ele
literalmente não pode ser interrompido até terminar, então o handler
nunca chega a competir pelo mesmo recurso enquanto ele está ocupado.

## Um executor de testes sem biblioteca padrão

O mecanismo normal de testes do Rust (o que roda quando você digita
`cargo test` em um projeto comum) depende da biblioteca padrão do Rust
(`std`) — que não existe aqui: o `os-rust` é `#![no_std]` desde o Marco
0, porque não há sistema operacional embaixo para fornecer arquivos,
threads, alocação de memória, etc. Ainda assim, o compilador nightly do
Rust tem um mecanismo pensado exatamente para este caso, chamado
`custom_test_frameworks`: em vez de usar o executor padrão, o projeto
registra a própria função que deve rodar quando alguém marca um item com
o atributo `#[test_case]`.

O nosso executor (`test_runner`, em `src/lib.rs`) é propositalmente
simples: recebe uma lista de tudo que foi marcado com `#[test_case]`,
imprime na serial quantos itens há (`Running <N> tests`), roda cada um na
ordem, e imprime `[ok]` depois de cada um que retornar sem dar erro. Não
há alocação de memória em nenhum ponto disso: a lista de testes é uma
fatia (`&[...]`) montada pelo próprio compilador, de tamanho fixo,
conhecida em tempo de compilação.

Todo o kernel é compilado *duas vezes*: uma vez normal (o binário que
`cargo run` usa) e uma vez em "modo de teste" (o que `cargo test` usa,
ativado pela flag `#[cfg(test)]` espalhada pelo código). Como testes de
integração em `tests/` são arquivos separados que dependem do kernel como
uma biblioteca, o projeto precisou ganhar um `src/lib.rs` novo (além do
`src/main.rs` já existente): a biblioteca contém todos os módulos do
kernel e pode ser reaproveitada tanto pelo binário de produção quanto por
cada teste de integração, cada um dando boot no kernel do zero, no seu
próprio processo QEMU independente.

## Do kernel ao código de saída: como o resultado chega ao host

Rodar os testes dentro do QEMU resolve metade do problema: e como o
`cargo test`, que está rodando no seu computador de verdade (o "host"),
sabe se os testes *dentro* da máquina virtual passaram ou falharam? O
QEMU não lê a mente do kernel — precisa de um sinal explícito.

A resposta é um dispositivo de hardware virtual que o próprio QEMU
oferece para esse propósito, chamado `isa-debug-exit`: um endereço de I/O
(`0xf4`, neste projeto) que, quando o kernel escreve um valor nele, faz o
processo do QEMU **encerrar imediatamente**, usando esse valor para
compor o código de saída do próprio processo QEMU. A fórmula exata é
`(valor << 1) | 1` — então escrever `0x10` faz o QEMU sair com código
`33`, e escrever `0x11` faz o QEMU sair com código `35`. Esses dois
valores (`Success`/`Failed`) foram escolhidos só por serem os mesmos
usados na literatura de referência sobre construir um kernel em Rust — o
importante é que eles não colidem com os códigos de saída que o próprio
QEMU já usa para seus próprios erros internos.

Só que `35` (ou `33`) não são exatamente `0`/`1` — não seria natural para
quem roda `cargo test` esperar decorar esses números. É aí que entra o
`bootimage`, a ferramenta que já empacotava o kernel numa imagem de disco
desde o Marco 0: o `Cargo.toml` deste projeto diz a ela, em
`test-success-exit-code = 33`, "quando o processo do QEMU sair com o
código 33, isso significa sucesso — traduza para o código de saída `0` do
próprio `cargo test`". Qualquer outro código de saída do QEMU (incluindo
`35`, ou o código usado quando o `bootimage` precisa matar o QEMU por
demorar demais) permanece diferente de zero. É por isso que, depois de
`cargo test`, `echo $?` já é suficiente para saber se tudo passou, sem
precisar ler nenhuma linha de texto.

## O papel do tempo máximo e o que acontece com um panic durante um teste

E se um teste nunca terminar — por exemplo, um `loop {}` por engano? Sem
alguma proteção, `cargo test` ficaria esperando para sempre. Por isso
`Cargo.toml` também define `test-timeout = 60`: se um binário de teste
não sinalizar sucesso ou falha dentro desse tempo, o `bootimage` mata o
processo do QEMU sozinho e reporta falha ao `cargo test` — sessenta
segundos é bem mais do que a suíte inteira normalmente leva, mas ainda
assim curto o suficiente para não deixar quem está rodando os testes
esperando por muito tempo.

E um `panic!` no meio de um teste? Como o alvo deste projeto usa
`panic-strategy = "abort"` (definido no target customizado
`x86_64-os_rust.json`), não existe a possibilidade de "capturar" um
panic e continuar executando o resto do programa normalmente, como
aconteceria num programa Rust comum rodando sobre um sistema operacional.
Um panic aqui **encerra o processo inteiro** — então, em modo de teste, o
`#[panic_handler]` (uma versão diferente da usada em `cargo run`, ver
`src/lib.rs`) trata qualquer panic como uma falha: escreve `[failed]`
mais a localização e a mensagem do panic na serial, e sinaliza
`QemuExitCode::Failed` ao host. Como o panic interrompe tudo, nenhum
teste depois dele, no mesmo binário, chega a rodar — exatamente o
comportamento esperado quando algo dá muito errado no meio da suíte.

Essa mesma limitação é o motivo de existir um teste bem diferente dos
outros: `tests/should_panic.rs`. Ele existe para provar que, quando o
kernel *deveria* entrar em panic numa certa situação, ele realmente
entra. Só que, se um panic normal sempre significa "falha", como testar
que um panic *aconteceu como esperado*? A resposta é que esse arquivo não
usa o executor de testes comum: ele é o seu próprio programa completo,
com seu próprio `#[panic_handler]`, que trata o panic como **sucesso**
(porque era exatamente o que se esperava que acontecesse) — e, se a
função sob teste terminar sem dar panic, é isso que vira uma falha.

## Como escrever um teste novo

Um teste de unidade — que mora dentro do próprio módulo que está sendo
testado, como os que já existem em `src/vga_buffer.rs`,
`src/keyboard.rs`, `src/shell.rs`, `src/interrupts.rs` e `src/serial.rs`
— é só uma função sem parâmetros, marcada com `#[test_case]`, dentro de
um bloco `#[cfg(test)] mod tests { ... }` no final do arquivo:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn minha_verificacao() {
        assert_eq!(2 + 2, 4);
    }
}
```

O teste passa se a função terminar normalmente, e falha se qualquer
`assert!`/`assert_eq!` (ou qualquer outro panic) disparar dentro dela.
Um único cuidado: nunca escreva, dentro de um teste comum desses, uma
chamada que você sabe que vai entrar em panic de propósito (como o
comando `panic` do prompt) — isso derrubaria o binário inteiro em vez de
passar, exatamente pelo motivo explicado na seção anterior. Para esse
caso específico, o teste precisa ser um arquivo próprio em `tests/`,
seguindo o modelo de `tests/should_panic.rs`.

Um teste de integração novo é um arquivo novo dentro de `tests/`, com sua
própria cópia mínima do cabeçalho que aparece em
`tests/boot_integration.rs`:

```rust
#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os_rust::test_runner)]
#![reexport_test_harness_main = "test_main"]

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;

entry_point!(main);

fn main(boot_info: &'static BootInfo) -> ! {
    os_rust::init(boot_info);
    test_main();
    os_rust::panic::halt_loop();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    os_rust::test_panic_handler(info)
}

#[test_case]
fn meu_teste_de_integracao() {
    // ... o resto do cenário sob teste
}
```

Cada arquivo em `tests/` dá boot no kernel do zero, no seu próprio
processo QEMU — por isso `os_rust::init(boot_info)` precisa ser chamado
de novo em cada um, mesmo que o teste anterior já o tenha chamado no seu
próprio binário. Desde o Marco 3, `init` recebe o `boot_info` que o
próprio `entry_point!` entrega (o mapa de memória e o deslocamento da
física completa que a inicialização de memória precisa).

## Memória física: o mapa que o bootloader entrega

Até aqui, tudo que o kernel usou — a tela, o teclado, a serial — são
endereços fixos e conhecidos de antemão. Memória é diferente: quanta RAM
a máquina tem, e quais pedaços dela já estão ocupados (pelo próprio
kernel, pelas tabelas de páginas, por dispositivos), só o bootloader sabe
dizer, porque é ele quem conversa com a BIOS para descobrir isso antes do
nosso código sequer começar a rodar.

É por isso que a crate `bootloader` entrega, junto com a chamada de
`entry_point!`, uma estrutura `BootInfo` com um `memory_map`: uma lista de
regiões da memória física, cada uma com um início, um fim e um tipo —
`Usable` (livre, o kernel pode usar), `Kernel`, `PageTable`, `Reserved`,
e outros. O `memory_map` é só leitura: o kernel nunca escreve nele, só
consulta. `src/memory.rs` usa exatamente essa lista, e só ela, para saber
o que pode entregar como memória livre — nunca um palpite, nunca uma
suposição sobre o tamanho da RAM.

## Frames e páginas: os blocos de 4 KiB dos dois lados

A memória física é dividida em blocos de 4 KiB chamados **frames**; o
espaço de endereços que o processador enxerga (memória *virtual*) é
dividido nos mesmos 4 KiB, chamados **páginas**. Um frame é identificado
só pelo endereço físico onde começa; uma página, pelo endereço virtual.
A ideia central de memória virtual é que cada página pode estar mapeada
em qualquer frame — ou em nenhum — e é o processador, consultando uma
estrutura chamada tabela de páginas, quem faz essa tradução a cada acesso
à memória, de forma transparente para o código que só enxerga endereços
virtuais.

`src/memory.rs` define `BootInfoFrameAllocator`: ele percorre as regiões
`Usable` do `memory_map`, quebra cada uma em frames de 4 KiB, e entrega um
frame novo a cada chamada de `allocate_frame()` — sem nunca repetir um já
entregue, e devolvendo `None` quando a memória utilizável acaba (isso é
tudo que o tipo `Option` já garante: não existe um "frame inválido"
disfarçado de frame válido). Devolver frames ao alocador não é algo que
este marco precisa fazer: o único consumidor é o heap, montado uma única
vez no boot.

## A tabela de páginas de 4 níveis, e como o kernel consegue editá-la

No `x86_64`, a tradução de um endereço virtual para um físico passa por
quatro níveis de tabelas (apelidadas de P4, P3, P2 e P1), cada uma com
512 entradas — cada entrada aponta para a tabela do nível seguinte, até a
última (P1) apontar para o frame físico de fato. O processador sabe onde
está a P4 ativa porque o endereço físico dela fica guardado num
registrador especial, `CR3`.

Só que ler ou editar essas tabelas exige acessar a memória física onde
elas vivem — e o código do kernel só enxerga endereços *virtuais*. É aqui
que entra a mesma peça central deste marco: a feature `map_physical_memory`
da crate `bootloader` (habilitada em `Cargo.toml`) faz o bootloader mapear
**toda** a memória física, de uma vez, começando em um endereço virtual
fixo — o `physical_memory_offset`, também entregue dentro do `BootInfo`.
Assim, para acessar o frame físico que começa no endereço `F`, basta
somar `F + physical_memory_offset` e usar esse resultado como um endereço
virtual comum. `active_level_4_table`, em `src/memory.rs`, usa exatamente
essa soma para chegar até a P4 ativa (lida de `CR3`) e devolver uma
referência mutável a ela.

Em vez de caminhar os quatro níveis manualmente toda vez, o kernel usa
`OffsetPageTable`, um tipo pronto da crate `x86_64` que já sabe fazer essa
aritmética — dado o `physical_memory_offset` e a P4 ativa, ele oferece
`translate_addr` (endereço virtual → físico, ou `None` se a página não
tem mapeamento — `memory::translate_addr`) e `map_to` (cria um mapeamento
novo, criando as tabelas intermediárias que faltarem — usado por
`memory::map_page`).

## Como um mapeamento novo é criado

Mapear uma página nova em um frame físico livre é: pegar um frame do
`BootInfoFrameAllocator`, e chamar `map_to` nele, com as flags `PRESENT`
(a página existe) e `WRITABLE` (pode ser escrita) — nunca
`USER_ACCESSIBLE` neste marco, porque programas de usuário em ring 3 só
chegam no Marco 5. Se a página já tivesse mapeamento, `map_to` devolve um
erro explícito (`PageAlreadyMapped`) em vez de sobrescrever silenciosamente
o que já estava lá — um mapeamento apontando para o frame errado seria um
tipo de bug muito difícil de rastrear depois.

`memory::map_page` empacota esses passos numa função só, usada tanto
pelos testes de integração (`tests/paging.rs`) quanto, indiretamente,
pela montagem do heap a seguir.

## O heap: por que o kernel precisa de memória dinâmica

Até este marco, toda estrutura de dados do kernel tinha um tamanho fixo,
conhecido em tempo de compilação — o buffer da tela, a fila de scancodes,
o buffer de linha do prompt. Isso funciona bem quando dá para prever o
tamanho máximo de antemão, mas quebra assim que o kernel precisa de algo
cujo tamanho só se sabe em tempo de execução (por exemplo, no Marco 5, a
lista de segmentos de um executável ELF, que varia de programa para
programa). É exatamente para isso que serve um **heap**: uma região de
memória de onde o programa pode pedir blocos de tamanho variável — os
tipos `Box` (um valor único, alocado) e `Vec` (uma lista que cresce) da
crate `alloc` da própria biblioteca padrão do Rust são a forma idiomática
de usar essa memória.

O heap deste kernel é simples de propósito: uma faixa **fixa** de 100 KiB
de endereços virtuais, começando em `0x_4444_4444_0000` (um endereço
escolhido só por estar bem longe de qualquer outra coisa que o bootloader
já tenha mapeado) — `src/allocator.rs`. No boot, `allocator::init_heap`
mapeia, uma por uma, todas as páginas dessa faixa em frames físicos livres
(reaproveitando exatamente o `map_page`/`map_to` explicados acima) e só
depois registra o alocador global. Essa ordem importa: nenhuma alocação
pode acontecer antes do heap inteiro estar mapeado.

## Como o alocador de heap escolhido funciona

Registrar um alocador global significa dizer ao compilador Rust: "sempre
que algum código pedir `alloc`/`dealloc` — o que `Box::new`, `Vec::push` e
companhia fazem por baixo dos panos — chame esta função". `src/allocator.rs`
usa a crate `linked_list_allocator` para isso, através do atributo
`#[global_allocator]`. O algoritmo por trás dela é uma lista encadeada de
blocos livres: cada bloco liberado (quando um `Box`/`Vec` sai de escopo)
volta para essa lista, disponível para a próxima alocação que couber nele
— **mesmo que outros blocos ainda estejam em uso**, o que é justamente o
que faz o comando `mem` poder ser digitado 100 vezes seguidas sem nunca
esgotar a memória: cada `Box`/`Vec` que ele cria é devolvido ao final da
mesma execução do comando.

Este marco optou por usar uma crate pronta para esse algoritmo, em vez de
escrever um alocador à mão, porque o objetivo didático aqui é entender
memória física e paginação — o algoritmo interno de um alocador de heap
(listas livres, blocos de tamanho fixo, e as várias formas de otimizá-los)
é um assunto por si só, que não precisa competir pelo tempo de aula deste
marco.

E se um pedido de alocação não couber em nenhum espaço livre do heap? O
compilador chama a função marcada com `#[alloc_error_handler]`, também em
`src/allocator.rs` — que aqui simplesmente chama `panic!` informando o
tamanho e o alinhamento pedidos, reaproveitando a mesma tela de panic
legível (e a mesma linha na serial) que já existe desde o Marco 0. Um
heap esgotado nunca trava o sistema silenciosamente nem devolve memória
inválida — ele para de um jeito que dá para ler e entender.

## O que o comando `mem` está mostrando

`mem` (em `src/shell.rs`) é a prova, na tela, de tudo isso funcionando
junto:

```text
os-rust> mem
memoria fisica utilizavel: 123848 KiB
heap: 0x444444440000, 100 KiB
Box: valor=42, endereco=0x444444440000
Vec: tamanho=10, capacidade=16, soma=55
os-rust>
```

A primeira linha soma todas as regiões `Usable` do `memory_map` do
bootloader (`memory::info()`) — não é a RAM total da máquina, é só a
parte que sobrou livre depois do que o próprio bootloader e o BIOS já
reservaram. A segunda mostra onde e do que tamanho é o heap descrito
acima. As duas últimas alocam, de fato, um `Box` e um `Vec` — o endereço
do `Box` cai dentro da faixa do heap (frequentemente bem no início dela,
já que é a primeira alocação depois do boot), e o `Vec` começa vazio
(`Vec::new()`) e cresce a cada `push`, então sua capacidade final pode
ser maior que seu tamanho (a estratégia de crescimento do `Vec` da
biblioteca padrão dobra a capacidade quando ela se esgota, em vez de
realocar a cada elemento).

## Exceções da CPU: quando o próprio processador interrompe o código

Desde o Marco 1, o kernel já lida com **interrupções**: eventos externos
assíncronos, como uma tecla pressionada (IRQ1) — algo que pode acontecer
a qualquer momento, sem relação com a instrução que estava rodando.
**Exceções** são parecidas (usam a mesma tabela, a IDT, e o mesmo
mecanismo de hardware para desviar a execução), mas são **síncronas**: o
próprio processador as dispara, no meio da execução de uma instrução
específica, porque essa instrução tentou fazer algo que não pode —
acessar uma página de memória não mapeada, executar um opcode que não
existe, violar uma regra de proteção. Cada exceção tem um vetor fixo na
IDT (os primeiros 32, reservados pela arquitetura — é por isso que as
IRQs de hardware começam no vetor 32, como o Marco 1 já explicou) e,
para algumas delas, o processador empilha um **código de erro** de 64
bits com detalhes sobre o que deu errado, antes mesmo de chamar o
handler.

Até este marco, o kernel só tratava duas: breakpoint (`#BP`, vetor 3,
disparada pela instrução `int3`) e double fault (`#DF`, vetor 8). Todas
as outras exceções — inclusive page fault e instrução inválida — não
tinham handler nenhum. E é exatamente aí que mora o problema que este
marco resolve.

## O que acontecia sem handler: double fault em cascata, depois triple fault

Quando o processador tenta entregar uma exceção e a entrada
correspondente da IDT está vazia (ou, pior, quando a própria entrega da
exceção falha por algum outro motivo, como uma pilha inválida), ele não
desiste — ele dispara uma segunda exceção, o **double fault** (`#DF`),
dando ao kernel uma última chance de reagir. Um estouro de pilha é
exatamente esse caso: a instrução que tenta empilhar mais um quadro
sobre uma pilha já cheia provoca uma falha de página (a próxima posição
da pilha não tem uma página válida por baixo) — e, como o próprio
handler dessa falha de página *também* precisa de espaço de pilha para
rodar, e não há mais nenhum disponível, o processador não consegue nem
entregar o handler de page fault. Isso vira um double fault.

Mas até este marco, o double fault também rodava na *mesma* pilha (a do
kernel) — e se ela já estava esgotada, tentar entregar o *handler* do
double fault também falhava. Quando isso acontece dentro de um double
fault, o processador não tem mais para onde escalar: ele dispara um
**triple fault**, que nenhum sistema operacional trata — é tratado pelo
próprio hardware como "o sistema está irrecuperável", e a reação padrão
de qualquer PC real (e do QEMU, simulando um) é reiniciar a máquina
imediatamente, sem nenhuma mensagem. É exatamente o reboot silencioso
que quem já mexeu com este projeto antes deste marco conhece.

## A GDT: por que ainda existe em modo 64 bits

A **GDT** (*Global Descriptor Table*) é uma peça herdada do modo
protegido de 32 bits, onde ela definia segmentos de memória com bases,
limites e permissões próprios — *segmentação*. Em modo longo (64 bits),
a segmentação de dados está essencialmente desligada: todo endereço já é
tratado como se começasse em zero. Mas a GDT continua existindo, porque
o processador ainda exige um seletor de segmento de código válido no
registrador `CS`, e é nela que o **descritor da TSS** (a seguir) precisa
morar — a instrução `ltr` (*load task register*), que ativa a TSS, só
sabe carregar um seletor que aponta para uma entrada da GDT.

Este marco cria `src/gdt.rs` com uma GDT própria do kernel, contendo só
duas entradas: o segmento de código do kernel (`Descriptor::kernel_code_segment()`,
usado para recarregar `CS`) e o descritor da TSS
(`Descriptor::tss_segment(&TSS)`). Segmentos de usuário (ring 3) ficam
para o Marco 5, quando modo usuário exigir um segmento de código e um de
dados próprios para rodar programas fora do kernel (foi o que aconteceu:
ver o capítulo "Marco 5" no fim deste texto, onde a GDT ganha quatro
descritores a mais).

## A TSS e a Interrupt Stack Table: a pilha que resolve o problema

A **TSS** (*Task State Segment*), em modo 64 bits, não serve mais para
trocar de tarefa (como fazia em 32 bits) — sobrou dela só um propósito:
guardar pilhas que o processador troca automaticamente em certas
situações. Uma dessas é a **Interrupt Stack Table** (IST): até sete
ponteiros de pilha, cada um podendo ser associado a uma entrada
específica da IDT. Quando essa associação existe, o processador troca
para aquela pilha *antes* de chamar o handler — independentemente de
qual pilha estava em uso no momento da falha.

`src/gdt.rs` reserva uma região estática de 20 KiB
(`DOUBLE_FAULT_STACK`, cinco páginas de 4 KiB — folga generosa sobre o
que o handler realmente precisa: só formatar e escrever uma mensagem de
texto, sem alocar nada do heap) e guarda o endereço do **fim** dela (a
pilha cresce para baixo) na entrada 0 da IST, dentro da TSS. A IDT então
associa essa entrada ao double fault:

```rust
unsafe {
    idt.double_fault
        .set_handler_fn(double_fault_handler)
        .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
}
```

Com isso, mesmo que a pilha do kernel esteja completamente estourada
quando o double fault acontece, o processador troca para essa pilha
reservada, intacta, antes de entrar no handler — o handler consegue
rodar, formatar a mensagem e escrevê-la (tela e serial) normalmente. É
exatamente o "antes e depois do reboot": sem a IST, estouro de pilha =
triple fault = reinício silencioso; com ela, estouro de pilha = tela de
`#DF` legível, kernel parado de forma controlada.

## A tela de exceção fatal: uma função, quatro exceções

`#UD` (instrução inválida), `#GP` (proteção geral), `#PF` (page fault) e
`#DF` (double fault) são tratadas como **fatais**: o kernel ainda não
sabe rodar programas de usuário (isso só chega no Marco 5), então não há
"só encerrar o programa culpado" — a única resposta seria seguir
rodando um kernel que acabou de provar que está em um estado
inesperado. As quatro compartilham uma única função privada,
`fatal_exception`, em `src/interrupts.rs`: ela escreve nome e sigla da
exceção, o endereço da instrução que falhou, o código de erro (quando a
exceção tem um) e a versão do os-rust — tudo espelhado na tela e na
serial — e termina parando a CPU (`halt_loop`, o mesmo laço de `hlt`
já usado pela tela de panic desde o Marco 0). Breakpoint (`#BP`)
continua a única exceção não fatal: a tela ganha só uma linha curta, e o
controle volta ao prompt normalmente.

## Como ler o código de erro e o endereço de um page fault

Page fault é a única das cinco exceções deste marco que carrega
informação extra o bastante para valer a pena traduzir em palavras. O
processador empilha um código de erro de 64 bits (`PageFaultErrorCode`,
um conjunto de *bit flags*) e guarda, no registrador `CR2`, o endereço
virtual exato que causou a falha — `fatal_exception` lê os dois e monta
uma mensagem como esta (tela e serial têm o mesmo conteúdo):

```text
[EXCEPTION] Page Fault (#PF) - os-rust v0.4.1 parou
endereco da instrucao: 0x...
codigo de erro: 0x0
endereco de falha: 0x444444459000
acesso: leitura
causa: pagina ausente
os-rust v0.4.1
```

Dois bits do código de erro bastam para a interpretação em palavras: o
bit `CAUSED_BY_WRITE` distingue leitura de escrita, e o bit
`PROTECTION_VIOLATION` distingue "página ausente" (não havia mapeamento
nenhum ali) de "violação de proteção" (havia mapeamento, mas o acesso
não respeitava suas flags — por exemplo, escrever numa página só de
leitura). O comando `falha pagina` do prompt provoca exatamente esse
cenário de propósito: lê um byte do endereço logo após a última página
mapeada do heap (`allocator::HEAP_START + allocator::HEAP_SIZE`) — um
endereço que o Marco 3 nunca mapeia, então o page fault é garantido e
previsível.

## O comando `falha <tipo>`: provocando cada exceção de propósito

`src/shell.rs` ganha o comando `falha <tipo>`, com cinco tipos, cada um
provocando sua exceção da forma mais simples possível de explicar:

- `falha pagina` — leitura num endereço nunca mapeado (acima).
- `falha pilha` — uma função recursiva sem caso base. Sozinha, essa
  recursão viraria um laço infinito sem nunca estourar a pilha, porque o
  compilador otimizaria a chamada recursiva em cauda (*tail call*) para
  um `jmp` que reaproveita o mesmo quadro; uma leitura volátil
  (`volatile::Volatile::new(0u8).read()`) depois da chamada recursiva
  impede essa otimização, forçando cada chamada a empilhar de verdade.
- `falha opcode` — a instrução `ud2`, reservada pelo próprio manual da
  Intel/AMD como sempre inválida, sem precisar de nenhum truque.
- `falha protecao` — escreve num endereço virtual *não canônico*: em
  modo longo, os bits 63 a 47 de todo endereço de 64 bits precisam ser
  todos iguais (uma extensão de sinal do bit 47); o endereço
  `0x8000_0000_0000_0000` viola essa regra de propósito (bit 63 ligado,
  bit 47 desligado), o que o processador rejeita como `#GP` antes mesmo
  de chegar a verificar se a página existe.
- `falha breakpoint` — a instrução `int3`, a mesma exceção não fatal que
  o Marco 1 já demonstrava.

Sem argumento, ou com um tipo que não é nenhum destes cinco, o comando
lista os tipos disponíveis em vez de provocar qualquer coisa.

## Como o nome e a versão chegam do `Cargo.toml` até a tela

Um pedido menor, mas visível em toda aula: nome e versão do sistema
precisam aparecer nas mensagens dele. A técnica é inteiramente em tempo
de compilação, sem nenhum código de formatação em tempo de execução:
`env!("CARGO_PKG_NAME")` e `env!("CARGO_PKG_VERSION")` são macros do
próprio `cargo`/`rustc` que expandem para strings literais com os
valores dos campos `name`/`version` de `Cargo.toml` — e `concat!` junta
essas strings numa única constante, em `src/lib.rs`:

```rust
pub const NAME: &str = env!("CARGO_PKG_NAME");
pub const VERSION: &str = concat!(env!("CARGO_PKG_NAME"), " v", env!("CARGO_PKG_VERSION"));
```

Como todas são macros resolvidas pelo compilador, `NAME`/`VERSION` já
nascem como strings completas (`"os-rust"`/`"os-rust v0.4.1"`) dentro do
binário compilado — não há leitura de arquivo, não há alocação, e não
existe nenhum outro lugar do código onde o nome ou a versão apareçam
escritos à mão. Todo lugar que precisa mostrar um dos dois — a mensagem
de boas-vindas (`print_welcome`, chamada por `main.rs`), a primeira
linha de diagnóstico da serial, o prompt (`shell::PROMPT`), o comando
`sobre`, a tela de panic e a tela de exceção fatal — só lê
`crate::NAME`/`crate::VERSION`. Mudar o nome ou a versão vira, então, uma
mudança em um único lugar: os campos `name`/`version` de `Cargo.toml`.

## Marco 4.1: por que renomear um projeto mexe em mais lugares do que parece

O Marco 4.1 trocou o nome do projeto de `proto-os` para `os-rust`. Numa
linguagem comum, "renomear um projeto" seria só editar um texto. Em
Rust, o nome de um pacote vira, por conta própria, vários nomes
diferentes em lugares diferentes — vale a pena entender por quê.

**Por que o pacote `os-rust` vira a crate `os_rust`.** O campo `name` de
`Cargo.toml` pode ter hífen (`"os-rust"`), porque ali ele é só um texto
dentro de um arquivo TOML. Mas um identificador de código Rust (o nome
que aparece depois de `use` ou antes de `::`) nunca pode ter hífen — a
gramática da linguagem não permite. Por isso o Cargo troca
automaticamente cada `-` por `_` ao gerar o nome da *crate* (a unidade de
compilação): o pacote `os-rust` compila para a crate `os_rust`, e é
`os_rust::algumacoisa` que aparece em todo `use` do projeto (`src/main.rs`,
cada arquivo de `tests/`). As duas grafias convivem de propósito: o
manifesto usa a grafia "bonita", o código usa a grafia que compila.
Só que existe uma terceira grafia, que não passa por essa troca: a macro
`env!("CARGO_PKG_NAME")` (usada na seção anterior) expande para o texto
**literal** do campo `name` do manifesto — com o hífen preservado. É por
isso que `NAME`/`PROMPT`, embora vivam dentro do código Rust, mostram
`"os-rust"` (com hífen) na tela, e não `"os_rust"`.

**Por que o nome do arquivo do target decide o nome da pasta dentro de
`target/`.** O arquivo `x86_64-os_rust.json` (renomeado de
`x86_64-proto_os.json` neste marco) descreve, para o `rustc`, como é a
CPU/plataforma de destino — não existe um target `x86_64-os_rust`
"oficial" do Rust, então este projeto define o seu próprio, com o
`json-target-spec` habilitado em `.cargo/config.toml`. O Cargo usa o
**nome do arquivo** (sem a extensão `.json`) como identificador desse
target, e cria uma subpasta com esse mesmo nome dentro de `target/` para
guardar tudo que foi compilado para ele — daí `target/x86_64-os_rust/`.
Renomear só o arquivo (sem tocar em mais nada) já seria suficiente para
o Cargo passar a usar a subpasta nova; uma pasta antiga
`target/x86_64-proto_os/`, se sobrar de uma compilação anterior, fica
simplesmente órfã, sem atrapalhar nada.

**Por que `█` em UTF-8 não é o mesmo que o byte `0xDB` da página de
código 437.** O logo novo (`src/logo.txt`, mostrado na tela por
`vga_buffer::draw_logo`) usa o caractere `█` (bloco cheio) nas linhas do
símbolo. O arquivo-fonte é UTF-8, o padrão universal de hoje, em que cada
caractere pode ocupar de 1 a 4 bytes — `█` especificamente ocupa 3 bytes
(`0xE2 0x96 0x88`). Mas o hardware de texto do VGA (o mesmo `0xb8000`
explicado lá em cima) não entende UTF-8: cada posição da tela é *um*
byte de caractere mais um byte de cor, usando uma tabela de 256 símbolos
fixos chamada "página de código 437" (a mesma herdada do PC original dos
anos 80), em que a posição `0xDB` é justamente um bloco cheio — um
símbolo diferente do `█` do Unicode, mas visualmente idêntico. Se o
kernel escrevesse os 3 bytes UTF-8 de `█` direto na tela (como o
`Writer` normal faz com texto comum), apareceriam três símbolos errados
lado a lado, não um bloco só. Por isso `draw_logo` não usa o `Writer`
normal: ele lê o logo **caractere Unicode por caractere Unicode**
(`str::chars()`, não `str::bytes()`) e traduz cada `'█'` para o único
byte `0xDB`, escrevendo direto nas células do buffer VGA.


## Marco 5: o primeiro programa de usuário

Até o Marco 4.1 o os-rust só executava código do próprio kernel. O Marco 5
faz a coisa que dá sentido a todo o resto: executa um programa escrito
**fora** do kernel, num nível de privilégio mais baixo, que só consegue
escrever na tela pedindo ao kernel. Você o vê digitando `run hello` no
prompt. Este capítulo explica o caminho inteiro, na ordem em que as peças
entram em ação.

### Por que o programa é compilado separadamente

O kernel é um binário `no_std` para o target `x86_64-os_rust.json`, com
detalhes que só fazem sentido dentro do kernel (por exemplo, o
`code-model = kernel` e a zona vermelha da pilha desligada, porque uma
interrupção pode chegar a qualquer momento em cima da pilha do kernel). Um programa de
usuário é outra coisa: roda em ring 3, em endereços baixos, numa pilha
que o kernel nunca usa. Por isso ele tem o **seu próprio target**,
`x86_64-os_rust_user.json`, e a sua própria crate, `programs/`, com um
executável por arquivo em `programs/src/bin/` (`hello.rs`, `crash.rs`).

Um detalhe que aparece quando se escolhe onde o programa vai morar: o
linker script `programs/link.ld` fixa a base em `0x4000_0000` (1 GiB).
Parece arbitrário, mas não é: o modelo de código padrão do Rust gera
endereços absolutos de 32 bits com sinal, então o programa precisa caber
abaixo de 2 GiB. Numa tentativa com uma base bem mais alta o linker falhou
com `relocation R_X86_64_32 out of range`. Cada grupo de seções (`.text`,
`.rodata`, `.data`) começa numa página de 4 KiB própria, para que o
carregador possa dar permissões diferentes a cada uma.

### Como o executável chega dentro do kernel

O kernel não compila nada enquanto roda: é um binário já pronto. Então o
programa precisa estar **dentro** dele. Isso é feito em tempo de
compilação, e sem nenhum passo manual, por um `build.rs` na raiz do
repositório (um *build script*: um programa Rust que o Cargo roda antes de
compilar o pacote). Esse script:

1. roda um segundo `cargo build`, com o target de usuário, dentro de
   `programs/`;
2. copia cada ELF resultante para a pasta de saída do build (`OUT_DIR`);
3. escreve, também em `OUT_DIR`, um arquivo `programs.rs` com uma tabela
   (nome do programa + os bytes, via `include_bytes!`), que
   `src/programs.rs` inclui com `include!`.

O mesmo mecanismo que já embute o `src/logo.txt` como texto, agora
embute um binário compilado. O `rerun-if-changed` faz o Cargo refazer tudo
sempre que algo em `programs/` muda: por isso o `hello` embutido nunca fica
desatualizado, e não precisa de `cargo clean`.

Rodar um `cargo` dentro de um build script tem três armadilhas, todas
resolvidas em `build.rs` e comentadas ali: o `cargo` de dentro usa um
`--target-dir` próprio (o de fora segura o *lock* do diretório de build, e
esperar por ele seria esperar para sempre); o ambiente que o `cargo` de
fora injeta (flags, target, perfil) é apagado, para o programa de usuário
não herdar as opções do kernel; e `programs` precisa ser membro do
*workspace* declarado no `Cargo.toml` (`default-members = ["."]` mantém
`cargo run` e `cargo test` compilando só o kernel).

Se o programa não compila, o `build.rs` termina com `panic!` **antes** de
copiar qualquer arquivo: o `cargo run` para com o erro do compilador à
vista, e nunca sai uma imagem de boot com um programa velho por engano.

### O que é um ELF e o que o carregador lê

O arquivo que o `cargo` interno produz é um **ELF64**: o formato de
executável dos sistemas Unix. Ele começa com um cabeçalho de 64 bytes
(a "magia" `7f 45 4c 46`, se é de 32 ou 64 bits, para qual CPU, onde fica
a primeira instrução) e uma tabela de **program headers**, que dizem o que
copiar para onde. Cada `PT_LOAD` descreve um pedaço do programa: um
endereço virtual, quantos bytes copiar do arquivo, quantos bytes ele ocupa
na memória (o que passa do arquivo é zerado: a `.bss`) e se é legível,
gravável, executável.

`src/elf.rs` lê esses campos com `from_le_bytes`, sem nenhuma crate, e
recusa, antes de mapear qualquer coisa, tudo que não seja um ELF64
estático para x86-64, ou cujos segmentos saiam da região do usuário,
dividam uma página ou tenham a entrada fora de código executável. Depois,
`src/user.rs` faz o trabalho: para cada página de cada segmento, pega um
frame físico, zera, copia os bytes e mapeia com as permissões do
segmento. Uma página de código nasce sem o bit de escrita, e uma página de
dados ou de pilha nasce com o bit `NO_EXECUTE`: nenhuma página é ao mesmo
tempo gravável e executável (a regra **W^X**). O kernel escreve nos
frames pela janela do mapa completo da memória física, e não pelo endereço
do usuário, para poder preencher uma página que ficará somente-leitura.

### Anéis de proteção e a GDT com segmentos de usuário

O x86 tem quatro *anéis* de privilégio; sistemas modernos só usam dois:
o **ring 0** (o kernel, que pode tudo) e o **ring 3** (programas, que não
podem tocar em hardware nem em memória do kernel). O anel atual é o nível
de privilégio do seletor de código carregado em `CS`. Por isso a GDT
(capítulo anterior) ganha, no Marco 5, quatro descritores: dados do
kernel, dados do usuário e código do usuário (além do código do kernel e da
TSS que já existiam). A ordem é imposta pelo hardware: os dados do usuário
precisam vir **antes** do código do usuário, e os dados do kernel logo
**depois** do código do kernel, porque as instruções de entrada e saída
de syscall calculam o seletor de um a partir do outro (`STAR + 8`,
`STAR + 16`). A crate `x86_64` confere essa disposição quando o kernel
grava o registrador `STAR`.

### A instrução `syscall`

Um programa de usuário não pode chamar uma função do kernel: o kernel está
em outro anel. A forma de pedir um serviço é a instrução `syscall`: a CPU
sobe para ring 0 e salta para um endereço que o kernel registrou antes,
no registrador `LSTAR`. Quatro registradores configuram o mecanismo
(`src/syscall.rs`):

- `EFER.SCE`: liga a instrução;
- `STAR`: os seletores de segmento de kernel e de usuário;
- `LSTAR`: o endereço do stub de entrada (`syscall_entry`);
- `SFMASK`: as flags que a CPU desliga sozinha ao entrar (interrupções,
  direção, *trace*).

A convenção (documentada em `SYSCALLS.md`) é: número da syscall em `rax`,
argumentos em `rdi`, `rsi`, `rdx`, resultado em `rax`. A instrução destrói
`rcx` (onde a CPU guarda o endereço de retorno) e `r11` (onde guarda as
flags), e o contrato manda o programa tratar os dois como destruídos; o
kernel preserva todo o resto.

Há uma pegadinha: a instrução `syscall`, diferente de uma exceção, **não
troca de pilha**. Ao chegar ao kernel, `rsp` ainda aponta para a pilha do
programa, que o kernel nunca deve usar (o programa poderia ter posto lá
qualquer valor). Por isso o stub de assembly guarda o `rsp` do programa
num lugar seguro e troca à mão para a pilha do kernel (a mesma que a TSS
declara em `rsp0`, usada pelo processador quando uma *exceção* chega em
ring 3), empilha o estado do programa (desde o Marco 7, um `TaskContext`
inteiro, com os 15 registradores e o frame de retorno), chama o despachante em
Rust e volta com `iretq`. (Até o Marco 6 o stub empilhava só os registradores
que o contrato promete preservar e voltava com `sysretq`; o `iretq` permite
devolver também um programa que foi tirado da CPU no meio, ver o capítulo do
Marco 7.)

### Como o programa volta ao prompt

O comando `run` é uma função comum do kernel, chamada pelo laço do prompt.
Como ela "espera" um programa que roda em outro anel? Com o mesmo truque de
`setjmp`/`longjmp`, do C. `enter_user` (assembly) empilha os registradores
que o kernel precisa preservar, guarda o `rsp` do kernel e entra no
escalonador, que coloca o programa em ring 3 (o contexto inicial zera todos os
registradores: nada do kernel pode vazar para o programa). Quando o programa
chama `exit`, ou faz algo errado, o kernel está numa pilha de entrada, dentro
de um handler; ao terminar o último programa, `leave_user` restaura o `rsp`
guardado, desempilha os registradores e executa `ret`, que retorna a quem
chamou `enter_user`, como se ele tivesse terminado normalmente. Antes disso, o
escalonador destruiu o espaço de endereçamento do programa e devolveu os frames
ao alocador: rodar `hello` mil vezes não esgota a memória. Quem mostra o
resultado na tela é o comando `run`, num único lugar (no Marco 7, na hora em
que cada programa termina).

### Por que o kernel não confia no ponteiro de `write`

`write(ptr, len)` recebe um endereço e um tamanho que **o programa**
escolheu. Se o kernel lesse `ptr` sem olhar, um programa poderia pedir
para "escrever" um endereço do próprio kernel e enxergar (ou provocar) o
que quisesse. Por isso `sys_write` confere, antes de ler qualquer byte,
que o intervalo inteiro está dentro da região do usuário e que **cada
página** dele está mapeada e marcada como acessível ao usuário; se não, a
chamada devolve `ERR_FAULT` (`-1`) sem escrever nada, e o programa
continua rodando. Esse é o ponto exato em que o kernel se protege do
programa.

### Por que `#DE`, `#SS` e `#NP` precisaram de handler

Uma exceção de CPU num programa de usuário não pode derrubar o kernel.
Os handlers do Marco 4 (`#UD`, `#GP`, `#PF`) passam a olhar de qual anel a
exceção veio (o seletor de código empilhado pela CPU): em ring 3, só o
programa é encerrado, com uma mensagem legível; em ring 0 continuam
fatais, como antes. Mas não bastava isso. Um gate ausente na IDT não
vira um erro comum: o processador levanta outra exceção, sem handler,
e escala para **double fault**. Foi confirmado no teste: com o gate de
divisão por zero (`#DE`) removido de propósito, o teste da divisão por
zero terminou em `Double Fault (#DF)`, que é fatal para o kernel. Por isso
`#DE`, `#SS` e `#NP` também ganharam handler. (Uma curiosidade do QEMU: um
`push` com o `rsp` não canônico chega como `#PF`, e não como `#SS`, que é
o que um processador real levantaria; o teste aceita os dois.)

### Um detalhe sobre o bit `USER_ACCESSIBLE`

Para uma página ser acessível em ring 3, o bit `USER_ACCESSIBLE` precisa
estar ligado em **todos** os níveis da tabela de páginas, não só na
página final. A região do usuário fica dentro da mesma entrada de nível 4
(a de número 0) que o kernel usa, então mapear a primeira página do
usuário liga esse bit também nessa entrada. Isso não expõe o kernel: as
páginas do kernel ficam sob outra entrada de nível 3 (o kernel vive nos
primeiros MiB, o programa a partir de 1 GiB) e as suas entradas de página
não têm o bit, então o processador continua barrando qualquer acesso de
ring 3 a elas. É a combinação de todos os níveis que decide.

## Marco 6: a interface de programação

No Marco 5 o `hello` pedia tudo ao kernel com a instrução `syscall` escrita à
mão. Isso prova que o mecanismo funciona, mas ninguém escreve um programa
assim. O Marco 6 transforma o mecanismo numa **interface de programação**:
uma biblioteca (`runtime/`) que esconde a instrução, duas syscalls novas
(ler uma linha do teclado e pedir memória) e a garantia, agora demonstrada
com um programa de propósito, de que um erro no programa nunca derruba o
kernel. Você vê tudo digitando `run eco` e `run falha_memoria` no prompt.
Quem quiser escrever o próprio programa, sem entender nada disto por
dentro, lê o `GUIA_DO_PROGRAMADOR.md`; aqui está o que acontece por baixo.

### Uma biblioteca de runtime: o equivalente da libc

Um programa de usuário não tem biblioteca padrão: nem `println!`, nem `Box`,
nem sequer um `main`. A crate `runtime/` dá isso a ele, e é o que a libc é
para um programa em C. Ela tem quatro arquivos:

- `sys.rs`: o **único** lugar com `asm!` de `syscall`. É o mesmo `asm!` que o
  `hello` do Marco 5 tinha escrito à mão, agora escrito uma vez só, com um
  wrapper fino por syscall (`write`, `exit`, `read_line_raw`, `alloc`).
- `io.rs`: `print!` e `println!` (que fazem `write`) e `read_line`.
- `heap.rs`: o alocador global, que faz `Box`, `Vec` e `String` funcionarem.
- `lib.rs`: a macro `entry!` e o tratador de `panic!`.

Os números das syscalls não são escritos duas vezes: vivem numa crate
minúscula, `abi/`, de que dependem **o kernel e a biblioteca**. Assim os
dois lados nunca divergem (o teste do kernel que compara o `SYSCALLS.md` com
as constantes protege também a biblioteca).

### O `_start` e a macro `entry!`

O processador começa a executar o programa no endereço que o ELF chama de
ponto de entrada, `_start`. Um `main` de Rust não é isso: alguém precisa
preparar o terreno, chamar `main` e, quando ele retornar, chamar `exit`. Isso
é o que a macro `entry!(main)` gera **dentro do programa**: uma função
`_start` que chama `main`, pega o `i32` que ele devolve e o passa a `exit`.
Como a macro chama `main` com a assinatura escrita, o compilador confere que
`main` é `fn() -> i32`; um `main` com outro tipo não compila. Um programa
nunca retorna de `_start`: não há para onde voltar, então o último passo é
sempre `exit` (é a regra do contrato, seção 3 do `SYSCALLS.md`).

### `print!` e o que cada `println!` faz

`print!` escreve com `core::fmt::Write` sobre a syscall `write`. O
`format_args!` produz o texto em vários trechos (`println!("a {}", 1)` gera
`a `, `1` e `\n`); a biblioteca os junta num buffer de 256 bytes na pilha do
programa e entrega tudo ao kernel numa **única** chamada `write`. No Marco 6
cada trecho era uma chamada, e isso era simples e bastava; com vários
programas ao mesmo tempo (Marco 7), três chamadas por linha deixavam uma troca
de tarefa partir a linha no meio (`ping ` de um programa, `pong 1` de outro).
Uma chamada por linha resolve: cada `write` chega inteiro à tela.

### `read_line`: esperar uma tecla dentro de uma syscall

`read_line(&mut buffer)` faz a syscall `SYS_READ_LINE`, que **bloqueia** o
programa até a pessoa apertar Enter. Enquanto espera, o kernel mostra na
tela o que é digitado e apaga com Backspace (o `write` não sabe apagar, então
essa edição tem de ser do kernel). A parte curiosa é como o kernel espera.

Como o kernel espera mudou no Marco 7. No Marco 6, a espera acontecia
**dentro** da syscall: a instrução `syscall` entra com as interrupções
desligadas (é o `SFMASK`), então o kernel executava `sti; hlt` até a IRQ1
trazer uma tecla, a única exceção à regra "syscall roda com interrupções
desligadas". Isso só funciona com um programa de cada vez: enquanto um
programa dorme dentro da syscall, nenhum outro roda. Agora a tarefa é
**bloqueada** e o escalonador passa a CPU a outra (ver "O teclado com vários
programas", no capítulo do Marco 7), e nenhuma syscall liga as interrupções.
`keyboard::read_line` continua existindo como a leitura bloqueante de um
leitor só (os testes a usam); o que ela e o escalonador compartilham é o
montador de linha, `LineEditor`.

De quem é o teclado enquanto o programa roda? Do programa. O laço que lê o
teclado e alimenta o prompt (`shell::poll_keyboard`) está **parado dentro** de
`run`, esperando o último programa terminar; quem lê a fila é o escalonador,
a pedido de um programa. Quando o último termina, o laço continua, e o que
sobrou na fila (digitação antecipada) vai para o prompt, sem perder nada.

### O heap que só cresce: `SYS_ALLOC`

`Box` e `Vec` precisam de um alocador, e o alocador precisa de memória. O
kernel tem um heap fixo, mapeado no boot; um programa começa **sem heap**.
`SYS_ALLOC(size)` amplia o heap do programa: arredonda para páginas de
4 KiB, mapeia páginas novas (zeradas, graváveis, nunca executáveis) logo
depois das que ele já tem e devolve o endereço do começo da área nova.
É o `brk` do Unix. O heap vive numa janela fixa, `[0x6000_0000,
0x6010_0000)`, então cabe no máximo 1 MiB; passar disso devolve `ERR_NOMEM`
em vez de travar ninguém, e a chamada é tudo ou nada. A página logo depois
do fim da janela nunca é mapeada: escrever além do fim do heap é um
`#PF`, não uma corrupção silenciosa.

O alocador da biblioteca (`heap.rs`) é o mesmo `linked_list_allocator` do
kernel: só muda de onde vem a memória. Quando a alocação falha, ele pede mais
páginas com `SYS_ALLOC` (no mínimo 16 KiB, para não fazer uma syscall a cada
`Box` pequeno) e **estende** o mesmo bloco, porque o contrato garante que
cada área nova começa onde a anterior terminou. No fim do programa
(`exit`, falha ou syscall inválida) o heap volta ao alocador de frames junto
com o código e a pilha: rodar um programa de 1 MiB 150 vezes, mais que a
memória do QEMU, não esgota nada, e um teste prova isso.

### Por que uma falha de programa não derruba o kernel: `falha_memoria`

O `falha_memoria` escreve no endereço `0xdead_beef`, que não é dele. A CPU
levanta um `#PF` em ring 3. O handler de `#PF` olha de qual anel a exceção
veio (`came_from_user`, o seletor de código que a CPU empilhou): em ring 3,
guarda o motivo, encerra só o programa e devolve o controle ao prompt, que
mostra `encerrado por erro de memoria` com o endereço de falha
(`0xdeadbeef`) e o código de erro (`0x6`: veio de ring 3, era uma escrita, a
página não existia). Em ring 0 o mesmo `#PF` continua fatal, com a tela de
exceção do Marco 4. É a mesma regra do Marco 5, agora com um programa feito
para mostrá-la: o kernel continua de pé, e o programa seguinte (`run hello`,
`run eco`) roda normalmente.

### Um bug que só apareceu com a espera de tecla

Ao rodar `run eco` de verdade no QEMU, a tela enchia de lixo assim que a
primeira tecla chegava, embora todos os testes automatizados passassem. O
que estava errado era a pilha de entrada do kernel: um `static` de `[u8; N]`
tem alinhamento 1, então o endereço da pilha (e do topo, que é o que o
processador e o stub de `syscall` usam) podia cair em qualquer byte; o topo
estava em `0x...f3`. O stub de `syscall` supõe um topo múltiplo de 16 (é o que
a convenção de chamada exige), e nada tinha aparecido antes porque nenhuma
syscall ficava esperando com interrupções chegando em cima dessa pilha.
Declarar as duas pilhas com `#[repr(align(16))]` resolveu, e há um teste que
confere o alinhamento. Os testes automatizados não pegam tudo: o QEMU
com teclado de verdade continua sendo parte da validação.

## Marco 7: multitarefa

Até o Marco 6, o kernel rodava um programa de usuário por vez: `run` ocupava o
prompt até o programa terminar. O Marco 7 carrega **vários** ao mesmo tempo e
alterna a CPU entre eles, primeiro quando o programa pede (a troca
**cooperativa**, a syscall `SYS_YIELD`) e depois quando o timer manda (a troca
**preemptiva**). A ideia central cabe numa frase: toda troca acontece quando um
programa entra no kernel, e nesse momento o kernel não está no meio de nada.

### O que é um contexto de execução

Para tirar um programa da CPU e devolvê-la depois "exatamente de onde parou",
o kernel precisa guardar tudo que define o estado dele em ring 3: os 15
registradores de uso geral, o ponteiro de instrução (`rip`), a pilha (`rsp`),
as flags (`rflags`) e os seletores de segmento (`cs`, `ss`). Isso é um
`TaskContext` (`src/task.rs`): 20 palavras de 64 bits, numa ordem escolhida de
propósito. Os cinco últimos campos (`rip, cs, rflags, rsp, ss`) são
**exatamente** o frame que a instrução `iretq` consome; os 15 primeiros
(`r15` no menor endereço, `rax` no maior) são os registradores, na ordem em que
o assembly os empilha, `rax` primeiro e `r15` por último.

Isso faz do contexto um bloco que pode ser montado em dois lugares sem cópia
alguma. O stub de `syscall` (`syscall.rs`) empilha os cinco campos do frame e
depois os 15 registradores, e quando a CPU entra por uma interrupção do timer
ela mesma empilha o frame, e o stub (`timer.rs`) empilha os 15. Nos dois casos a
pilha de entrada do kernel passa a conter um `TaskContext`, e o código em Rust
recebe um `&mut TaskContext` apontando direto para ela. Retomar um programa é o
caminho inverso, em `resume_task` (`user.rs`): apontar `rsp` para um
`TaskContext`, desempilhar os 15 registradores e executar `iretq`, que carrega
`rip, cs, rflags, rsp, ss` de uma vez e entra em ring 3. A primeira entrada de
um programa usa o mesmo caminho: o contexto inicial (`TaskContext::initial`)
tem `rip` na entrada do ELF, `rsp` no topo da pilha, `IF` ligado, e `rcx` e
`r11` com o `rip` e as flags iniciais (era um efeito de `sysretq`; agora é o
kernel que os coloca).

### Por que não há pilha de kernel por tarefa

O desenho clássico dá a cada tarefa uma pilha de kernel e uma rotina
`switch_to` que troca de pilha no meio de código Rust. Aqui não precisa: toda
troca acontece numa fronteira ring 3 → ring 0 (uma syscall, uma exceção ou o
timer), quando a pilha de entrada do kernel acabou de ser carregada e **está
vazia**, e o estado do programa está inteiro no topo dela. O kernel é
"corre até o fim" a cada entrada: não existe nada dele para guardar entre duas
trocas. Trocar é copiar esse `TaskContext` para a `Task` que sai e carregar o
de outra. É a razão de o escalonador inteiro caber em ~270 linhas.

Uma consequência: a volta de uma syscall passou de `sysretq` para `iretq`. O
`sysretq` só devolve `rip` e `rflags` por `rcx` e `r11`, e não sabe restaurar
um programa preemptado por inteiro; o `iretq` sabe. O contrato continua
dizendo que `rcx` e `r11` são destruídos por uma syscall, mas o kernel
agora os devolve intactos.

### A troca cooperativa: `SYS_YIELD`, passo a passo

1. O programa executa `syscall` com `rax = 5`. O stub (`syscall_entry`) troca
   para a pilha de entrada do kernel e monta o `TaskContext`.
2. `syscall_dispatch` recebe o contexto e chama `scheduler::yield_now`, que
   grava `0` em `rax` (o resultado de `yield`).
3. Se não há outra tarefa pronta, `yield_now` simplesmente retorna: o stub
   desempilha o contexto e volta ao mesmo programa. Se há, copia o contexto para
   a `Task` atual, marca-a `Ready` e chama `run_next`.
4. `run_next` é o ponto único por onde toda tarefa começa ou volta a rodar. Ele
   escolhe o **primeiro `Ready` depois do que rodou por último**, em ordem
   circular (um rodízio: `ping`, `pong`, `ping`, ...), ativa o espaço de
   endereçamento dela (`CR3`) e chama `resume_task`.
5. `resume_task` desempilha o contexto da outra tarefa e executa `iretq`: o
   outro programa continua na instrução seguinte ao `syscall` dele.

O primeiro `run_next` de uma execução não vem de uma syscall: `enter_user`
guarda o estado do kernel (como `setjmp`), desliga as interrupções e salta para
o escalonador; quando a última tarefa termina, `leave_user` restaura esse estado
e "retorna" de `enter_user`, como se ele tivesse terminado normalmente.

### Um espaço de endereçamento por programa

Os programas são todos ligados em `0x4000_0000`. Se dividissem uma tabela de
páginas, qualquer um leria a memória do outro (e dois não poderiam nem
coexistir no mesmo endereço). Por isso cada tarefa ganha a **própria tabela
P4** (`memory::AddressSpace`).

Criar um espaço é alocar dois frames, um para a P4 e outro para uma P3, e
copiar para eles as tabelas do kernel. As entradas da P4 copiada apontam para as
**mesmas** tabelas de nível mais baixo, então o kernel, a janela de memória
física e a pilha de entrada continuam visíveis com qualquer `CR3`; só a entrada
da região do usuário aponta para a P3 própria, que é cópia da do kernel com a
entrada da região do usuário **vazia**. As páginas do programa são mapeadas
depois, enquanto o espaço está ativo, e `map_to` cria as tabelas P2 e P1, que só
ele usa. Trocar de tarefa é escrever o `CR3` (o que também descarta o TLB).

Isso resolveu um vazamento que existia por construção: `Mapper::unmap` não
libera tabelas de página. Destruir um espaço (`AddressSpace::destroy`)
**percorre** a região do usuário (`P3[1] → P2 → P1`) e devolve ao alocador cada
frame de página e cada frame de tabela, mais a P3 e a P4 próprias. Nada do que
é do kernel é tocado. Um teste roda dois programas 100 vezes e confere que a
conta de frames em uso volta ao mesmo valor.

### O timer: PIT e IRQ0 pelo PIC

O PIT (Programmable Interval Timer) conta a partir de um divisor e gera uma
interrupção a cada volta da contagem. O oscilador dele roda a 1.193.182 Hz;
com o divisor `11932` (a divisão arredondada, `1.193.182 / 100`), saem 100
interrupções por segundo, um *tick* a cada 10 ms. `timer::init` escreve o
comando `0x36` (canal 0, modo 3) na porta `0x43` e o divisor, byte baixo e
byte alto, na porta `0x40`. A interrupção chega pela linha **IRQ0** do PIC 8259,
o vetor 32; a máscara do PIC, que só deixava passar a IRQ1 (teclado), passa a
liberar também a IRQ0.

Todo handler de interrupção de hardware precisa terminar com o **fim de
interrupção** (EOI, escrever `0x20` na porta `0x20` do PIC mestre); sem isso o
PIC nunca entrega o tick seguinte. O stub do timer envia o EOI **antes** de
qualquer troca de tarefa, em todos os caminhos: a CPU não volta ao ponto onde o
handler estava se o escalonador trocar de tarefa, e um EOI deixado para depois
silenciaria a IRQ0 para sempre. Um teste confere que, depois de muitas trocas,
o contador de ticks continua avançando.

### Por que e onde a troca preemptiva é segura

O kernel **nunca** é trocado. O stub do timer (`timer_entry`) olha o seletor de
código que a CPU empilhou, e o RPL dele diz de onde veio a interrupção:

- de **ring 0** (o kernel estava rodando: o laço ocioso, o prompt, um teste):
  conta o tick, envia o EOI e volta com `iretq`. Não troca de tarefa, não aloca,
  não pega nenhum lock;
- de **ring 3** (um programa estava rodando): empilha os 15 registradores
  (nasce um `TaskContext`), envia o EOI e chama o escalonador, que decide se
  passa a CPU a outro programa.

Isso responde à pergunta "em que ponto é seguro trocar de tarefa?". Todo o
resto do kernel (syscalls, exceções, a carga de programas, o alocador de frames,
a tela) roda com as interrupções **desligadas** (o `SFMASK` do `syscall` e as
portas de interrupção da IDT fazem isso), e portanto nunca é interrompido pelo
timer. O único trecho com `IF = 1` é a espera ociosa (`hlt`), que não segura
nenhum lock. Não há regiões críticas a marcar: o ponto seguro é a fronteira
ring 3 → ring 0, e ela é também o único lugar onde se troca. Um tick que chega
enquanto o kernel trata uma syscall fica **pendente** na CPU e é entregue assim
que o `iretq` devolve o programa a ring 3 com `IF = 1`.

Uma consequência dessa regra vale registrar: o código que roda depois de uma
entrada vinda de ring 3 (o escalonador, o término de uma tarefa) **pode** alocar
e liberar memória, porque nenhum código do kernel estava no meio de uma
operação, então nenhum lock está preso. Handlers disparados em ring 0 continuam
proibidos de alocar.

### A fatia de tempo, e o que aprendemos com ela

A primeira versão trocava de tarefa em todo tick vindo de ring 3 ("fatia de 1
tick"), e quebrou uma garantia simples: `ping` e `pong` deixaram de se alternar
sempre na mesma ordem (às vezes `pong 1` saía antes de `ping 1`). A causa é a
combinação de duas coisas: o kernel roda com interrupções desligadas, então um
tick que chega durante uma syscall fica pendente e é entregue no instante em
que o programa volta a rodar; e o PIC guarda **no máximo um** tick pendente. Um
tick caía entre a escrita e o `yield` de um programa, e ele perdia a vez.

A correção é contar a fatia. Ao colocar uma tarefa na CPU, o escalonador guarda
o valor do contador de ticks (`slice_start`); um tick vindo de ring 3 só tira a
CPU da tarefa se `ticks - slice_start >= SLICE_TICKS`, e `SLICE_TICKS` é **5**
(50 ms). Cada retorno de uma syscall pode somar um tick sem o programa ter
computado nada, mas um programa que escreve uma linha e cede a CPU faz duas ou
três syscalls por vez, menos que 5: nunca acumula uma fatia só com isso. Só é
interrompido quem calcula em ring 3 por 40 ms ou mais, que é exatamente o que
`contador_a` e `contador_b` fazem. Com 2 ticks o problema diminuiu mas não
sumiu: `run ping trio` ainda saiu fora de ordem em 2 de 3 execuções no QEMU; com
5, saiu em ordem em 6 de 6.

### O teclado com vários programas

`SYS_READ_LINE` valida o ponteiro como antes e, em vez de esperar dentro da
syscall, **bloqueia a tarefa** (`WaitingKeyboard`) com um número de ordem (o
`ticket`): o menor número é o que pediu primeiro. O escalonador passa a CPU a
outra tarefa.

Quem olha o teclado é `scheduler::poll_keyboard`, e só enquanto há alguém
esperando: o que ninguém pediu fica na fila, como no Marco 6. Ela monta a linha
com o `LineEditor` para a tarefa de menor `ticket` (eco e Backspace incluídos);
quando o Enter completa a linha, ela fica guardada na tarefa, que passa a
`Ready`, e as teclas seguintes valem para o próximo da espera. Uma tecla nunca
vai a dois programas, e uma linha pela metade continua sendo de quem pediu
primeiro, mesmo que outro programa peça depois. A **cópia** da linha para a
memória do programa só acontece quando ele volta a rodar (`deliver_line`), já
com o espaço de endereçamento dele ativo: o ponteiro foi validado antes de
bloquear, e nada desmapeia páginas de uma tarefa que espera.

`poll_keyboard` roda quando uma tarefa bloqueia (digitação antecipada), a cada
`yield`, a cada tick vindo de ring 3 e a cada volta do laço ocioso. O tick
importa: com um programa esperando o teclado e outro só calculando, a única
chance de o kernel olhar a fila é o tick; se `preempt` não chamasse
`poll_keyboard` **antes** de decidir se há outro programa pronto, a linha só
chegaria depois que o outro terminasse. Um teste cobre exatamente isso.

Quando nenhuma tarefa está pronta mas alguma espera, o kernel **dorme**: o laço
ocioso de `run_next` executa `sti; hlt` (como uma instrução atômica: uma tecla
ou um tick que chegue entre olhar e dormir acorda a CPU), e cada volta do laço
é causada por uma interrupção. Não há laço ocupado. Os testes provam isso
contando as voltas ociosas contra os ticks, e para "digitar" no meio da
execução usam um gancho que o escalonador chama a cada `poll_keyboard`.

### O ciclo de vida de uma tarefa

1. `run_images` carrega **todos** os programas pedidos (cada um num espaço novo)
   antes de iniciar qualquer um; se um falha, os já carregados são destruídos e
   nada roda. Mais de 4 programas, ou um nome que não existe, recusa o pedido
   inteiro.
2. As tarefas nascem `Ready`, na ordem em que foram pedidas.
3. Uma tarefa termina por `exit`, por uma exceção em ring 3 ou por uma syscall
   inexistente. Em todos os casos o caminho é `terminate_current`: volta ao
   espaço de endereçamento do kernel, registra o motivo, **avisa o shell** (que
   mostra a mensagem na hora, antes de os outros acabarem), destrói o espaço
   (todos os frames voltam ao alocador) e passa a CPU adiante.
4. Quando não resta nenhuma tarefa, `leave_user` devolve o controle ao prompt.

Por isso uma falha em um programa não derruba nem os outros: o kernel só
descarta a tarefa que falhou. Os testes montam os programas de apoio à mão
(ELFs de poucos bytes, com um mini-assembler no arquivo de teste), e um deles
confere que cada registrador, a pilha e o heap de dois programas que se
alternam voltam exatamente como estavam.

### O que ficou de fora

Prioridades, `sleep`, criar programas a partir de outros programas,
comunicação entre programas e sincronização (mutex, semáforo) continuam
fora do escopo. O kernel segue com um único processador e sem APIC.
