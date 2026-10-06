//! O timer: o PIT (Programmable Interval Timer) gera uma interrupção periódica
//! pela linha IRQ0 do PIC 8259, e é ela que tira a CPU de um programa que não
//! a cede (a preempção).
//!
//! O PIT é programado para `TIMER_HZ` interrupções por segundo (100, ou seja,
//! um *tick* a cada 10 ms). A fatia de cada programa é de `SLICE_TICKS` ticks
//! (5, ou seja, 50 ms), contados pelo escalonador desde que o programa foi
//! colocado na CPU.
//!
//! # Onde a troca de tarefa é segura
//!
//! O kernel **nunca** é trocado. O stub `timer_entry` olha o seletor de código
//! (`cs`) que a CPU empilhou e decide:
//!
//! - veio de **ring 0** (o kernel estava rodando: o laço ocioso, o prompt, um
//!   teste): conta o tick, avisa o PIC (EOI) e volta com `iretq`. Não troca de
//!   tarefa, não aloca, não pega nenhum lock;
//! - veio de **ring 3** (um programa estava rodando): salva o estado dele num
//!   `TaskContext`, avisa o PIC e deixa o escalonador decidir se passa a CPU a
//!   outro programa.
//!
//! Todo o resto do kernel (syscalls, exceções, carga de programas, alocador de
//! frames, tela) roda com as interrupções desligadas e, portanto, nunca é
//! interrompido pelo timer. O único trecho do kernel com `IF = 1` é a espera
//! ociosa (`hlt`), que não segura nenhum lock. Por isso não há regiões críticas
//! a marcar: o ponto seguro da preempção é a fronteira ring 3 → ring 0.

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

use abi::TIMER_HZ;
use x86_64::instructions::port::Port;

use crate::scheduler;
use crate::task::TaskContext;

/// Frequência do oscilador do PIT, em Hz.
const PIT_FREQUENCY: u64 = 1_193_182;

/// Divisor que o PIT conta a cada interrupção: a frequência do oscilador
/// dividida por `TIMER_HZ`, arredondada (a divisão inteira daria 11931, e o
/// valor mais próximo de 100 Hz é 11932).
const PIT_DIVISOR: u16 = ((PIT_FREQUENCY + TIMER_HZ / 2) / TIMER_HZ) as u16;

/// Porta de comando do PIT.
const PIT_COMMAND_PORT: u16 = 0x43;
/// Porta de dados do canal 0 do PIT (o que está ligado à IRQ0).
const PIT_CHANNEL0_PORT: u16 = 0x40;
/// Comando: canal 0, acesso ao byte baixo e depois ao alto, modo 3 (onda
/// quadrada, a mais usada para um tick periódico), contagem binária.
const PIT_COMMAND_CHANNEL0_SQUARE_WAVE: u8 = 0x36;

/// Porta de comando do PIC mestre, onde o fim de interrupção (EOI) é escrito.
const PIC_MASTER_COMMAND_PORT: u16 = 0x20;
/// O comando de fim de interrupção.
const PIC_EOI: u8 = 0x20;

/// Quantos ticks já ocorreram desde o boot (ring 0 e ring 3). Os testes o usam
/// para provar que o timer está vivo.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Quantos ticks já ocorreram desde o boot.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Programa o PIT para `TIMER_HZ` interrupções por segundo. Chamada uma única
/// vez, no boot, antes de as interrupções serem habilitadas globalmente (e com
/// a IRQ0 ainda mascarada no PIC; `interrupts::init` a libera).
pub fn init() {
    let mut command: Port<u8> = Port::new(PIT_COMMAND_PORT);
    let mut channel0: Port<u8> = Port::new(PIT_CHANNEL0_PORT);
    // SAFETY: 0x43 e 0x40 são as portas fixas de comando e de dados do PIT; o
    // comando escolhe o canal 0 em modo 3 e os dois bytes seguintes, escritos
    // na ordem baixo e alto, são o divisor. Nenhum outro código do kernel usa
    // estas portas, e nenhuma interrupção está habilitada ainda.
    unsafe {
        command.write(PIT_COMMAND_CHANNEL0_SQUARE_WAVE);
        channel0.write((PIT_DIVISOR & 0xFF) as u8);
        channel0.write((PIT_DIVISOR >> 8) as u8);
    }
}

/// Avisa o PIC mestre de que a interrupção da IRQ0 foi atendida (EOI). Sem
/// isto o PIC nunca entregaria o tick seguinte. Escreve direto na porta, sem
/// pegar o `Mutex` do `ChainedPics`: é seguro em qualquer contexto, até dentro
/// de uma interrupção que interrompeu código do kernel.
pub(crate) fn end_of_interrupt() {
    let mut port: Port<u8> = Port::new(PIC_MASTER_COMMAND_PORT);
    // SAFETY: 0x20 é a porta de comando do PIC mestre, e `0x20` é o comando de
    // fim de interrupção não específico; a IRQ0 é do PIC mestre.
    unsafe { port.write(PIC_EOI) };
}

extern "C" {
    /// O stub da IRQ0 (assembly, logo abaixo), registrado na IDT.
    pub(crate) fn timer_entry();
}

// O stub da IRQ0. A CPU já empilhou, no topo da pilha em uso, o frame de uma
// interrupção sem código de erro: `rip, cs, rflags` e, só se veio de ring 3,
// também `rsp, ss` (e antes disso trocou para a pilha de entrada do kernel,
// `TSS.rsp0`). `[rsp + 8]` é o `cs` empilhado, e o RPL dele (os 2 bits baixos)
// diz de onde a interrupção veio.
//
// Caminho de ring 0: só o que é seguro em qualquer lugar do kernel (sem
// alocar, sem lock): conta o tick, avisa o PIC e volta.
//
// Caminho de ring 3: empilha os 15 registradores gerais, na mesma ordem do stub
// de `syscall`, de modo que a pilha passa a conter um `TaskContext`. O
// escalonador decide: se não há outro programa pronto, `timer_from_user`
// retorna e este stub restaura tudo e volta ao mesmo programa; se há, ele
// guarda o contexto e passa a CPU adiante, e nunca retorna.
global_asm!(
    ".global timer_entry",
    "timer_entry:",
    "test qword ptr [rsp + 8], 3",
    "jnz 2f",
    // De ring 0.
    "push rax",
    "lock inc qword ptr [rip + {ticks}]",
    "mov al, {eoi}",
    "out {pic_port}, al",
    "pop rax",
    "iretq",
    // De ring 3.
    "2:",
    "push rax",
    "push rbx",
    "push rcx",
    "push rdx",
    "push rsi",
    "push rdi",
    "push rbp",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    // O CPU alinhou `rsp` em 16 antes de empilhar o frame (5 palavras) e agora
    // há mais 15: o total (20 palavras) mantém `rsp` alinhado em 16 no `call`.
    "mov rdi, rsp",
    "call {from_user}",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rbp",
    "pop rdi",
    "pop rsi",
    "pop rdx",
    "pop rcx",
    "pop rbx",
    "pop rax",
    "iretq",
    ticks = sym TICKS,
    eoi = const PIC_EOI,
    pic_port = const PIC_MASTER_COMMAND_PORT,
    from_user = sym timer_from_user,
);

/// O trabalho do tick que interrompeu um programa em ring 3. O fim de
/// interrupção sai **primeiro**, antes de qualquer troca: a CPU nunca volta
/// para este ponto se o escalonador trocar de tarefa, e um EOI que ficasse
/// para depois deixaria a IRQ0 muda para sempre.
extern "C" fn timer_from_user(ctx: &mut TaskContext) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    end_of_interrupt();
    scheduler::preempt(ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn o_divisor_do_pit_da_100_hz() {
        // 1_193_182 / 100 = 11931,82: o divisor arredondado é 11932.
        assert_eq!(PIT_DIVISOR, 11932);
        assert_eq!(TIMER_HZ, 100);
    }
}
