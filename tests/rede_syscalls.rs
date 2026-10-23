//! Testes do Marco 12 das sete syscalls de rede (17 a 23), com programas ELF
//! montados à mão: cada erro na ordem do contrato, `ERR_FAULT` antes de qualquer
//! efeito, os limites de 4 sockets e de 8 conexões, o socket que é só do
//! programa, `accept` bloqueando e acordando, o tempo limite exato, o programa
//! bloqueado que não gasta CPU, e o término (por `exit` e por falha) fechando os
//! sockets. O envio com o buffer cheio é provado direto na camada de sockets do
//! kernel, sem ELF.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os_rust::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::vec::Vec;
use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use os_rust::net::{self, socket::SocketTable, Outcome};
use os_rust::syscall::{
    ERR_ADDRINUSE, ERR_BADF, ERR_CONNREFUSED, ERR_FAULT, ERR_INVAL, ERR_MFILE, ERR_NOBUFS,
    ERR_NODEV, ERR_TIMEDOUT, ERR_TYPE, SYS_TCP_ACCEPT, SYS_TCP_CLOSE, SYS_TCP_CONNECT,
    SYS_TCP_LISTEN, SYS_TCP_RECV, SYS_TCP_SEND, SYS_TCP_TIMEOUT, SYS_YIELD,
};
use os_rust::user::{self, Termination, USER_REGION_START, USER_STACK_BOTTOM};
use os_rust::{memory, timer};

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

// ---------------------------------------------------------------------------
// Programas de apoio montados à mão
// ---------------------------------------------------------------------------

/// Monta um ELF64 `ET_EXEC` x86-64 com um único segmento `PT_LOAD` `R E` em
/// `USER_REGION_START`, contendo `code`, com a entrada no primeiro byte. Campos
/// usados (ver `src/elf.rs`): cabeçalho de 64 bytes, um *program header* de 56
/// bytes logo depois, e o código em seguida. (O mesmo construtor de
/// `tests/multitarefa.rs`.)
fn synth_elf(code: &[u8]) -> Vec<u8> {
    let offset = 64 + 56;
    let mut file = alloc::vec![0u8; offset];
    file[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    file[4] = 2; // ELF64
    file[5] = 1; // little-endian
    file[6] = 1; // versão
    file[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    file[18..20].copy_from_slice(&62u16.to_le_bytes()); // EM_X86_64
    file[24..32].copy_from_slice(&USER_REGION_START.to_le_bytes()); // e_entry
    file[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
    file[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
    file[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
    file.extend_from_slice(code);
    let h = 64;
    file[h..h + 4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    file[h + 4..h + 8].copy_from_slice(&5u32.to_le_bytes()); // R + X
    file[h + 8..h + 16].copy_from_slice(&(offset as u64).to_le_bytes()); // p_offset
    file[h + 16..h + 24].copy_from_slice(&USER_REGION_START.to_le_bytes()); // p_vaddr
    file[h + 32..h + 40].copy_from_slice(&(code.len() as u64).to_le_bytes()); // p_filesz
    file[h + 40..h + 48].copy_from_slice(&(code.len() as u64).to_le_bytes()); // p_memsz
    file
}

/// Os registradores de uso geral que o mini-assembler usa, com o número que o
/// código de máquina usa.
#[derive(Clone, Copy)]
enum Reg {
    Rax = 0,
    Rcx = 1,
    Rdx = 2,
    Rbx = 3,
    Rsi = 6,
    Rdi = 7,
}

use Reg::*;

/// Um mini-assembler: só as instruções que os programas de apoio usam.
struct Asm(Vec<u8>);

impl Asm {
    fn new() -> Asm {
        Asm(Vec::new())
    }

    /// `mov r32, imm32` (zera a metade alta do registrador de 64 bits).
    fn mov32(&mut self, reg: Reg, imm: u32) {
        self.0.push(0xB8 + reg as u8);
        self.0.extend_from_slice(&imm.to_le_bytes());
    }

    /// `movabs r64, imm64`.
    fn movabs(&mut self, reg: Reg, imm: u64) {
        self.0.push(0x48); // REX.W
        self.0.push(0xB8 + reg as u8);
        self.0.extend_from_slice(&imm.to_le_bytes());
    }

    /// `mov dst, src` (64 bits, registrador para registrador).
    fn mov_rr(&mut self, dst: Reg, src: Reg) {
        self.0.push(0x48);
        self.0.push(0x89);
        self.0.push(0xC0 | ((src as u8) << 3) | dst as u8);
    }

    /// `syscall`.
    fn syscall(&mut self) {
        self.0.extend_from_slice(&[0x0F, 0x05]);
    }

    /// `cmp a, b` (calcula `a - b` e liga as flags).
    fn cmp(&mut self, a: Reg, b: Reg) {
        self.0.push(0x48);
        self.0.push(0x39);
        self.0.push(0xC0 | ((b as u8) << 3) | a as u8);
    }

    /// `mov [base], src` (64 bits; `base` não pode ser `rsp`, `rbp`, `r12`, `r13`).
    fn store(&mut self, base: Reg, src: Reg) {
        self.0.push(0x48);
        self.0.push(0x89);
        self.0.push(((src as u8) << 3) | base as u8);
    }

    /// `exit(code)`: `mov edi, code; mov eax, 2; syscall`.
    fn exit(&mut self, code: u32) {
        self.mov32(Rdi, code);
        self.mov32(Rax, 2);
        self.syscall();
    }

    /// Tamanho em bytes de `exit`: 5 + 5 + 2.
    const EXIT_LEN: u8 = 12;

    /// Chama a syscall `nr` com três argumentos e exige que o resultado seja
    /// `esperado`; senão o programa sai com `code`. O resultado vai para `rbx`
    /// (que a syscall preserva) e é comparado em `rax`.
    fn chamada(&mut self, nr: u64, args: [u64; 3], esperado: i64, code: u32) {
        self.movabs(Rax, nr);
        self.movabs(Rdi, args[0]);
        self.movabs(Rsi, args[1]);
        self.movabs(Rdx, args[2]);
        self.syscall();
        self.mov_rr(Rbx, Rax);
        self.movabs(Rax, esperado as u64);
        self.cmp(Rbx, Rax);
        self.0.extend_from_slice(&[0x74, Asm::EXIT_LEN]); // je +12: igual, pula o exit
        self.exit(code);
    }

    /// `jmp rel32` sobre `len` bytes de dados.
    fn pular_dados(&mut self, len: usize) {
        self.0.push(0xE9);
        self.0.extend_from_slice(&(len as u32).to_le_bytes());
    }
}

/// Monta um programa que carrega `dados` no começo do segmento (depois de um
/// `jmp` que os pula) e roda o código que `escreve` gera. `escreve` recebe o
/// endereço, no programa, de cada pedaço de `dados`, para usá-los como
/// ponteiros. O segmento é `R E`: os dados são legíveis e **não** graváveis.
fn programa(dados: &[&[u8]], escreve: impl FnOnce(&mut Asm, &[u64])) -> Vec<u8> {
    let total: usize = dados.iter().map(|d| d.len()).sum();
    let mut enderecos = Vec::new();
    let mut corrente = USER_REGION_START + 5; // depois do `jmp rel32`
    for d in dados {
        enderecos.push(corrente);
        corrente += d.len() as u64;
    }
    let mut a = Asm::new();
    a.pular_dados(total);
    for d in dados {
        a.0.extend_from_slice(d);
    }
    escreve(&mut a, &enderecos);
    a.exit(0);
    synth_elf(&a.0)
}



/// `127.0.0.1` como o kernel o recebe.
const LOCALHOST: u64 = 0x7F00_0001;
/// Um buffer gravável do programa (a pilha dele).
const BUF: u64 = USER_STACK_BOTTOM + 0x100;
/// Um endereço fora da região do usuário.
const RUIM: u64 = 0x10;

impl Asm {
    /// `loop: dec rcx; jnz loop` depois de `mov rcx, n`: computa sem syscall.
    fn computar(&mut self, n: u64) {
        self.movabs(Rcx, n);
        self.0.extend_from_slice(&[0x48, 0xFF, 0xC9, 0x75, 0xFB]);
    }
}

fn nr(n: u64) -> u64 {
    n
}

/// Roda os programas juntos e devolve o relatório e quantos ticks passaram.
fn rodar(programas: &[(&str, &[u8])]) -> (user::RunReport, u64) {
    let antes = timer::ticks();
    let relatorio = user::run_images(programas, None).expect("o pedido de run foi recusado");
    (relatorio, timer::ticks() - antes)
}

fn saiu_bem(relatorio: &user::RunReport, nome: &str) {
    let fim = relatorio
        .finished
        .iter()
        .find(|f| f.name == nome)
        .unwrap_or_else(|| panic!("{} nao terminou: {:?}", nome, relatorio.finished));
    assert_eq!(fim.termination, Termination::Exit { code: 0 }, "programa {}", nome);
}

/// Espera (andando a pilha) até a tabela de conexões esvaziar, no máximo
/// `limite` ticks. Devolve quantas ficaram.
fn esperar_pilha_vazia(limite: u64) -> usize {
    let fim = timer::ticks() + limite;
    loop {
        x86_64::instructions::interrupts::disable();
        net::poll();
        let em_uso = net::tcp_in_use();
        if em_uso == 0 || timer::ticks() >= fim {
            x86_64::instructions::interrupts::enable();
            return em_uso;
        }
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

// ---------------------------------------------------------------------------
// Erros na ordem do contrato
// ---------------------------------------------------------------------------

#[test_case]
fn cada_erro_de_cada_syscall_na_ordem_do_contrato() {
    let frames = memory::frames_outstanding();
    let elf = programa(&[b"hello"], |a, en| {
        let ola = en[0];
        a.chamada(SYS_TCP_LISTEN, [0, 0, 0], ERR_INVAL, 1);
        a.chamada(SYS_TCP_LISTEN, [65536, 0, 0], ERR_INVAL, 2);
        a.chamada(SYS_TCP_LISTEN, [8000, 0, 0], 0, 3);
        a.chamada(SYS_TCP_LISTEN, [8000, 0, 0], ERR_ADDRINUSE, 4);
        a.chamada(SYS_TCP_CONNECT, [0, 80, 0], ERR_INVAL, 5);
        a.chamada(SYS_TCP_CONNECT, [LOCALHOST, 0, 0], ERR_INVAL, 6);
        a.chamada(SYS_TCP_CONNECT, [1 << 32, 80, 0], ERR_INVAL, 7);
        a.chamada(SYS_TCP_CONNECT, [LOCALHOST, 8000, 0], 1, 8);
        a.chamada(SYS_TCP_ACCEPT, [0, 0, 0], 2, 9);
        a.chamada(SYS_TCP_ACCEPT, [1, 0, 0], ERR_TYPE, 10);
        a.chamada(SYS_TCP_ACCEPT, [9, 0, 0], ERR_BADF, 11);
        a.chamada(SYS_TCP_SEND, [1, ola, 5], 5, 12);
        a.chamada(SYS_TCP_RECV, [2, BUF, 16], 5, 13);
        a.chamada(SYS_TCP_SEND, [0, ola, 5], ERR_TYPE, 14);
        a.chamada(SYS_TCP_RECV, [0, BUF, 16], ERR_TYPE, 15);
        a.chamada(SYS_TCP_SEND, [9, ola, 5], ERR_BADF, 16);
        a.chamada(SYS_TCP_SEND, [1, RUIM, 5], ERR_FAULT, 17);
        a.chamada(SYS_TCP_SEND, [9, RUIM, 5], ERR_FAULT, 18);
        a.chamada(SYS_TCP_SEND, [1, ola, 5000], ERR_INVAL, 19);
        a.chamada(SYS_TCP_RECV, [2, ola, 5], ERR_FAULT, 20);
        a.chamada(SYS_TCP_SEND, [1, RUIM, 0], 0, 21);
        a.chamada(SYS_TCP_SEND, [9, RUIM, 0], ERR_BADF, 22);
        a.chamada(SYS_TCP_TIMEOUT, [2, 60001, 0], ERR_INVAL, 23);
        a.chamada(SYS_TCP_TIMEOUT, [9, 10, 0], ERR_BADF, 24);
        a.chamada(SYS_TCP_TIMEOUT, [2, 0, 0], 0, 25);
        a.chamada(SYS_TCP_RECV, [2, BUF, 16], ERR_TIMEDOUT, 26);
        a.chamada(SYS_TCP_RECV, [2, RUIM, 0], 0, 27);
        a.chamada(SYS_TCP_CLOSE, [1, 0, 0], 0, 28);
        a.chamada(SYS_TCP_RECV, [2, BUF, 16], 0, 29);
        a.chamada(SYS_TCP_SEND, [2, ola, 5], 5, 30);
        a.chamada(SYS_TCP_CLOSE, [1, 0, 0], ERR_BADF, 31);
        a.chamada(SYS_TCP_LISTEN, [8001, 0, 0], 1, 32);
        a.chamada(SYS_TCP_LISTEN, [8002, 0, 0], 3, 33);
        a.chamada(SYS_TCP_LISTEN, [8003, 0, 0], ERR_MFILE, 34);
        // Sai com os sockets abertos de propósito: o término os fecha.
    });
    let (relatorio, _) = rodar(&[("erros", &elf)]);
    assert_eq!(relatorio.finished[0].termination, Termination::Exit { code: 0 }, "{:?}", relatorio.finished);
    assert_eq!(esperar_pilha_vazia(400), 0);
    assert_eq!(memory::frames_outstanding(), frames);
}

#[test_case]
fn connect_sem_rede_para_fora_do_loopback_e_nodev_e_com_loopback_e_recusado() {
    net::set_card_enabled(false);
    let elf = programa(&[], |a, _| {
        a.chamada(SYS_TCP_CONNECT, [0x0A00_0202, 80, 0], ERR_NODEV, 1);
        a.chamada(SYS_TCP_CONNECT, [LOCALHOST, 9, 0], ERR_CONNREFUSED, 2);
        // `listen` não depende de placa.
        a.chamada(SYS_TCP_LISTEN, [8000, 0, 0], 0, 3);
    });
    let (relatorio, _) = rodar(&[("sem_placa", &elf)]);
    net::set_card_enabled(true);
    saiu_bem(&relatorio, "sem_placa");
    assert_eq!(esperar_pilha_vazia(400), 0);
}

#[test_case]
fn numero_24_continua_encerrando_so_o_programa() {
    let elf = programa(&[], |a, _| {
        a.movabs(Rax, nr(24));
        a.syscall();
    });
    let (relatorio, _) = rodar(&[("n24", &elf)]);
    assert_eq!(relatorio.finished[0].termination, Termination::BadSyscall { number: 24 });
}

// ---------------------------------------------------------------------------
// Bloqueio
// ---------------------------------------------------------------------------

#[test_case]
fn accept_bloqueia_e_acorda_quando_o_cliente_conecta() {
    let frames = memory::frames_outstanding();
    let servidor = programa(&[], |a, _| {
        a.chamada(SYS_TCP_LISTEN, [8000, 0, 0], 0, 1);
        a.chamada(SYS_TCP_ACCEPT, [0, 0, 0], 1, 2); // bloqueia até o cliente chegar
        a.chamada(SYS_TCP_RECV, [1, BUF, 16], 3, 3);
        a.chamada(SYS_TCP_CLOSE, [1, 0, 0], 0, 4);
    });
    let cliente = programa(&[b"abc"], |a, en| {
        a.chamada(SYS_TCP_CONNECT, [LOCALHOST, 8000, 0], 0, 1);
        a.chamada(SYS_TCP_SEND, [0, en[0], 3], 3, 2);
    });
    let (relatorio, _) = rodar(&[("srv", &servidor), ("cli", &cliente)]);
    saiu_bem(&relatorio, "srv");
    saiu_bem(&relatorio, "cli");
    assert_eq!(esperar_pilha_vazia(400), 0);
    assert_eq!(memory::frames_outstanding(), frames);
}

/// Um programa que conecta nele mesmo e depois espera dados por `ms`
/// milissegundos: o `recv` tem de terminar com `ERR_TIMEDOUT`.
fn espera_com_tempo_limite(ms: u64) -> Vec<u8> {
    programa(&[], |a, _| {
        a.chamada(SYS_TCP_LISTEN, [8000, 0, 0], 0, 1);
        a.chamada(SYS_TCP_CONNECT, [LOCALHOST, 8000, 0], 1, 2);
        a.chamada(SYS_TCP_ACCEPT, [0, 0, 0], 2, 3);
        a.chamada(SYS_TCP_TIMEOUT, [2, ms, 0], 0, 4);
        a.chamada(SYS_TCP_RECV, [2, BUF, 16], ERR_TIMEDOUT, 5);
    })
}

#[test_case]
fn recv_respeita_o_tempo_limite_arredondado_ao_tick_e_o_kernel_dorme() {
    // 100 ms = 10 ticks exatos; 25 ms arredonda para cima, 3 ticks.
    for (ms, minimo, maximo) in [(100u64, 10u64, 14u64), (25, 3, 7), (1, 1, 5)] {
        let elf = espera_com_tempo_limite(ms);
        let (relatorio, ticks) = rodar(&[("espera", &elf)]);
        saiu_bem(&relatorio, "espera");
        assert!(
            (minimo..=maximo).contains(&ticks),
            "{} ms: esperou {} ticks (esperado {}..={})",
            ms,
            ticks,
            minimo,
            maximo
        );
        // Dormiu esperando a rede: houve voltas ociosas, e nao mais que ticks.
        assert!(relatorio.idle_loops >= 1, "{} ms: o kernel nao dormiu", ms);
        assert!(u64::from(relatorio.idle_loops) <= ticks + 3, "{} ms: laco ocupado", ms);
    }
    assert_eq!(esperar_pilha_vazia(400), 0);
}

#[test_case]
fn programa_bloqueado_na_rede_nao_atrasa_o_que_computa() {
    // `x` espera 500 ms por dados que nunca vêm; `y` só computa e termina.
    let x = espera_com_tempo_limite(500);
    let y = programa(&[], |a, _| a.computar(5_000_000));
    let (relatorio, ticks) = rodar(&[("x", &x), ("y", &y)]);
    saiu_bem(&relatorio, "x");
    saiu_bem(&relatorio, "y");
    // `y` termina antes de `x` acordar (o prazo de `x` é de 50 ticks).
    assert_eq!(relatorio.finished[0].name, "y", "{:?}", relatorio.finished);
    assert!(ticks >= 50, "x acordou cedo demais: {} ticks", ticks);
    // Depois de `y` sair, só `x` espera: o kernel dormiu.
    assert!(relatorio.idle_loops >= 1);
    assert_eq!(esperar_pilha_vazia(400), 0);
}

// ---------------------------------------------------------------------------
// Isolamento e limites
// ---------------------------------------------------------------------------

#[test_case]
fn socket_de_outro_programa_nao_existe() {
    let a = programa(&[], |a, _| {
        a.chamada(SYS_TCP_LISTEN, [8000, 0, 0], 0, 1);
        a.chamada(SYS_YIELD, [0, 0, 0], 0, 2);
        a.chamada(SYS_YIELD, [0, 0, 0], 0, 3);
    });
    let b = programa(&[], |a, _| {
        // O socket 0 existe, mas é do outro programa.
        a.chamada(SYS_TCP_CLOSE, [0, 0, 0], ERR_BADF, 1);
        a.chamada(SYS_TCP_RECV, [0, BUF, 4], ERR_BADF, 2);
        a.chamada(SYS_TCP_ACCEPT, [0, 0, 0], ERR_BADF, 3);
        a.chamada(SYS_TCP_TIMEOUT, [0, 5, 0], ERR_BADF, 4);
    });
    let (relatorio, _) = rodar(&[("a", &a), ("b", &b)]);
    saiu_bem(&relatorio, "a");
    saiu_bem(&relatorio, "b");
    assert_eq!(esperar_pilha_vazia(400), 0);
}

#[test_case]
fn limite_de_oito_conexoes_no_kernel_inteiro() {
    let frames = memory::frames_outstanding();
    let quatro = |base: u64| {
        programa(&[], move |a, _| {
            for i in 0..4 {
                a.chamada(SYS_TCP_LISTEN, [base + i, 0, 0], i as i64, 1 + i as u32);
            }
            a.chamada(SYS_YIELD, [0, 0, 0], 0, 10);
            a.chamada(SYS_YIELD, [0, 0, 0], 0, 11);
        })
    };
    let a = quatro(9001);
    let b = quatro(9011);
    let c = programa(&[], |a, _| {
        a.chamada(SYS_TCP_LISTEN, [9021, 0, 0], ERR_NOBUFS, 1);
        a.chamada(SYS_TCP_CONNECT, [LOCALHOST, 9001, 0], ERR_NOBUFS, 2);
    });
    let (relatorio, _) = rodar(&[("a", &a), ("b", &b), ("c", &c)]);
    saiu_bem(&relatorio, "a");
    saiu_bem(&relatorio, "b");
    saiu_bem(&relatorio, "c");
    assert_eq!(esperar_pilha_vazia(400), 0);
    assert_eq!(memory::frames_outstanding(), frames);
}

// ---------------------------------------------------------------------------
// Término
// ---------------------------------------------------------------------------

/// Abre um `listen`, conecta nele e aceita: três sockets abertos.
fn abre_tres(a: &mut Asm) {
    a.chamada(SYS_TCP_LISTEN, [8000, 0, 0], 0, 1);
    a.chamada(SYS_TCP_CONNECT, [LOCALHOST, 8000, 0], 1, 2);
    a.chamada(SYS_TCP_ACCEPT, [0, 0, 0], 2, 3);
}

#[test_case]
fn termino_por_exit_e_por_falha_com_sockets_abertos_libera_tudo() {
    let frames = memory::frames_outstanding();
    // Sai com `exit` sem fechar nada.
    let por_exit = programa(&[], |a, _| abre_tres(a));
    let (relatorio, _) = rodar(&[("por_exit", &por_exit)]);
    saiu_bem(&relatorio, "por_exit");
    assert_eq!(esperar_pilha_vazia(400), 0, "exit");
    // Cai por instrução inválida (#UD) com os três abertos.
    let por_falha = programa(&[], |a, _| {
        abre_tres(a);
        a.0.extend_from_slice(&[0x0F, 0x0B]); // ud2
    });
    let (relatorio, _) = rodar(&[("por_falha", &por_falha)]);
    assert_ne!(relatorio.finished[0].termination, Termination::Exit { code: 0 });
    assert_eq!(esperar_pilha_vazia(400), 0, "#UD");
    // Morre bloqueado na rede? O pedido pendente vive só na tarefa: sai com ela.
    assert_eq!(memory::frames_outstanding(), frames);
}

// ---------------------------------------------------------------------------
// Envio com o buffer cheio (direto na camada de sockets)
// ---------------------------------------------------------------------------

fn pronto(outcome: Outcome) -> i64 {
    match outcome {
        Outcome::Done(r) => r,
        Outcome::Block(_) => panic!("esperava resultado imediato"),
    }
}

#[test_case]
fn send_com_buffer_cheio_bloqueia_ate_haver_espaco() {
    let mut table = SocketTable::new();
    let agora = || timer::ticks();
    assert_eq!(net::sys_listen(&mut table, 8000), 0);
    let conectou = pronto(net::sys_connect(&mut table, [127, 0, 0, 1], 8000, agora()));
    assert_eq!(conectou, 1);
    let aceito = pronto(net::sys_accept(&mut table, 0, agora()));
    assert_eq!(aceito, 2);
    assert_eq!(net::sys_timeout(&mut table, 1, 0), 0);

    // Enche o caminho: o buffer de envio (2048) e o de recepção do par (2048).
    let dados = [0x5Au8; 1024];
    let mut aceitos = 0usize;
    let motivo = loop {
        let r = pronto(net::sys_send(&mut table, 1, dados.as_ptr() as u64, 1024, agora()));
        if r <= 0 {
            break r;
        }
        aceitos += r as usize;
        assert!(aceitos <= 8192, "o buffer nunca encheu");
    };
    assert_eq!(motivo, ERR_TIMEDOUT, "sem espaco e sem espera");
    assert!((2048..=4096).contains(&aceitos), "{} bytes aceitos", aceitos);

    // O par lê tudo; o espaço reabre e o envio volta a funcionar.
    let mut lidos = 0usize;
    let mut buf = [0u8; 512];
    assert_eq!(net::sys_timeout(&mut table, 2, 0), 0);
    while lidos < aceitos {
        let r = pronto(net::sys_recv(&mut table, 2, buf.as_mut_ptr() as u64, 512, agora()));
        if r > 0 {
            assert!(buf[..r as usize].iter().all(|&b| b == 0x5A));
            lidos += r as usize;
        } else {
            // Ainda em trânsito (para-e-espera): deixa a pilha andar.
            x86_64::instructions::interrupts::disable();
            net::poll();
            x86_64::instructions::interrupts::enable_and_hlt();
        }
    }
    assert_eq!(lidos, aceitos);
    let r = pronto(net::sys_send(&mut table, 1, dados.as_ptr() as u64, 1024, agora()));
    assert_eq!(r, 1024);
    drop(table);
    assert_eq!(esperar_pilha_vazia(400), 0);
}
