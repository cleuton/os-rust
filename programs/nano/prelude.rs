#![allow(dead_code)]

// A funcao predefinida `ler_i64` do nanorust, para o `rustc` conferir os
// exemplos no computador. Le uma linha da entrada padrao (sem o `\n`); se ela
// casa com `-?[0-9]{1,18}` devolve o valor, senao devolve 0 (o fim da entrada
// tambem da 0). Tudo fica dentro da funcao para nenhum nome colidir com o do
// fonte de quem usa.
fn ler_i64() -> i64 {
    let mut linha = String::new();
    if std::io::stdin().read_line(&mut linha).is_err() {
        return 0;
    }
    let bytes = linha.strip_suffix('\n').unwrap_or(&linha).as_bytes();
    let (negativo, digitos) = match bytes.first() {
        Some(b'-') => (true, &bytes[1..]),
        _ => (false, bytes),
    };
    if digitos.is_empty() || digitos.len() > 18 || !digitos.iter().all(|c| c.is_ascii_digit()) {
        return 0;
    }
    let mut valor: i64 = 0;
    for c in digitos {
        valor = valor * 10 + i64::from(*c - b'0');
    }
    if negativo {
        -valor
    } else {
        valor
    }
}
