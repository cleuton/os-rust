//! `cliente`: conecta num servidor na porta 8000, pede uma página e a mostra.
//!
//! É a metade "cliente" da demonstração de rede do Marco 12. Pergunta o
//! endereço do servidor; apertar só Enter usa `127.0.0.1`, e então o par é o
//! programa `servidor` (`run servidor cliente`), pelo loopback, sem placa de
//! rede. Digitando `10.0.2.2` (o computador de verdade, visto de dentro do
//! QEMU), conversa com um servidor do host, por exemplo
//! `python3 -m http.server 8000` (README, "Rede no QEMU").
//!
//! A requisição tem 33 bytes. A resposta é escrita na tela sem os `\r` (que só
//! servem ao protocolo), até 1024 bytes; se vier mais que isso, o programa
//! mostra o começo e avisa que cortou. Toda espera tem tempo limite (10 s).

#![no_std]
#![no_main]

use runtime::sys::write;
use runtime::{entry, print, println, read_line, Ipv4, NetError, TcpStream};

entry!(main);

/// A porta do servidor.
const PORTA: u16 = 8000;

/// O pedido: 33 bytes (`GET / HTTP/1.0\r\n` 16, `Host: os-rust\r\n` 15, `\r\n` 2).
const REQUISICAO: &[u8] = b"GET / HTTP/1.0\r\nHost: os-rust\r\n\r\n";

/// Quanto da resposta vai para a tela.
const LIMITE_NA_TELA: usize = 1024;

/// Quanto espera cada resposta do servidor (10 s).
const ESPERA_MS: u32 = 10_000;

fn main() -> i32 {
    print!("cliente: endereco do servidor (vazio = 127.0.0.1): ");
    let mut linha = [0u8; 64];
    let texto = read_line(&mut linha).trim();
    let ip = if texto.is_empty() {
        Ipv4([127, 0, 0, 1])
    } else {
        match Ipv4::parse(texto) {
            Some(ip) => ip,
            None => {
                println!("cliente: endereco invalido: {}", texto);
                return 2;
            }
        }
    };

    let conexao = match TcpStream::connect(ip, PORTA) {
        Ok(conexao) => conexao,
        Err(erro) => {
            println!("cliente: nao conectou em {}:{}: {}", ip, PORTA, erro);
            return 1;
        }
    };

    match conversar(&conexao) {
        Ok(()) => 0,
        Err(erro) => {
            println!("cliente: erro de rede: {}", erro);
            2
        }
    }
}

/// Envia o pedido e mostra a resposta até o servidor fechar a conexão.
fn conversar(conexao: &TcpStream) -> Result<(), NetError> {
    conexao.set_timeout(ESPERA_MS)?;
    conexao.write_all(REQUISICAO)?;

    let mut lidos_no_total = 0;
    let mut escritos = 0;
    let mut cortou = false;
    let mut pedaco = [0u8; 256];
    loop {
        let n = conexao.read(&mut pedaco)?;
        if n == 0 {
            break;
        }
        lidos_no_total += n;
        // Tira os `\r` e respeita o limite da tela.
        let mut limpo = [0u8; 256];
        let mut tamanho = 0;
        for &byte in &pedaco[..n] {
            if byte == b'\r' {
                continue;
            }
            if escritos + tamanho == LIMITE_NA_TELA {
                cortou = true;
                break;
            }
            limpo[tamanho] = byte;
            tamanho += 1;
        }
        write(&limpo[..tamanho]);
        escritos += tamanho;
        if cortou {
            break;
        }
    }

    if cortou {
        println!("cliente: {} bytes recebidos (cortado)", LIMITE_NA_TELA);
    } else {
        println!("cliente: {} bytes recebidos", lidos_no_total);
    }
    Ok(())
}
