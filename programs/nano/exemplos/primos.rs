fn main() {
    let mut n: i64 = 2;
    while n <= 50 {
        let mut d: i64 = 2;
        let mut primo: bool = true;
        while d * d <= n {
            if n % d == 0 {
                primo = false;
            }
            d = d + 1;
        }
        if primo {
            println!("{}", n);
        }
        n = n + 1;
    }
}
