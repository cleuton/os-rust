//! Escrita de texto na porta serial (UART 16550, COM1) para o terminal do host.

use core::fmt;
use lazy_static::lazy_static;
use spin::Mutex;
use uart_16550::SerialPort;

/// Endereço de I/O fixo e padrão da primeira porta serial (COM1) em um PC.
const SERIAL_IO_PORT: u16 = 0x3F8;

lazy_static! {
    /// A porta serial global usada por `serial_print!`/`serial_println!`.
    static ref SERIAL1: Mutex<SerialPort> = {
        // SAFETY: 0x3F8 é o endereço de I/O fixo e padrão da primeira
        // porta serial de um PC; nenhum outro código do kernel cria uma
        // segunda instância de `SerialPort` para o mesmo endereço, então
        // não há dono duplicado dessas portas de I/O.
        let mut port = unsafe { SerialPort::new(SERIAL_IO_PORT) };
        port.init();
        Mutex::new(port)
    };
}

/// Inicializa a porta serial. Deve ser chamada uma única vez, antes de
/// qualquer outra escrita na serial (FR-001).
pub fn init() {
    // A inicialização de fato acontece na primeira vez que `SERIAL1` é
    // acessado (via `lazy_static!`); forçar esse acesso aqui garante que
    // ela aconteça neste ponto do boot, e não em algum uso posterior.
    lazy_static::initialize(&SERIAL1);
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => ($crate::serial::_serial_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _serial_print(args: fmt::Arguments) {
    use core::fmt::Write;
    // A aquisição do Mutex e a escrita inteira acontecem com interrupções
    // desabilitadas (FR-003): se o fluxo principal estiver com o lock
    // travado, ele nunca pode ser interrompido no meio dessa seção, então
    // um tratador de interrupção que também escreva na serial nunca tenta
    // adquirir o mesmo Mutex enquanto ele já está travado — sem disputa,
    // sem deadlock.
    x86_64::instructions::interrupts::without_interrupts(|| {
        // `SerialPort::write_str` (via `core::fmt::Write`) só falha se a
        // própria formatação falhar, o que não acontece aqui — o
        // `unwrap` existe só porque `write_fmt` retorna `fmt::Result`.
        SERIAL1.lock().write_fmt(args).unwrap();
    });
}

#[cfg(test)]
mod tests {
    #[test_case]
    fn escreve_na_serial_sem_falhar() {
        // Não há como ler de volta o outro lado do canal serial a partir
        // do próprio kernel; o teste passa se a chamada retorna
        // normalmente, sem travar nem entrar em panic (FR-002, FR-003).
        crate::serial_println!("teste de escrita na serial");
    }
}
