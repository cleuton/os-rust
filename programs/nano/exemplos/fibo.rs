fn fib(n: i64) -> i64 {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn main() {
    let mut i: i64 = 0;
    while i <= 10 {
        println!("fib({}) = {}", i, fib(i));
        i = i + 1;
    }
}
