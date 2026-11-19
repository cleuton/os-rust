fn t(n: i64) -> bool {
    println!("t({})", n);
    true
}

fn f(n: i64) -> bool {
    println!("f({})", n);
    false
}

fn mostra(n: i64) -> i64 {
    println!("avalia {}", n);
    n
}

fn main() {
    if f(1) && t(2) {
        println!("a");
    }
    if t(3) || f(4) {
        println!("b");
    }
    if f(5) || t(6) {
        println!("c");
    }
    if t(7) && t(8) {
        println!("d");
    }
    if !f(9) {
        println!("e");
    }
    println!("{} {} {}", mostra(1), mostra(2), mostra(3));
}
