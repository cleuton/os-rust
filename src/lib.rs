#![no_std]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
#![cfg_attr(test, no_main)]
#![cfg_attr(test, feature(custom_test_frameworks))]
#![cfg_attr(test, test_runner(crate::test_runner))]
#![cfg_attr(test, reexport_test_harness_main = "test_main")]

extern crate alloc;

pub mod allocator;
pub mod vga_buffer;
pub mod serial;
pub mod gdt;
pub mod interrupts;
pub mod keyboard;
pub mod memory;
pub mod shell;
pub mod panic;
pub mod logo;
pub mod elf;
pub mod programs;
pub mod syscall;
pub mod user;

/// Nome do projeto, derivado literalmente do campo `name` de
/// `Cargo.toml` em tempo de compilação — `env!("CARGO_PKG_NAME")`
/// preserva o hífen do manifesto (`"os-rust"`), ao contrário do
/// identificador da crate (`os_rust`, com underscore). Única fonte do
/// nome exibido em todo o código, para nunca ficar dessincronizado do
/// nome real do pacote; reutilizada por `shell::cmd_sobre` (`shell::PROMPT`
/// e `VERSION` repetem o `env!` por causa de `concat!`).
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// Identificação do sistema no formato `os-rust vX.Y.Z`, derivada do
/// nome e da versão do pacote em `Cargo.toml`, em tempo de compilação —
/// a única fonte do nome e da versão em todo o código, para que um bump
/// de versão baste alterar `Cargo.toml`. Reutilizada por `print_welcome`,
/// pela primeira linha de diagnóstico da serial, por `shell::cmd_sobre`,
/// por `panic::handle` e pela tela de exceção fatal de `interrupts.rs`.
pub const VERSION: &str = concat!(env!("CARGO_PKG_NAME"), " v", env!("CARGO_PKG_VERSION"));

/// Escreve a mensagem de boas-vindas na tela: uma única linha, `VERSION`.
/// Chamada pelo binário de produção (`main.rs::kernel_main`) e por
/// testes de integração que verificam o conteúdo exato da tela — extraída
/// para a biblioteca justamente para ser testável por `cargo test`.
pub fn print_welcome() {
    println!("{}", VERSION);
}

/// Inicializa a infraestrutura de baixo nível do kernel, nesta ordem:
/// porta serial primeiro (nenhuma mensagem de diagnóstico existe antes
/// dela, e elas só vão para a serial, nunca para a tela); GDT/TSS antes
/// da IDT, porque o handler de double fault na IDT referencia o índice
/// de pilha que só a TSS reserva; interrupções antes de memória, porque
/// nada em memória depende delas; memória depois; `syscall` por último,
/// porque lê os seletores de segmento da GDT já carregada. Uma mensagem de
/// diagnóstico na serial depois de cada etapa ajuda a localizar em qual
/// delas o boot parou, se parar. Chamada tanto pelo binário de produção
/// (`main.rs`) quanto pelos pontos de entrada de teste (`lib.rs`,
/// `main.rs` em modo de teste, `tests/*.rs`).
pub fn init(boot_info: &'static bootloader::BootInfo) {
    serial::init();
    serial_println!("[boot] {} iniciado", VERSION);
    gdt::init();
    serial_println!("[boot] gdt/tss ativos");
    interrupts::init();
    serial_println!("[boot] interrupcoes ativas");
    memory::init(boot_info);
    let info = memory::info();
    serial_println!(
        "[boot] memoria inicializada: {} KiB utilizaveis, heap em {:#x} ({} KiB)",
        info.usable_bytes / 1024,
        info.heap_start,
        info.heap_size / 1024
    );
    syscall::init();
    serial_println!("[boot] syscall ativo");
}

/// Um teste executável pelo executor de testes: qualquer função sem
/// parâmetros ganha esta capacidade automaticamente (`impl<T: Fn()>`
/// abaixo) — imprime o nome do teste antes de rodar e "[ok]" depois,
/// sem exigir que cada teste declare isso manualmente.
pub trait Testable {
    fn run(&self) -> ();
}

impl<T> Testable for T
where
    T: Fn(),
{
    fn run(&self) {
        serial_print!("{}...\t", core::any::type_name::<T>());
        self();
        serial_println!("[ok]");
    }
}

/// Executor de testes: imprime a contagem total, roda cada teste na
/// ordem em que aparece e, se todos retornarem sem panic, sinaliza
/// sucesso ao host. Um panic dentro de qualquer teste nunca retorna a
/// este ponto — o `#[panic_handler]` de teste assume o controle e chama
/// `exit_qemu(QemuExitCode::Failed)`, reportando a falha ao host mesmo
/// sem o executor chegar a rodar de novo.
pub fn test_runner(tests: &[&dyn Testable]) {
    serial_println!("Running {} tests", tests.len());
    for test in tests {
        test.run();
    }
    exit_qemu(QemuExitCode::Success);
}

/// Valores escritos no dispositivo `isa-debug-exit` do QEMU para
/// sinalizar o resultado ao host. `isa-debug-exit` transforma o valor
/// escrito `V` no código de saída do processo QEMU via `(V << 1) | 1`,
/// então `Success` (`0x10`) produz `33` e `Failed` (`0x11`) produz `35`
/// — `33` é exatamente o `test-success-exit-code` configurado em
/// `Cargo.toml`, o valor que o script de teste espera para considerar a
/// suíte bem-sucedida.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
}

/// Encerra o QEMU informando `code` ao host, através do dispositivo
/// `isa-debug-exit` (presente só durante `cargo test`, via `test-args`
/// em `Cargo.toml` — nunca em `cargo run`, então esta função nunca roda
/// fora de um teste).
pub fn exit_qemu(code: QemuExitCode) -> ! {
    use x86_64::instructions::port::Port;

    // SAFETY: 0xf4 é o endereço de I/O do dispositivo `isa-debug-exit`
    // configurado só nos argumentos de teste do QEMU (`test-args`);
    // escrever nele é a forma documentada desse dispositivo de encerrar
    // o QEMU e repassar `code` como parte do código de saída do
    // processo — não há outro código do kernel usando essa porta.
    unsafe {
        let mut port = Port::new(0xf4);
        port.write(code as u32);
    }

    // `isa-debug-exit` sempre encerra o processo QEMU antes deste ponto
    // ser alcançado; este `halt_loop` só existe para satisfazer o tipo
    // de retorno `!` caso, por algum motivo externo ao kernel, o QEMU
    // não tenha realmente encerrado.
    panic::halt_loop();
}

/// `#[panic_handler]` usado em modo de teste (`lib.rs`, `main.rs` sob
/// `#[cfg(test)]`, e cada arquivo de `tests/` exceto `should_panic.rs`,
/// que tem o seu próprio): qualquer panic durante um teste é reportado
/// ao host como falha, imediatamente, em vez de deixar o QEMU travado
/// esperando o `test-timeout` de `Cargo.toml` esgotar.
pub fn test_panic_handler(info: &core::panic::PanicInfo) -> ! {
    serial_println!("[failed]\n");
    serial_println!("Error: {}\n", info);
    exit_qemu(QemuExitCode::Failed);
}

// Ponto de entrada usado quando a própria biblioteca é compilada em modo
// de teste (`cargo test`, testes de unidade de `lib.rs` e de seus
// módulos): inicializa o kernel normalmente e roda a suíte.
#[cfg(test)]
bootloader::entry_point!(test_kernel_main);

#[cfg(test)]
fn test_kernel_main(boot_info: &'static bootloader::BootInfo) -> ! {
    init(boot_info);
    test_main();
    panic::halt_loop();
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    test_panic_handler(info)
}
