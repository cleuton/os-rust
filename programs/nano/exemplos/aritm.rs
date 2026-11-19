fn main() {
    let a: i64 = 17;
    let b: i64 = 5;
    println!("{} {} {}", a + b, a - b, a * b);
    println!("{} {}", a / b, a % b);
    println!("{} {}", -a / b, -a % b);
    let p: i64 = 2 + 3 * 4;
    let q: i64 = (2 + 3) * 4;
    let r: i64 = 100 - 30 - 20;
    println!("{} {} {}", p, q, r);
    let menor: i64 = -9223372036854775807 - 1;
    println!("{}", menor);
    let maximo: i64 = 9223372036854775807;
    println!("{}", maximo);
    if a > b && !(a == b) || false {
        println!("logica ok");
    }
    if a <= b {
        println!("errado");
    } else {
        println!("a > b");
    }
    if a != b {
        println!("a != b");
    }
    if a >= 17 {
        println!("a >= 17");
    }
    if b < a {
        println!("b < a");
    }
}
