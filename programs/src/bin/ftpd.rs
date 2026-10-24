//! `ftpd`: um servidor FTP que troca arquivos entre o computador de verdade e
//! os volumes `/ram` e `/disco` do os-rust.
//!
//! O programa não guarda arquivos: cada comando FTP vira uma chamada de
//! arquivo (`File`, `FileWriter`, `Dir`, `remove`, `mkdir`, `rmdir`) e os bytes
//! viajam por TCP (`TcpListener` e `TcpStream`). Como o FTP usa duas conexões,
//! uma de **controle** (os comandos e as respostas, porta 21) e uma de
//! **dados** (o conteúdo de uma transferência, porta 2100), o programa fica com
//! no máximo quatro sockets abertos: a escuta da 21, a conexão de controle, a
//! escuta da 2100 e a conexão de dados. É exatamente o limite de cada programa,
//! por isso atende **uma sessão por vez** e só fala o **modo passivo** (`PASV`
//! e `EPSV`): no modo ativo o os-rust teria de conectar de volta ao cliente.
//!
//! Para o computador alcançar o servidor, o QEMU é iniciado à mão com
//! `hostfwd` (README, "Servidor FTP"); então `curl ftp://127.0.0.1:2121/` lista
//! as pastas `ram` e `disco`. Rode com `run ftpd`, aperte Enter na pergunta do
//! endereço (ele vai no `PASV`) e use o cliente FTP do computador.
//!
//! Não existe espera infinita nem Ctrl+C, então o servidor tem tempos: 30 s
//! esperando um cliente, 30 s sem receber comando e 10 s por espera de dados.
//! Ele termina sozinho depois de 30 s sem cliente, ou quando o cliente manda
//! `SITE EXIT`, e o prompt volta.
//!
//! Sem senha: qualquer `USER` e `PASS` entram, e todo arquivo é binário (nenhuma
//! quebra de linha é convertida). `STOR` nunca sobrescreve: um nome que já
//! existe dá erro, e uma transferência que falha no meio apaga o arquivo
//! parcial. Para trocar um arquivo, apague-o antes (`DELE`).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use runtime::{
    entry, mkdir, print, println, read_line, remove, rmdir, Dir, File, FileWriter, FsError, Ipv4,
    NetError, TcpListener, TcpStream,
};

entry!(main);

/// A porta da conexão de controle.
const PORTA_CONTROLE: u16 = 21;

/// A porta da conexão de dados, a mesma que o `PASV` anuncia (8 * 256 + 52).
const PORTA_DADOS: u16 = 2100;

/// Quanto espera um cliente se conectar (30 s).
const ESPERA_CLIENTE_MS: u32 = 30_000;

/// Quanto espera o próximo comando de um cliente conectado (30 s).
const ESPERA_COMANDO_MS: u32 = 30_000;

/// Quanto espera a conexão de dados ser aberta e cada leitura ou escrita dela
/// (10 s).
const ESPERA_DADOS_MS: u32 = 10_000;

/// Tamanho do buffer de comandos: uma linha de 512 bytes ou mais sem fim de
/// linha é recusada.
const TAM_LINHA: usize = 512;

/// Tamanho de cada pedaço de uma transferência (um segmento TCP).
const TAM_PEDACO: usize = 1024;

/// A resposta de quem pede algo na raiz virtual `/`, que só lista `ram` e `disco`.
const RAIZ_SO_LISTA: &str = "550 a raiz so lista /ram e /disco";

/// A resposta de um caminho que não começa por `ram` nem por `disco`.
const NAO_ENCONTRADO: &str = "550 nao encontrado";

/// A resposta de um comando de dados sem `PASV` ou `EPSV` antes.
const SEM_PASV: &str = "425 use PASV ou EPSV antes";

/// A resposta de uma conexão de dados que caiu ou parou.
const INTERROMPIDA: &str = "426 transferencia interrompida";

/// O que fazer depois de um comando.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fluxo {
    /// Esperar o próximo comando.
    Continua,
    /// Terminar esta sessão e esperar outro cliente.
    Encerra,
    /// Terminar a sessão **e** o programa (`SITE EXIT`).
    Sai,
}

fn main() -> i32 {
    match servir() {
        Ok(()) => 0,
        Err(codigo) => codigo,
    }
}

/// Pergunta o endereço, abre a escuta e atende um cliente por vez. O erro é o
/// código de saída do programa.
fn servir() -> Result<(), i32> {
    print!("ftpd: endereco a anunciar no modo passivo (vazio = 127.0.0.1): ");
    let mut linha = [0u8; 64];
    let texto = read_line(&mut linha).trim();
    let anunciado = if texto.is_empty() {
        Ipv4([127, 0, 0, 1])
    } else {
        match Ipv4::parse(texto) {
            Some(ip) => ip,
            None => {
                println!("ftpd: endereco invalido: {}", texto);
                return Err(2);
            }
        }
    };

    let ouvinte = TcpListener::bind(PORTA_CONTROLE).map_err(erro_de_rede)?;
    ouvinte.set_timeout(ESPERA_CLIENTE_MS).map_err(erro_de_rede)?;
    println!(
        "ftpd: ouvindo na porta {} (dados na porta {}), anunciando {}",
        PORTA_CONTROLE, PORTA_DADOS, anunciado
    );

    loop {
        let controle = match ouvinte.accept() {
            Ok(controle) => controle,
            Err(NetError::TimedOut) => {
                println!("ftpd: ninguem se conectou em 30 s, encerrando");
                return Ok(());
            }
            Err(erro) => return Err(erro_de_rede(erro)),
        };
        println!("ftpd: cliente conectado");
        // A sessão é consumida aqui: ao terminar, a conexão de controle, o
        // ouvinte de dados e a conexão de dados saem de escopo e são fechados.
        let fluxo = Sessao::nova(controle, anunciado).executar();
        println!("ftpd: sessao encerrada");
        if fluxo == Fluxo::Sai {
            println!("ftpd: encerrado por SITE EXIT");
            return Ok(());
        }
    }
}

/// Mostra o erro e devolve o código de saída 2.
fn erro_de_rede(erro: NetError) -> i32 {
    println!("ftpd: erro de rede: {}", erro);
    2
}

/// Uma sessão: o que vale entre a conexão de um cliente e o fim dela.
struct Sessao {
    /// A conexão de controle: comandos de entrada, respostas de saída.
    controle: TcpStream,
    /// Bytes lidos do controle que ainda não formaram uma linha inteira.
    entrada: [u8; TAM_LINHA],
    tam: usize,
    /// O diretório atual, sempre normalizado (`/` ou `/ram/docs`...).
    cwd: String,
    usuario_visto: bool,
    logado: bool,
    /// O ouvinte da porta 2100, armado por `PASV` ou `EPSV` e gasto por um
    /// comando de dados. Nunca há dois: a escuta duplicada daria erro.
    dados: Option<TcpListener>,
    /// O endereço que o `PASV` anuncia.
    anunciado: Ipv4,
}

impl Sessao {
    fn nova(controle: TcpStream, anunciado: Ipv4) -> Sessao {
        Sessao {
            controle,
            entrada: [0; TAM_LINHA],
            tam: 0,
            cwd: String::from("/"),
            usuario_visto: false,
            logado: false,
            dados: None,
            anunciado,
        }
    }

    /// Cumprimenta o cliente e atende comandos até a sessão acabar.
    fn executar(mut self) -> Fluxo {
        if self.controle.set_timeout(ESPERA_COMANDO_MS).is_err() {
            return Fluxo::Encerra;
        }
        if self.responder("220 os-rust ftpd pronto").is_err() {
            return Fluxo::Encerra;
        }
        loop {
            // Um segmento pode trazer vários comandos, e um comando pode vir
            // partido em vários segmentos: só se age sobre linhas inteiras.
            while let Some(fim) = self.entrada[..self.tam].iter().position(|&b| b == b'\n') {
                let mut linha: Vec<u8> = self.entrada[..fim].to_vec();
                if linha.last() == Some(&b'\r') {
                    linha.pop();
                }
                self.entrada.copy_within(fim + 1..self.tam, 0);
                self.tam -= fim + 1;
                let fluxo = match core::str::from_utf8(&linha) {
                    Ok(texto) => self.tratar(texto),
                    // Bytes fora do ASCII: um verbo que ninguém conhece.
                    Err(_) => self.tratar("?"),
                };
                if fluxo != Fluxo::Continua {
                    return fluxo;
                }
            }
            if self.tam == TAM_LINHA {
                let _ = self.responder("421 linha longa demais");
                return Fluxo::Encerra;
            }
            match self.controle.read(&mut self.entrada[self.tam..]) {
                // O cliente fechou a conexão sem `QUIT`.
                Ok(0) => return Fluxo::Encerra,
                Ok(n) => self.tam += n,
                Err(NetError::TimedOut) => {
                    let _ = self.responder("421 tempo esgotado, ate logo");
                    return Fluxo::Encerra;
                }
                Err(_) => return Fluxo::Encerra,
            }
        }
    }

    /// Escreve uma resposta no controle, com o `\r\n` do protocolo.
    fn responder(&self, texto: &str) -> Result<(), NetError> {
        let linha = format!("{}\r\n", texto);
        self.controle.write_all(linha.as_bytes())
    }

    /// Responde e segue; se a resposta não puder ser escrita, a sessão acaba
    /// em silêncio (o cliente já foi embora).
    fn dizer(&self, texto: &str) -> Fluxo {
        if self.responder(texto).is_ok() {
            Fluxo::Continua
        } else {
            Fluxo::Encerra
        }
    }

    /// Interpreta uma linha de comando.
    fn tratar(&mut self, linha: &str) -> Fluxo {
        let linha = linha.trim();
        let (verbo, arg) = match linha.split_once(' ') {
            Some((verbo, arg)) => (verbo, arg.trim()),
            None => (linha, ""),
        };
        let verbo = verbo.to_ascii_uppercase();

        if !self.logado && !matches!(verbo.as_str(), "USER" | "PASS" | "QUIT" | "NOOP") {
            return self.dizer("530 faca login primeiro");
        }

        match verbo.as_str() {
            "USER" => {
                if arg.is_empty() {
                    return self.dizer("501 faltou o argumento");
                }
                self.usuario_visto = true;
                self.dizer("331 envie a senha")
            }
            "PASS" => {
                if !self.usuario_visto {
                    return self.dizer("503 envie USER antes");
                }
                self.logado = true;
                self.dizer("230 login aceito")
            }
            "QUIT" => {
                let _ = self.responder("221 ate logo");
                Fluxo::Encerra
            }
            "NOOP" => self.dizer("200 ok"),
            "SYST" => self.dizer("215 UNIX Type: L8"),
            "TYPE" => {
                // Os dois tipos transferem os mesmos bytes: nada é convertido.
                let tipo = arg.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
                match tipo.as_str() {
                    "" => self.dizer("501 faltou o argumento"),
                    "A" => self.dizer("200 tipo A"),
                    "I" => self.dizer("200 tipo I"),
                    _ => self.dizer("504 tipo nao suportado"),
                }
            }
            "SITE" => {
                if arg.is_empty() {
                    self.dizer("501 faltou o argumento")
                } else if arg.eq_ignore_ascii_case("EXIT") {
                    let _ = self.responder("221 encerrando o servidor");
                    Fluxo::Sai
                } else {
                    self.dizer("504 so SITE EXIT existe")
                }
            }
            "PWD" => {
                let resposta = format!("257 \"{}\" e o diretorio atual", self.cwd);
                self.dizer(&resposta)
            }
            "CWD" => {
                if arg.is_empty() {
                    self.dizer("501 faltou o argumento")
                } else {
                    self.mudar_diretorio(arg)
                }
            }
            "CDUP" => self.mudar_diretorio(".."),
            "PASV" | "EPSV" => self.armar_dados(verbo == "PASV"),
            "LIST" | "NLST" => {
                let fluxo = self.listar(arg, verbo == "LIST");
                // Todo comando de dados gasta o ouvinte, dê certo ou não.
                self.dados = None;
                fluxo
            }
            "RETR" => {
                let fluxo = self.baixar(arg);
                self.dados = None;
                fluxo
            }
            "STOR" => {
                let fluxo = self.enviar(arg);
                self.dados = None;
                fluxo
            }
            "SIZE" => self.tamanho(arg),
            "DELE" => self.apagar(arg),
            "MKD" => self.criar_diretorio(arg),
            "RMD" => self.remover_diretorio(arg),
            _ => self.dizer("502 comando nao implementado"),
        }
    }

    /// `CWD` e `CDUP`: só muda de diretório se o destino for mesmo um diretório.
    fn mudar_diretorio(&mut self, arg: &str) -> Fluxo {
        let caminho = match resolver(&self.cwd, arg) {
            Ok(caminho) => caminho,
            Err(resposta) => return self.dizer(&resposta),
        };
        // Abrir e ler a primeira entrada é o que revela um arquivo
        // (`WrongType`) ou um volume ausente; a raiz virtual não existe no kernel.
        if let Err(resposta) = listar_entradas(&caminho) {
            return self.dizer(&resposta);
        }
        self.cwd = caminho;
        self.dizer("250 diretorio mudado")
    }

    /// `PASV` e `EPSV`: abre a escuta de dados e diz ao cliente onde conectar.
    fn armar_dados(&mut self, passivo_simples: bool) -> Fluxo {
        // O ouvinte antigo sai antes do novo entrar: duas escutas na mesma
        // porta dão "porta em uso".
        self.dados = None;
        let ouvinte = match TcpListener::bind(PORTA_DADOS) {
            Ok(ouvinte) => ouvinte,
            Err(NetError::NoBuffers) => return self.dizer("425 sem conexoes livres no kernel"),
            Err(erro) => return self.dizer(&format!("425 erro de rede: {}", erro)),
        };
        if let Err(erro) = ouvinte.set_timeout(ESPERA_DADOS_MS) {
            return self.dizer(&format!("425 erro de rede: {}", erro));
        }
        self.dados = Some(ouvinte);
        if passivo_simples {
            let ip = self.anunciado.0;
            let resposta = format!(
                "227 Entering Passive Mode ({},{},{},{},{},{})",
                ip[0],
                ip[1],
                ip[2],
                ip[3],
                PORTA_DADOS >> 8,
                PORTA_DADOS & 0xff
            );
            self.dizer(&resposta)
        } else {
            let resposta =
                format!("229 Entering Extended Passive Mode (|||{}|)", PORTA_DADOS);
            self.dizer(&resposta)
        }
    }

    /// Espera o cliente abrir a conexão de dados no ouvinte armado. O ouvinte
    /// é fechado aqui, com qualquer desfecho: serve a uma transferência só.
    fn abrir_dados(&mut self) -> Result<TcpStream, String> {
        let ouvinte = self.dados.take().ok_or_else(|| String::from(SEM_PASV))?;
        let conexao = match ouvinte.accept() {
            Ok(conexao) => conexao,
            Err(NetError::TimedOut) => {
                return Err(String::from("425 o cliente nao abriu a conexao de dados"))
            }
            Err(erro) => return Err(format!("425 erro de rede: {}", erro)),
        };
        drop(ouvinte);
        conexao
            .set_timeout(ESPERA_DADOS_MS)
            .map_err(|_| String::from(INTERROMPIDA))?;
        Ok(conexao)
    }

    /// `LIST` (uma linha detalhada por entrada) e `NLST` (só os nomes).
    fn listar(&mut self, arg: &str, detalhado: bool) -> Fluxo {
        if self.dados.is_none() {
            return self.dizer(SEM_PASV);
        }
        // Opções como `-a` e `-la` são ignoradas; a primeira palavra que sobra
        // é o diretório.
        let alvo = arg.split_whitespace().find(|p| !p.starts_with('-')).unwrap_or("");
        let caminho = match resolver(&self.cwd, alvo) {
            Ok(caminho) => caminho,
            Err(resposta) => return self.dizer(&resposta),
        };
        // A listagem inteira é lida antes do `150`: se o diretório não abre,
        // o cliente recebe o erro em vez de uma transferência vazia.
        let entradas = match listar_entradas(&caminho) {
            Ok(entradas) => entradas,
            Err(resposta) => return self.dizer(&resposta),
        };
        if self.dizer("150 abrindo a conexao de dados") == Fluxo::Encerra {
            return Fluxo::Encerra;
        }
        let conexao = match self.abrir_dados() {
            Ok(conexao) => conexao,
            Err(resposta) => return self.dizer(&resposta),
        };
        let mut texto = String::new();
        for entrada in &entradas {
            if detalhado {
                let (modo, tamanho) = if entrada.diretorio {
                    ("drwxr-xr-x", 0)
                } else {
                    ("-rw-r--r--", entrada.tamanho)
                };
                texto.push_str(&format!(
                    "{} 1 os-rust os-rust {} Jan  1  1980 {}\r\n",
                    modo, tamanho, entrada.nome
                ));
            } else {
                texto.push_str(&format!("{}\r\n", entrada.nome));
            }
        }
        let enviado = conexao.write_all(texto.as_bytes());
        // Fechar a conexão é o que avisa o cliente de que a lista acabou.
        drop(conexao);
        match enviado {
            Ok(()) => self.dizer("226 transferencia completa"),
            Err(_) => self.dizer(INTERROMPIDA),
        }
    }

    /// `RETR`: manda o conteúdo de um arquivo.
    fn baixar(&mut self, arg: &str) -> Fluxo {
        if arg.is_empty() {
            return self.dizer("501 faltou o argumento");
        }
        if self.dados.is_none() {
            return self.dizer(SEM_PASV);
        }
        let caminho = match self.alvo(arg, false) {
            Ok(caminho) => caminho,
            Err(resposta) => return self.dizer(&resposta),
        };
        // Abrir e ler o primeiro pedaço antes do `150`: é aqui que um arquivo
        // inexistente, ou um diretório, vira erro sem abrir transferência.
        let mut arquivo = match File::open(&caminho) {
            Ok(arquivo) => arquivo,
            Err(erro) => return self.dizer(&resposta_de_erro(erro)),
        };
        let mut pedaco = [0u8; TAM_PEDACO];
        let mut lidos = match arquivo.read(&mut pedaco) {
            Ok(n) => n,
            Err(erro) => return self.dizer(&resposta_de_erro(erro)),
        };
        if self.dizer("150 abrindo a conexao de dados") == Fluxo::Encerra {
            return Fluxo::Encerra;
        }
        let conexao = match self.abrir_dados() {
            Ok(conexao) => conexao,
            Err(resposta) => return self.dizer(&resposta),
        };

        let mut total = 0;
        let resultado: Result<(), String> = loop {
            if lidos == 0 {
                break Ok(());
            }
            if conexao.write_all(&pedaco[..lidos]).is_err() {
                break Err(String::from(INTERROMPIDA));
            }
            total += lidos;
            lidos = match arquivo.read(&mut pedaco) {
                Ok(n) => n,
                Err(erro) => break Err(resposta_de_erro(erro)),
            };
        };
        // O arquivo e a conexão fecham antes da resposta: quem espera o `226`
        // já pode apagar o arquivo, e o cliente vê o fim dos dados.
        drop(arquivo);
        drop(conexao);
        match resultado {
            Ok(()) => {
                println!("ftpd: enviou {} ({} bytes)", caminho, total);
                self.dizer("226 transferencia completa")
            }
            Err(resposta) => self.dizer(&resposta),
        }
    }

    /// `STOR`: recebe os bytes de um arquivo novo.
    fn enviar(&mut self, arg: &str) -> Fluxo {
        if arg.is_empty() {
            return self.dizer("501 faltou o argumento");
        }
        if self.dados.is_none() {
            return self.dizer(SEM_PASV);
        }
        let caminho = match self.alvo(arg, true) {
            Ok(caminho) => caminho,
            Err(resposta) => return self.dizer(&resposta),
        };
        // Criar antes do `150`: nome repetido, nome fora de 8.3, pasta cheia ou
        // volume cheio viram erro sem transferência e sem arquivo parcial.
        let mut escritor = match FileWriter::create(&caminho) {
            Ok(escritor) => escritor,
            Err(erro) => return self.dizer(&resposta_de_erro(erro)),
        };
        if self.dizer("150 abrindo a conexao de dados") == Fluxo::Encerra {
            drop(escritor);
            let _ = remove(&caminho);
            return Fluxo::Encerra;
        }
        let conexao = match self.abrir_dados() {
            Ok(conexao) => conexao,
            Err(resposta) => {
                drop(escritor);
                let _ = remove(&caminho);
                return self.dizer(&resposta);
            }
        };

        let mut pedaco = [0u8; TAM_PEDACO];
        let mut total = 0;
        let resultado: Result<(), String> = loop {
            match conexao.read(&mut pedaco) {
                // O cliente fechou a conexão de dados: o arquivo está completo.
                Ok(0) => break Ok(()),
                Ok(n) => {
                    if let Err(erro) = escritor.write_all(&pedaco[..n]) {
                        break Err(resposta_de_erro(erro));
                    }
                    total += n;
                }
                Err(_) => break Err(String::from(INTERROMPIDA)),
            }
        };
        // Fechar o arquivo vem antes de apagá-lo: o kernel recusa apagar um
        // arquivo aberto.
        drop(escritor);
        drop(conexao);
        match resultado {
            Ok(()) => {
                println!("ftpd: recebeu {} ({} bytes)", caminho, total);
                self.dizer("226 transferencia completa")
            }
            Err(resposta) => {
                let _ = remove(&caminho);
                self.dizer(&resposta)
            }
        }
    }

    /// `SIZE`: o tamanho de um arquivo, achado na listagem do diretório pai.
    fn tamanho(&mut self, arg: &str) -> Fluxo {
        let caminho = match self.alvo(arg, false) {
            Ok(caminho) => caminho,
            Err(resposta) => return self.dizer(&resposta),
        };
        let (pai, nome) = match caminho.rsplit_once('/') {
            // `/ram` e `/disco` são diretórios.
            Some(("", _)) | None => {
                return self.dizer("550 tipo errado (arquivo ou diretorio)");
            }
            Some(partes) => partes,
        };
        let entradas = match listar_entradas(pai) {
            Ok(entradas) => entradas,
            Err(resposta) => return self.dizer(&resposta),
        };
        match entradas.iter().find(|e| e.nome.eq_ignore_ascii_case(nome)) {
            None => self.dizer(NAO_ENCONTRADO),
            Some(entrada) if entrada.diretorio => {
                self.dizer("550 tipo errado (arquivo ou diretorio)")
            }
            Some(entrada) => self.dizer(&format!("213 {}", entrada.tamanho)),
        }
    }

    /// `DELE`: apaga um arquivo.
    fn apagar(&mut self, arg: &str) -> Fluxo {
        let caminho = match self.alvo(arg, false) {
            Ok(caminho) => caminho,
            Err(resposta) => return self.dizer(&resposta),
        };
        match remove(&caminho) {
            Ok(()) => self.dizer("250 apagado"),
            Err(erro) => self.dizer(&resposta_de_erro(erro)),
        }
    }

    /// `MKD`: cria um diretório.
    fn criar_diretorio(&mut self, arg: &str) -> Fluxo {
        let caminho = match self.alvo(arg, true) {
            Ok(caminho) => caminho,
            Err(resposta) => return self.dizer(&resposta),
        };
        match mkdir(&caminho) {
            Ok(()) => self.dizer(&format!("257 \"{}\" criado", caminho)),
            Err(erro) => self.dizer(&resposta_de_erro(erro)),
        }
    }

    /// `RMD`: remove um diretório vazio.
    fn remover_diretorio(&mut self, arg: &str) -> Fluxo {
        let caminho = match self.alvo(arg, false) {
            Ok(caminho) => caminho,
            Err(resposta) => return self.dizer(&resposta),
        };
        match rmdir(&caminho) {
            Ok(()) => self.dizer("250 removido"),
            Err(erro) => self.dizer(&resposta_de_erro(erro)),
        }
    }

    /// O caminho de um comando que age sobre um arquivo ou diretório: exige o
    /// argumento e recusa a raiz virtual, onde não se lê nem se escreve. Quem
    /// cria (`cria`) algo direto na raiz, como `/x.txt`, recebe o aviso da raiz
    /// em vez de "não encontrado": a raiz existe, só não guarda nada.
    fn alvo(&self, arg: &str, cria: bool) -> Result<String, String> {
        if arg.is_empty() {
            return Err(String::from("501 faltou o argumento"));
        }
        let caminho = match resolver(&self.cwd, arg) {
            Ok(caminho) => caminho,
            Err(resposta) => {
                if cria && normalizar(&self.cwd, arg).len() == 1 {
                    return Err(String::from(RAIZ_SO_LISTA));
                }
                return Err(resposta);
            }
        };
        if caminho == "/" {
            return Err(String::from(RAIZ_SO_LISTA));
        }
        Ok(caminho)
    }
}

/// Uma entrada de diretório, já copiada para fora do kernel.
struct Entrada {
    nome: String,
    diretorio: bool,
    tamanho: u32,
}

/// As entradas de um diretório, na ordem dele. A raiz virtual `/` tem sempre
/// `ram` e `disco`, sem perguntar ao kernel (que não tem raiz única). Um
/// arquivo no lugar de um diretório dá `WrongType` na primeira leitura.
fn listar_entradas(caminho: &str) -> Result<Vec<Entrada>, String> {
    let mut entradas = Vec::new();
    if caminho == "/" {
        for nome in ["ram", "disco"] {
            entradas.push(Entrada { nome: String::from(nome), diretorio: true, tamanho: 0 });
        }
        return Ok(entradas);
    }
    let mut diretorio = Dir::open(caminho).map_err(resposta_de_erro)?;
    while let Some(entrada) = diretorio.next().map_err(resposta_de_erro)? {
        entradas.push(Entrada {
            nome: String::from(entrada.name()),
            diretorio: entrada.is_dir(),
            tamanho: entrada.size(),
        });
    }
    Ok(entradas)
}

/// Transforma o argumento de um comando num caminho que o kernel entende. O
/// kernel não aceita `.` nem `..` e não tem raiz única, então quem normaliza
/// é o `ftpd`: relativo ao diretório atual, sem componentes vazios nem `.`,
/// `..` sobe um nível (na raiz, fica na raiz) e tudo em minúsculas. O primeiro
/// componente tem de ser `ram` ou `disco`. Os limites de tamanho ficam com o
/// kernel.
fn resolver(cwd: &str, arg: &str) -> Result<String, String> {
    let partes = normalizar(cwd, arg);
    if let Some(primeiro) = partes.first() {
        if primeiro != "ram" && primeiro != "disco" {
            return Err(String::from(NAO_ENCONTRADO));
        }
    }
    let mut caminho = String::new();
    for parte in &partes {
        caminho.push('/');
        caminho.push_str(parte);
    }
    if caminho.is_empty() {
        caminho.push('/');
    }
    Ok(caminho)
}

/// Os componentes de `arg` (relativo a `cwd`, se não começar por `/`), sem os
/// vazios nem `.`, com `..` subindo um nível e tudo em minúsculas.
fn normalizar(cwd: &str, arg: &str) -> Vec<String> {
    let base = if arg.starts_with('/') { "" } else { cwd };
    let mut partes: Vec<String> = Vec::new();
    for trecho in base.split('/').chain(arg.split('/')) {
        match trecho {
            "" | "." => {}
            ".." => {
                partes.pop();
            }
            outro => partes.push(outro.to_ascii_lowercase()),
        }
    }
    partes
}

/// A resposta FTP de um erro de arquivo: código `550`, `552`, `553` ou `451`
/// e o texto do próprio erro.
fn resposta_de_erro(erro: FsError) -> String {
    let codigo = match erro {
        FsError::NotFound
        | FsError::NoVolume
        | FsError::WrongType
        | FsError::Exists
        | FsError::ReadOnly
        | FsError::Busy
        | FsError::NotEmpty => 550,
        FsError::Invalid | FsError::PathTooLong => 553,
        FsError::NoSpace => 552,
        FsError::TooManyOpen | FsError::BadDescriptor | FsError::Io | FsError::Fault => 451,
    };
    format!("{} {}", codigo, erro)
}
