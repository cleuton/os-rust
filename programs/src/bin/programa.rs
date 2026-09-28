#![no_std]
#![no_main]

use runtime::{entry, println};

entry!(main);

fn main() -> i32 {
    println!("ola, os-rust!");
    0
}