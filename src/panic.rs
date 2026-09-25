//! Tratamento de panic: mostra uma mensagem legível na tela em vez de
//! travar silenciosamente ou reiniciar sem aviso.

use crate::serial_println;
use crate::vga_buffer::WRITER;
use core::fmt::Write;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

/// `true` assim que o primeiro panic **ou** a primeira exceção fatal
/// (`interrupts::fatal_exception`) começa a ser tratada — as duas
/// situações compartilham a mesma trava porque compartilham o mesmo
/// risco: um segundo evento fatal reentrante tentando travar `WRITER`/
/// `SERIAL1` enquanto o primeiro ainda os segura.
static PANICKING: AtomicBool = AtomicBool::new(false);

/// Marca o início de um panic ou de uma exceção fatal. Devolve `true` se
/// já havia um panic ou uma exceção fatal em andamento (reentrante) —
/// nesse caso, o chamador não deve tentar travar `WRITER`/`SERIAL1` de
/// novo, só escrever na serial (se conseguir) e parar a CPU.
pub(crate) fn enter_fatal_handler() -> bool {
    PANICKING.swap(true, Ordering::SeqCst)
}

/// Ponto de entrada chamado pelo `#[panic_handler]` em `src/main.rs`.
pub fn handle(info: &PanicInfo) -> ! {
    if enter_fatal_handler() {
        // Já estávamos tratando um panic ou uma exceção fatal quando
        // este segundo evento disparou (ex.: um bug no próprio código de
        // formatação da mensagem). Não tentamos escrever na tela de novo
        // — o Mutex do WRITER pode já estar travado pelo primeiro evento
        // — então vamos direto parar a CPU, preservando a última
        // mensagem válida que já estava na tela.
        halt_loop();
    }

    serial_println!("[PANIC] {}", info);

    let mut writer = WRITER.lock();
    // `write_str`/`write!` sobre `Writer` nunca falham (ver vga_buffer.rs),
    // então ignorar o `Result` aqui não esconde nenhum erro real possível.
    let _ = write!(writer, "\n[PANIC] {} parou: ", crate::VERSION);
    let _ = write!(writer, "{}", info);
    drop(writer);

    halt_loop();
}

/// Para a CPU em definitivo, sem gastar CPU à toa e sem reiniciar.
///
/// Reaproveitada pelo handler de double fault (`interrupts.rs`) e pelos
/// pontos de entrada de teste em `lib.rs`/`main.rs`/`tests/*.rs`: uma
/// falha de CPU inesperada deve parar o sistema do mesmo jeito que um
/// panic de software, em vez de virar um reinício silencioso em loop.
/// Pública (não `pub(crate)`) porque, depois da reorganização em
/// biblioteca + binário, `main.rs` é um crate externo à biblioteca
/// `os_rust` e precisa poder chamá-la.
pub fn halt_loop() -> ! {
    loop {
        // SAFETY: `hlt` apenas pausa a CPU até a próxima interrupção; não
        // acessa memória nem modifica a pilha, então é seguro executá-la
        // em loop para parar a execução de forma segura.
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}
