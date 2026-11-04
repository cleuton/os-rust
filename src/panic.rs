//! Tratamento de panic: mostra uma mensagem legível na tela em vez de
//! travar silenciosamente ou reiniciar sem aviso.

use crate::vga_buffer::WRITER;
use core::fmt::Write;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

/// `true` assim que o primeiro panic começa a ser tratado. Usado para
/// detectar um panic reentrante (um panic disparado dentro do próprio
/// tratamento de panic).
static PANICKING: AtomicBool = AtomicBool::new(false);

/// Ponto de entrada chamado pelo `#[panic_handler]` em `src/main.rs`.
pub fn handle(info: &PanicInfo) -> ! {
    if PANICKING.swap(true, Ordering::SeqCst) {
        // Já estávamos tratando um panic quando este segundo panic
        // disparou (ex.: um bug no próprio código de formatação da
        // mensagem). Não tentamos escrever na tela de novo — o Mutex do
        // WRITER pode já estar travado pelo primeiro panic — então vamos
        // direto parar a CPU, preservando a última mensagem válida que já
        // estava na tela.
        halt_loop();
    }

    let mut writer = WRITER.lock();
    // `write_str`/`write!` sobre `Writer` nunca falham (ver vga_buffer.rs),
    // então ignorar o `Result` aqui não esconde nenhum erro real possível.
    let _ = writer.write_str("\n[PANIC] proto-os parou: ");
    let _ = write!(writer, "{}", info);
    drop(writer);

    halt_loop();
}

/// Para a CPU em definitivo, sem gastar CPU à toa e sem reiniciar.
fn halt_loop() -> ! {
    loop {
        // SAFETY: `hlt` apenas pausa a CPU até a próxima interrupção; não
        // acessa memória nem modifica a pilha, então é seguro executá-la
        // em loop para parar a execução de forma segura.
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}
