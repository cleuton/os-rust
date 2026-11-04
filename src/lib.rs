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
pub mod interrupts;
pub mod keyboard;
pub mod memory;
pub mod shell;
pub mod panic;

/// Inicializa a infraestrutura de baixo nível do kernel: porta serial
/// primeiro (FR-001), depois interrupções (IDT + PIC), depois memória
/// física/paginação/heap (FR-011), com uma mensagem de diagnóstico na
/// serial após cada etapa (FR-004, FR-018). Chamada tanto pelo binário
/// de produção (`main.rs`) quanto pelos pontos de entrada de teste
/// (`lib.rs`, `main.rs` em modo de teste, `tests/*.rs`).
pub fn init(boot_info: &'static bootloader::BootInfo) {
    serial::init();
    serial_println!("[boot] iniciado");
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
}

/// Um teste executável pelo executor de testes: qualquer função sem
/// parâmetros ganha esta capacidade automaticamente (`impl<T: Fn()>`
/// abaixo). Ver `data-model.md`, "Teste".
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
/// sucesso ao host (FR-009). Um panic dentro de qualquer teste nunca
/// retorna a este ponto — o `#[panic_handler]` de teste assume o
/// controle e chama `exit_qemu(QemuExitCode::Failed)` (FR-011).
pub fn test_runner(tests: &[&dyn Testable]) {
    serial_println!("Running {} tests", tests.len());
    for test in tests {
        test.run();
    }
    exit_qemu(QemuExitCode::Success);
}

/// Valores escritos no dispositivo `isa-debug-exit` do QEMU para
/// sinalizar o resultado ao host (ver `research.md`, seção 5, e
/// `contracts/serial-test-protocol.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
}

/// Encerra o QEMU informando `code` ao host, através do dispositivo
/// `isa-debug-exit` (presente só durante `cargo test`, via `test-args`
/// em `Cargo.toml` — nunca em `cargo run`, ver FR-018).
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
/// ao host como falha, imediatamente (FR-011).
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
