//! O que o kernel guarda de cada programa de usuário carregado: o estado dele
//! em ring 3 (`TaskContext`), em que ponto da vida ele está (`TaskState`),
//! um pedido de leitura de teclado em andamento (`PendingRead`) e a própria
//! tarefa (`Task`). Quem decide qual tarefa roda é o `scheduler`; este módulo
//! só define as estruturas.

use core::fmt;
use core::ops::Deref;

use crate::fs::FileTable;
use crate::gdt::{USER_CODE_SELECTOR_BITS, USER_DATA_SELECTOR_BITS};
use crate::keyboard::LINE_CAPACITY;
use crate::memory::AddressSpace;
use crate::net::socket::{PendingNet, SocketTable};

/// Tamanho máximo do nome de um programa, em bytes. O maior nome embutido
/// (`falha_memoria`) tem 13; nomes de arquivo 8.3 têm no máximo 12.
pub const PROGRAM_NAME_MAX: usize = 16;

/// O nome de um programa nas mensagens do kernel. Um nome embutido vive para
/// sempre, mas o de um programa lido de um arquivo (`run /disco/bin/visita`)
/// nasce de um caminho digitado: guardá-lo num `&'static str` exigiria vazar
/// memória a cada `run`. Por isso o nome é copiado para um array de tamanho
/// fixo, que é `Copy` e não usa o heap (o registro de término também é escrito
/// dentro de handlers, onde o heap nunca é usado).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ProgramName {
    bytes: [u8; PROGRAM_NAME_MAX],
    len: u8,
}

impl ProgramName {
    /// Copia `name`; um nome com mais de `PROGRAM_NAME_MAX` bytes é cortado
    /// (nomes de programa são ASCII, então o corte nunca parte um caractere).
    pub fn new(name: &str) -> ProgramName {
        let mut bytes = [0u8; PROGRAM_NAME_MAX];
        let len = name.len().min(PROGRAM_NAME_MAX);
        bytes[..len].copy_from_slice(&name.as_bytes()[..len]);
        ProgramName { bytes, len: len as u8 }
    }
}

impl Deref for ProgramName {
    type Target = str;

    fn deref(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("?")
    }
}

impl fmt::Display for ProgramName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self)
    }
}

impl fmt::Debug for ProgramName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", &**self)
    }
}

impl PartialEq<&str> for ProgramName {
    fn eq(&self, other: &&str) -> bool {
        &**self == *other
    }
}

impl PartialEq<str> for ProgramName {
    fn eq(&self, other: &str) -> bool {
        &**self == other
    }
}

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
    /// Espera a rede (`accept`, `connect`, `send` com buffer cheio ou `recv`);
    /// não é escolhida até a pilha poder atender o pedido ou o prazo vencer.
    WaitingNet,
}

/// O que um programa espera do teclado: uma linha inteira (`SYS_READ_LINE`) ou
/// uma tecla só (`SYS_READ_KEY`). As duas usam a mesma espera e a mesma ordem
/// de pedidos; só muda como o escalonador usa cada scancode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PendingKind {
    /// `SYS_READ_LINE`: monta uma linha com eco até o Enter.
    Line,
    /// `SYS_READ_KEY`: entrega a próxima tecla pressionada, sem eco.
    Key,
}

/// Um pedido de teclado em andamento: o tipo, onde a linha deve ser entregue
/// (já validado, só para `Line`), a ordem do pedido, e o resultado à espera de
/// ser entregue quando o programa voltar a rodar (a linha pronta, ou o código
/// da tecla).
pub struct PendingRead {
    /// Linha ou tecla.
    pub kind: PendingKind,
    /// Endereço do buffer do programa (`Line`).
    pub ptr: u64,
    /// Tamanho do buffer do programa (`Line`).
    pub len: u64,
    /// Número de ordem do pedido: o menor é o que pediu primeiro.
    pub ticket: u64,
    /// A linha pronta, com o `\n` final.
    pub line: [u8; LINE_CAPACITY],
    /// Quantos bytes de `line` valem; `0` enquanto a linha não ficou pronta.
    pub written: usize,
    /// O código da tecla já recebida (`Key`); `0` enquanto nenhuma chegou (um
    /// código válido é sempre `>= 1`).
    pub key: u32,
}

/// O texto de argumentos de uma tarefa (o que `exec` passa e `SYS_ARGS`
/// entrega): até `MAX_ARGS_LEN` bytes num array fixo, sem usar o heap. Nasce
/// vazio e some junto com a tarefa, então nunca passa de um programa para outro.
#[derive(Clone, Copy)]
pub struct TaskArgs {
    bytes: [u8; abi::MAX_ARGS_LEN],
    len: u8,
}

impl TaskArgs {
    /// Sem argumentos.
    pub const fn new() -> TaskArgs {
        TaskArgs { bytes: [0; abi::MAX_ARGS_LEN], len: 0 }
    }

    /// Guarda `text` (no máximo `MAX_ARGS_LEN` bytes; o que passar é cortado,
    /// mas quem chama já recusou textos maiores).
    pub fn set(&mut self, text: &str) {
        let n = text.len().min(abi::MAX_ARGS_LEN);
        self.bytes[..n].copy_from_slice(&text.as_bytes()[..n]);
        self.len = n as u8;
    }

    /// Os bytes do texto.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

impl Default for TaskArgs {
    fn default() -> Self {
        TaskArgs::new()
    }
}

/// Uma instância carregada de um programa de usuário.
pub struct Task {
    /// Nome do programa embutido, para as mensagens.
    pub name: ProgramName,
    pub state: TaskState,
    /// O estado em ring 3 enquanto a tarefa não está rodando.
    pub context: TaskContext,
    /// O espaço de endereçamento dela; dono dos frames de tudo que ela mapeia.
    pub space: AddressSpace,
    /// Quantas páginas de heap ela já recebeu de `SYS_ALLOC`.
    pub heap_pages: u64,
    /// O pedido de teclado em andamento, se está em `WaitingKeyboard`.
    pub read: Option<PendingRead>,
    /// Os arquivos abertos desta tarefa; fechados quando ela é destruída.
    pub files: FileTable,
    /// Os sockets desta tarefa; fechados (como `SYS_TCP_CLOSE`) quando ela é
    /// destruída.
    pub sockets: SocketTable,
    /// O pedido de rede em andamento, se está em `WaitingNet`.
    pub pending_net: Option<PendingNet>,
    /// O texto de argumentos (vazio, salvo se veio de `exec`).
    pub args: TaskArgs,
    /// Quantas vezes foi colocada na CPU (os testes usam para provar que cada
    /// tarefa rodou em mais de uma fatia).
    pub slices: u32,
}

impl Task {
    /// Uma tarefa pronta para rodar a partir de `context`.
    pub fn new(name: ProgramName, context: TaskContext, space: AddressSpace) -> Task {
        Task {
            name,
            state: TaskState::Ready,
            context,
            space,
            heap_pages: 0,
            read: None,
            files: FileTable::new(),
            sockets: SocketTable::new(),
            pending_net: None,
            args: TaskArgs::new(),
            slices: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test_case]
    fn argumentos_comecam_vazios_e_guardam_o_texto() {
        let mut args = TaskArgs::new();
        assert_eq!(args.as_bytes(), b"");
        args.set("a   b");
        assert_eq!(args.as_bytes(), b"a   b");
        assert_eq!(args.as_bytes().len(), 5);
        args.set("");
        assert_eq!(args.as_bytes(), b"");
    }

    #[test_case]
    fn argumentos_cortam_no_maximo_do_contrato() {
        let longo = "x".repeat(abi::MAX_ARGS_LEN + 10);
        let mut args = TaskArgs::new();
        args.set(&longo);
        assert_eq!(args.as_bytes().len(), abi::MAX_ARGS_LEN);
    }

    #[test_case]
    fn nome_de_programa_guarda_o_texto_sem_cortar_nomes_embutidos() {
        let name = ProgramName::new("falha_memoria");
        assert_eq!(&*name, "falha_memoria");
        assert!(name == "falha_memoria");
        assert_eq!(alloc::format!("{}", name), "falha_memoria");
        assert_eq!(alloc::format!("{:?}", ProgramName::new("hello")), "\"hello\"");
    }

    #[test_case]
    fn nome_de_programa_muito_longo_e_cortado() {
        let name = ProgramName::new("abcdefghijklmnopqrstuvwxyz");
        assert_eq!(&*name, "abcdefghijklmnop");
        assert_eq!(name.len(), PROGRAM_NAME_MAX);
        assert_eq!(ProgramName::new("").len(), 0);
    }

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
