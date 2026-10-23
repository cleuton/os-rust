//! `servidor`: espera uma conexão TCP na porta 8000 e responde a uma
//! requisição, no formato mínimo de uma página HTTP/1.0.
//!
//! É a metade "servidora" da demonstração de rede do Marco 12. Rode junto do
//! `cliente` (`run servidor cliente`), e o tráfego passa pelo loopback
//! (`127.0.0.1`), sem nenhuma placa de rede. Com o QEMU iniciado à mão com
//! `hostfwd` (README, "Rede no QEMU"), o próprio computador de verdade também
//! consegue conversar com ele: `curl http://127.0.0.1:8080/`.
//!
//! O programa não interpreta a requisição: lê o que chegar (uma vez, até 512
//! bytes) e devolve sempre a mesma página, com 65 bytes de cabeçalho e 23 de
//! corpo. Toda espera tem tempo limite, porque não existe Ctrl+C: se ninguém
//! se conectar em 30 segundos, o programa desiste e o prompt volta.

#![no_std]
#![no_main]

use runtime::{entry, println, NetError, TcpListener};

entry!(main);

/// A porta em que o servidor espera.
const PORTA: u16 = 8000;

/// Quanto o servidor espera alguém se conectar (30 s).
const ESPERA_CONEXAO_MS: u32 = 30_000;

/// Quanto espera a requisição, depois de conectado (10 s).
const ESPERA_DADOS_MS: u32 = 10_000;

/// O corpo da resposta: 23 bytes, com o `\n` final.
const CORPO: &str = "ola da rede do os-rust\n";

/// A resposta inteira: 65 bytes de cabeçalho (`\r\n` termina cada linha, e uma
/// linha em branco termina o cabeçalho) mais os 23 do corpo, 88 no total.
const RESPOSTA: &[u8] =
    b"HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 23\r\n\r\nola da rede do os-rust\n";

fn main() -> i32 {
    match servir() {
        Ok(()) => 0,
        Err(codigo) => codigo,
    }
}

/// Atende uma conexão. O erro é o código de saída do programa.
fn servir() -> Result<(), i32> {
    let ouvinte = TcpListener::bind(PORTA).map_err(erro_de_rede)?;
    ouvinte.set_timeout(ESPERA_CONEXAO_MS).map_err(erro_de_rede)?;
    println!("servidor: ouvindo na porta {}", PORTA);

    let conexao = match ouvinte.accept() {
        Ok(conexao) => conexao,
        Err(NetError::TimedOut) => {
            println!("servidor: ninguem se conectou em 30 s");
            return Err(1);
        }
        Err(erro) => return Err(erro_de_rede(erro)),
    };
    conexao.set_timeout(ESPERA_DADOS_MS).map_err(erro_de_rede)?;

    // Uma leitura basta: a requisição do `cliente` cabe num segmento só.
    let mut requisicao = [0u8; 512];
    let lidos = conexao.read(&mut requisicao).map_err(erro_de_rede)?;
    println!("servidor: recebi {} bytes", lidos);

    conexao.write_all(RESPOSTA).map_err(erro_de_rede)?;
    // Fechar a conexão e o ouvinte: o kernel termina de enviar o que falta e
    // encerra a conexão em segundo plano, mesmo depois de o programa acabar.
    drop(conexao);
    drop(ouvinte);
    println!("servidor: respondi {} bytes de corpo", CORPO.len());
    Ok(())
}

/// Mostra o erro e devolve o código de saída 2.
fn erro_de_rede(erro: NetError) -> i32 {
    println!("servidor: erro de rede: {}", erro);
    2
}
