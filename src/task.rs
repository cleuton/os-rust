//! O que o kernel guarda de cada programa de usuário carregado: o estado dele
//! em ring 3 (`TaskContext`), em que ponto da vida ele está (`TaskState`),
//! um pedido de leitura de teclado em andamento (`PendingRead`) e a própria
//! tarefa (`Task`). Quem decide qual tarefa roda é o `scheduler`; este módulo
//! só define as estruturas.

use crate::gdt::{USER_CODE_SELECTOR_BITS, USER_DATA_SELECTOR_BITS};
use crate::keyboard::LINE_CAPACITY;
use crate::memory::AddressSpace;

/// O estado de uma tarefa em ring 3: tudo que é preciso para retomá-la
/// exatamente de onde parou. São 20 palavras de 64 bits, na ordem de endereços
/// crescentes em que o assembly as empilha (`syscall.rs` e `timer.rs`) e as
/// desempilha (`user.rs`): os 15 registradores gerais, com `r15` no menor
/// endereço, e depois cinco palavras que são **exatamente** o frame que a
/// instrução `iretq` consome (`rip`, `cs`, `rflags`, `rsp`, `ss`). Por isso
/// retomar uma tarefa é apontar `rsp` para este struct, desempilhar os 15
/// registradores e executar `iretq`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskContext {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

impl TaskContext {
    /// O contexto da primeira instrução de um programa (`SYSCALLS.md`, seção
    /// 3): `rip` na entrada do ELF, `rsp` no topo da pilha, `IF` ligado, os
    /// seletores de usuário, e todo o resto zero. `rcx` e `r11` não são zero:
    /// valem o `rip` e as flags iniciais, como o contrato sempre prometeu
    /// (era um efeito de `sysretq`; agora é o kernel que os coloca aqui).
    pub fn initial(entry: u64, user_rsp: u64) -> TaskContext {
        const USER_RFLAGS: u64 = 0x202;
        TaskContext {
            rip: entry,
            cs: USER_CODE_SELECTOR_BITS,
            rflags: USER_RFLAGS,
            rsp: user_rsp,
            ss: USER_DATA_SELECTOR_BITS,
            rcx: entry,
            r11: USER_RFLAGS,
            ..TaskContext::default()
        }
    }
}

/// Em que ponto da vida está uma tarefa. Uma tarefa que termina deixa de
/// existir (o slot dela na tabela do escalonador fica vazio), então não há
/// estado "terminada".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    /// Pode ser escolhida para rodar.
    Ready,
    /// É a tarefa em execução (no máximo uma).
    Running,
    /// Pediu uma linha ao teclado e espera ela ficar completa; não é escolhida.
    WaitingKeyboard,
}

/// Um `SYS_READ_LINE` em andamento: onde a linha deve ser entregue (já
/// validado), a ordem do pedido, e a linha pronta à espera de ser copiada para
/// a memória do programa quando ele voltar a rodar.
pub struct PendingRead {
    /// Endereço do buffer do programa.
    pub ptr: u64,
    /// Tamanho do buffer do programa.
    pub len: u64,
    /// Número de ordem do pedido: o menor é o que pediu primeiro.
    pub ticket: u64,
    /// A linha pronta, com o `\n` final.
    pub line: [u8; LINE_CAPACITY],
    /// Quantos bytes de `line` valem; `0` enquanto a linha não ficou pronta.
    pub written: usize,
}

/// Uma instância carregada de um programa de usuário.
pub struct Task {
    /// Nome do programa embutido, para as mensagens.
    pub name: &'static str,
    pub state: TaskState,
    /// O estado em ring 3 enquanto a tarefa não está rodando.
    pub context: TaskContext,
    /// O espaço de endereçamento dela; dono dos frames de tudo que ela mapeia.
    pub space: AddressSpace,
    /// Quantas páginas de heap ela já recebeu de `SYS_ALLOC`.
    pub heap_pages: u64,
    /// O pedido de teclado em andamento, se está em `WaitingKeyboard`.
    pub read: Option<PendingRead>,
    /// Quantas vezes foi colocada na CPU (os testes usam para provar que cada
    /// tarefa rodou em mais de uma fatia).
    pub slices: u32,
}

impl Task {
    /// Uma tarefa pronta para rodar a partir de `context`.
    pub fn new(name: &'static str, context: TaskContext, space: AddressSpace) -> Task {
        Task {
            name,
            state: TaskState::Ready,
            context,
            space,
            heap_pages: 0,
            read: None,
            slices: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test_case]
    fn o_layout_do_contexto_e_o_que_o_assembly_assume() {
        // 20 palavras: 15 registradores + o frame do `iretq`.
        assert_eq!(size_of::<TaskContext>(), 20 * 8);
        assert_eq!(offset_of!(TaskContext, r15), 0);
        assert_eq!(offset_of!(TaskContext, rax), 14 * 8);
        // O frame do `iretq` começa logo depois do `rax`, nesta ordem.
        assert_eq!(offset_of!(TaskContext, rip), 15 * 8);
        assert_eq!(offset_of!(TaskContext, cs), 16 * 8);
        assert_eq!(offset_of!(TaskContext, rflags), 17 * 8);
        assert_eq!(offset_of!(TaskContext, rsp), 18 * 8);
        assert_eq!(offset_of!(TaskContext, ss), 19 * 8);
    }

    #[test_case]
    fn o_contexto_inicial_segue_o_contrato() {
        let ctx = TaskContext::initial(0x4000_0010, 0x7FFF_FFF8);
        assert_eq!(ctx.rip, 0x4000_0010);
        assert_eq!(ctx.rsp, 0x7FFF_FFF8);
        assert_eq!(ctx.rflags, 0x202);
        assert_eq!(ctx.cs, 0x23);
        assert_eq!(ctx.ss, 0x1B);
        // `rcx` e `r11` valem o `rip` e as flags iniciais (SYSCALLS.md, seção 3).
        assert_eq!(ctx.rcx, 0x4000_0010);
        assert_eq!(ctx.r11, 0x202);
        // Todo o resto é zero: nada do kernel vaza para o programa.
        assert_eq!(ctx.rax | ctx.rbx | ctx.rdx | ctx.rsi | ctx.rdi | ctx.rbp, 0);
        assert_eq!(ctx.r8 | ctx.r9 | ctx.r10 | ctx.r12 | ctx.r13 | ctx.r14 | ctx.r15, 0);
    }
}
