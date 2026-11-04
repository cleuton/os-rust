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

## O caminho de uma tecla: do teclado até a tela

A v1 só escrevia na tela. Esta versão responde à pergunta natural que vem
depois: "e dá para digitar?". Para isso, o `proto-os` precisa reagir a um
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

Até aqui, a única forma de "ver" o que o `proto-os` está fazendo era olhar
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
(`std`) — que não existe aqui: o `proto-os` é `#![no_std]` desde o Marco
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
`x86_64-proto_os.json`), não existe a possibilidade de "capturar" um
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
#![test_runner(proto_os::test_runner)]
#![reexport_test_harness_main = "test_main"]

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;

entry_point!(main);

fn main(boot_info: &'static BootInfo) -> ! {
    proto_os::init(boot_info);
    test_main();
    proto_os::panic::halt_loop();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    proto_os::test_panic_handler(info)
}

#[test_case]
fn meu_teste_de_integracao() {
    // ... o resto do cenário sob teste
}
```

Cada arquivo em `tests/` dá boot no kernel do zero, no seu próprio
processo QEMU — por isso `proto_os::init(boot_info)` precisa ser chamado
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
proto-os> mem
memoria fisica utilizavel: 123848 KiB
heap: 0x444444440000, 100 KiB
Box: valor=42, endereco=0x444444440000
Vec: tamanho=10, capacidade=16, soma=55
proto-os>
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

