# Programmer's Guide - os-rust

This guide is for anyone who wants to write a program that runs inside os-rust, without needing to understand the kernel internals. If you want to know how boot, memory, or interrupts work, the right document is `WALKTHROUGH.md`. This one assumes you just want to write code that runs in user mode, using the project's runtime library.

## Prerequisites

The same three as in `README.md`: Rust with the nightly toolchain pinned by the project (`rust-toolchain.toml`), `bootimage`, and QEMU. If you already run `cargo run` successfully at the repository root, you are ready to follow this guide. To create programs **outside** the repository and bring them into os-rust over FTP, see the "Programs outside the repository" section.

## How a user program is organized

A user program is not compiled together with the kernel. It is a `no_std` Rust file, compiled for a user-mode target (different from the kernel's bare-metal target), and the resulting executable is embedded in the boot image at build time. This happens automatically inside `cargo run`/`cargo test`: you do not need to run any separate build command.

Each program is **one file** in `programs/src/bin/`, for example `programs/src/bin/hello.rs`. The file name, without `.rs`, is the name you type after `run`. There is no per-program `Cargo.toml`: they all share the one in `programs/`, which already depends on the runtime library.

## The runtime library

The runtime library (`runtime/`, at the repository root) exists so that you do not have to call syscalls by hand. It provides:

- `entry!(main)`: wires your `main` function to the program's entry point (`_start`). When `main` returns, the library calls `exit` with the returned value.
- `print!` and `println!`: write text to the screen, through the write syscall. Each call becomes **one** write (for texts up to 256 bytes), so a line is never split by the output of another program.
- `read_line(&mut buffer)`: waits for the user to type a line and press Enter, and returns what was typed (without the Enter) as text, through the keyboard read syscall. The system echoes what is typed to the screen and erases with Backspace; the line is at most 127 characters long.
- A global allocator: lets you use `Box`, `Vec`, `String`, and the rest of the `alloc` crate (with `extern crate alloc;` in your program), through the memory syscall. The program's heap grows up to 1 MiB.
- `yield_now()`: yields the CPU to another program that is running at the same time (see the **Multitasking** section).
- `File`, `Dir`, `DirEntry`, and `FsError`: open and read files and list directories (see the **Files** section). `File` and `Dir` only read.
- `FileWriter`, `remove`, `mkdir` and `rmdir`: create a file, append bytes to its end, delete it, and create and remove directories, on the `/ram` and `/disco` volumes (see the **Writing files** section).
- `time::now()`: the current date and time, in UTC (see the **The time** section).
- `exit(codigo)`: ends the program early, if you need to.
- A `panic!` handler: writes `[panic] <message>` to the screen and ends the program with code 101.

With that, the only low-level code you need to write is your own logic: read, process, write.

## Minimal structure of a program

```rust
#![no_std]
#![no_main]

use runtime::{entry, println};

entry!(main);

fn main() -> i32 {
    println!("ola, os-rust!");
    0
}
```

Note four things:

1. `#![no_std]` and `#![no_main]`: as in the kernel, a user program has neither the standard library nor a conventional `main` underneath.
2. `entry!(main);` generates the entry point and wires it to your function. The compiler checks that `main` is `fn() -> i32`.
3. The value that `main` returns becomes the program's exit code: `0` means success and prints nothing; any other value makes the system write `[run] <name> terminou com codigo <n>`.
4. `println!` comes from the runtime library (`runtime::println`), not from the standard library.

## The syscall contract, summarized

The official, versioned document for the syscall contract is [`SYSCALLS.md`](SYSCALLS.md), at the repository root. It is the source of truth for numbers, register convention, error codes, and limits. This guide only summarizes what each syscall does, because in practice you will use the runtime library, not the syscall directly:

| No. | Syscall | What it does | How you use it |
|---|---|---|---|
| 1 | `SYS_WRITE` | Writes text to the screen | `print!`/`println!` |
| 2 | `SYS_EXIT` | Ends the program and returns an exit code | the return of `main`, or `exit(codigo)` |
| 3 | `SYS_READ_LINE` | Waits for a typed line, up to Enter | `read_line(&mut buffer)` |
| 4 | `SYS_ALLOC` | Grows the program's heap | the global allocator, used by `Box`/`Vec`/`String` |
| 5 | `SYS_YIELD` | Yields the CPU to another program | `yield_now()` |
| 6 | `SYS_OPEN` | Opens a file or directory for reading | `File::open`, `Dir::open` |
| 7 | `SYS_READ` | Reads bytes from an open file | `File::read` |
| 8 | `SYS_CLOSE` | Closes a file or directory | automatic, when the `File`/`Dir` goes out of scope |
| 9 | `SYS_READ_DIR` | Reads the next entry of an open directory | `Dir::next` |
| 11 | `SYS_CREATE` | Creates an empty file and opens it for appending | `FileWriter::create` |
| 12 | `SYS_OPEN_APPEND` | Opens an existing file for appending | `FileWriter::append` |
| 13 | `SYS_WRITE_FILE` | Writes bytes at the end of the file | `FileWriter::write`, `FileWriter::write_all` |
| 14 | `SYS_REMOVE` | Deletes a file | `remove` |

If your program needs to call a syscall directly (beyond what the runtime library already covers), consult `SYSCALLS.md` for the exact number and convention.

## Example program 1: eco

Reads a line from the keyboard, writes the same text back, and counts how many words it has. It demonstrates the complete input and output cycle of a user program, and the use of `Vec` through the library's allocator. File: `programs/src/bin/eco.rs`.

```rust
//! `eco`: reads a line from the keyboard and replies.
//!
//! Shows the complete cycle of a user program: writing (`print!`),
//! reading from the keyboard (`read_line`) and using dynamic memory (`Vec`), all
//! through kernel syscalls, via the runtime library. The program never
//! touches the keyboard hardware or the screen: it only asks the kernel.

#![no_std]
#![no_main]

// The global allocator comes from the runtime library; `extern crate alloc`
// gives access to `Vec`, `Box` and `String`.
extern crate alloc;

use alloc::vec::Vec;
use runtime::{entry, print, println, read_line};

entry!(main);

fn main() -> i32 {
    print!("digite algo: ");

    // The buffer belongs to the program. `read_line` writes the typed line
    // into it and returns the text, without the Enter. Up to 127 characters fit.
    let mut buffer = [0u8; 128];
    let texto = read_line(&mut buffer);

    // The reply: exactly what was typed.
    println!("voce digitou: {}", texto);

    // `collect` creates a `Vec`: the first allocation makes the library ask
    // the kernel for memory (the `SYS_ALLOC` syscall).
    let palavras: Vec<&str> = texto.split_whitespace().collect();
    println!("palavras: {}", palavras.len());

    0
}
```

Running:

```text
os-rust> run eco
digite algo: Ola mundo
voce digitou: Ola mundo
palavras: 2
os-rust>
```

## Example program 2: falha_memoria

Deliberately accesses a memory address that does not belong to the program, to demonstrate that the kernel isolates the fault: only the program is terminated, the kernel stays up. File: `programs/src/bin/falha_memoria.rs`.

```rust
//! `falha_memoria`: a program that errs on purpose.
//!
//! Writes to a memory address that is not its own. The processor raises a
//! page fault exception (`#PF`); the kernel handles it, ends only
//! this program, shows a readable message and returns control to the
//! prompt. A fault in a user program never brings down the kernel
//! (`SYSCALLS.md`, section 7).

#![no_std]
#![no_main]

use runtime::{entry, println};

entry!(main);

fn main() -> i32 {
    println!("prestes a acessar memoria invalida...");

    // SAFETY: this is **intentionally** unsafe. `0xdead_beef` is outside
    // this program's memory, so the write becomes a CPU exception
    // (`#PF`), handled by the kernel: it is exactly the behavior this
    // program exists to show.
    unsafe {
        core::ptr::write_volatile(0xdead_beef as *mut u8, 42);
    }

    // This line is never reached.
    println!("se voce esta vendo isso, o isolamento falhou");
    0
}
```

Running:

```text
os-rust> run falha_memoria
prestes a acessar memoria invalida...
[run] falha_memoria encerrado por erro de memoria: #PF (Page Fault) em 0x40000015
codigo de erro: 0x6
endereco de falha: 0xdeadbeef
os-rust>
```

The address after `em` changes from one build to another; the `endereco de falha` is what the program tried to access. Right after, the prompt keeps working: `run hello` and `run eco` run normally.

## Multitasking

So far the examples have run one at a time. os-rust also runs **several programs at the same time**: `run <name> [<name>...]` loads all the requested programs (up to 4) before starting any of them, and only returns the prompt when the last one finishes. The same name may be repeated (`run ping ping` runs two independent copies). If a name does not exist, or if more than 4 are requested, no program is started: the prompt states the problem and lists the available programs.

### What changes for your program

- **Memory is yours alone.** Each program sees the same address map from `SYSCALLS.md`, but nobody else sees or changes its pages, and it does not see those of others. Two copies of the same program do not even share variables.
- **You do not notice the switches.** The kernel saves the entire state of your program (registers, stack, memory) and resumes it exactly where it stopped. But it can be interrupted at **any** instruction: do not assume any execution order between programs, nor any timing.
- **The CPU is shared.** A timer interrupts a program that computes for too long (40 to 50 ms in a row) and hands the CPU to the next one. So a program that only computes, without asking for anything, does not freeze the others.
- **Each `print!`/`println!` comes out whole.** A line written by one call is not split by the output of another program (up to 256 bytes per call).
- **Waiting for the keyboard does not use CPU.** While one program waits in `read_line`, the others keep running. If more than one is waiting, the typed line goes to the one that asked **first**; no key goes to two programs or gets lost.
- **A fault only takes down the one that failed.** If a program accesses invalid memory, only it is terminated, with the usual message, at the moment it happens; the others continue, and the prompt returns when the last one finishes.

### `yield_now`: yielding the CPU on purpose

`yield_now()` tells the kernel: "you can pass the turn". The kernel saves the program's state, runs the next ready program, and only comes back to yours when its turn arrives, at the next line. If no other program is ready, it returns immediately. You do not need to call `yield_now` for the other programs to run (the timer takes care of that); calling it serves to make two programs **alternate** predictably. The call never fails and returns nothing.

### Example program 3: ping

Writes a line and yields the CPU to the other program, four times. File: `programs/src/bin/ping.rs`.

```rust
//! `ping`: the first of two programs that alternate on the screen.
//!
//! Writes a line and **yields the CPU** (`yield_now`) to the other program; when
//! its turn comes again, it writes the next one. Run `run ping pong` and the
//! lines of `ping` and `pong` appear interleaved, in order. Each
//! program has its own memory: the counter `i` of one is not seen by the other.

#![no_std]
#![no_main]

use runtime::{entry, println, yield_now};

entry!(main);

fn main() -> i32 {
    for i in 1..=4 {
        // Asks the kernel (`write` syscall) to write the line to the screen. The
        // line arrives whole, even with another program running.
        println!("ping {}", i);

        // Asks the kernel (`yield` syscall) to pass the CPU to the next
        // ready program. It only returns here when this one's turn comes again.
        yield_now();
    }
    0
}
```

### Example program 4: pong

The second of the pair: the same as `ping`, with different text. File: `programs/src/bin/pong.rs`.

```rust
//! `pong`: the second of two programs that alternate on the screen.
//!
//! It is the same as `ping`, with different text. Run `run ping pong` and the lines of
//! both appear interleaved, in order: each one writes a line and **yields the
//! CPU** (`yield_now`) to the other. Each program has its own memory: the
//! counter `i` of one is not seen by the other.

#![no_std]
#![no_main]

use runtime::{entry, println, yield_now};

entry!(main);

fn main() -> i32 {
    for i in 1..=4 {
        // Asks the kernel (`write` syscall) to write the line to the screen. The
        // line arrives whole, even with another program running.
        println!("pong {}", i);

        // Asks the kernel (`yield` syscall) to pass the CPU to the next
        // ready program. It only returns here when this one's turn comes again.
        yield_now();
    }
    0
}
```

Run the two together:

```text
os-rust> run ping pong
ping 1
pong 1
ping 2
pong 2
ping 3
pong 3
ping 4
pong 4
os-rust>
```

Each writes a line and yields the CPU to the other: the lines appear alternating.

### Example program 5: contador_a

Counts in a long loop and **never yields the CPU**. File: `programs/src/bin/contador_a.rs`.

```rust
//! `contador_a`: counts in a long loop, **never yielding the CPU**.
//!
//! This program asks nobody for a turn: it just counts, as if it were the only
//! program in the system. Even so, running next to `contador_b`
//! (`run contador_a contador_b`), the lines of both appear interleaved on the
//! screen, because the kernel's timer interrupts each program after a
//! time slice and hands the CPU to the other. The kernel is in charge of the CPU, not the
//! program.

#![no_std]
#![no_main]

use core::hint::black_box;
use runtime::{entry, println};

entry!(main);

/// How many iterations the program counts between one line and the next. It is large
/// on purpose: it takes several timer time slices, so the lines of the
/// two programs only alternate if the kernel interrupts them.
const PASSO: u64 = 20_000_000;

/// How many lines the program writes. Only a few: the screen has 25 lines and the
/// tests read the screen, so the output of both programs has to fit on it.
const LINHAS: u64 = 8;

fn main() -> i32 {
    let mut contador: u64 = 0;
    for linha in 1..=LINHAS {
        for _ in 0..PASSO {
            // `black_box` hides the value from the compiler: without it, the loop would be
            // eliminated and the program would finish immediately.
            contador = black_box(contador).wrapping_add(1);
        }
        // Asks the kernel (`write` syscall) to write the line to the screen.
        println!("A: {}", linha);
    }
    0
}
```

### Example program 6: contador_b

`contador_a`'s sibling, writing `B:` instead of `A:`. File: `programs/src/bin/contador_b.rs`.

```rust
//! `contador_b`: counts in a long loop, **never yielding the CPU**.
//!
//! This program asks nobody for a turn: it just counts, as if it were the only
//! program in the system. Even so, running next to `contador_a`
//! (`run contador_a contador_b`), the lines of both appear interleaved on the
//! screen, because the kernel's timer interrupts each program after a
//! time slice and hands the CPU to the other. The kernel is in charge of the CPU, not the
//! program.

#![no_std]
#![no_main]

use core::hint::black_box;
use runtime::{entry, println};

entry!(main);

/// How many iterations the program counts between one line and the next. It is large
/// on purpose: it takes several timer time slices, so the lines of the
/// two programs only alternate if the kernel interrupts them.
const PASSO: u64 = 20_000_000;

/// How many lines the program writes. Only a few: the screen has 25 lines and the
/// tests read the screen, so the output of both programs has to fit on it.
const LINHAS: u64 = 8;

fn main() -> i32 {
    let mut contador: u64 = 0;
    for linha in 1..=LINHAS {
        for _ in 0..PASSO {
            // `black_box` hides the value from the compiler: without it, the loop would be
            // eliminated and the program would finish immediately.
            contador = black_box(contador).wrapping_add(1);
        }
        // Asks the kernel (`write` syscall) to write the line to the screen.
        println!("B: {}", linha);
    }
    0
}
```

Run the two together:

```text
os-rust> run contador_a contador_b
B: 1
A: 1
B: 2
A: 2
...
```

The lines of both appear interleaved, even though neither asks for a turn: the kernel is in charge of the CPU, and it interrupts each program after a time slice. The exact order of the lines may vary from one run to another.

### Example program 7: eco2

`eco`'s sibling, with the `eco2:` prefix in the reply, to run next to it. File: `programs/src/bin/eco2.rs`.

```rust
//! `eco2`: `eco`'s sibling, to run next to it.
//!
//! Does the same as `eco` (reads a line from the keyboard and replies), but puts
//! the prefix `eco2:` on the reply. Run `run eco eco2`, type a line and Enter, and
//! then another: the keyboard goes to the program that **asked first** (`eco`), and the
//! second line goes to the other. Each reply shows who received which line.
//! While a program waits for the keyboard it does not use CPU: the kernel passes the
//! turn to the other.

#![no_std]
#![no_main]

// The global allocator comes from the runtime library; `extern crate alloc`
// gives access to `Vec`, `Box` and `String`.
extern crate alloc;

use alloc::vec::Vec;
use runtime::{entry, print, println, read_line};

entry!(main);

fn main() -> i32 {
    print!("eco2: digite algo: ");

    // The buffer belongs to the program. `read_line` writes the typed line
    // into it and returns the text, without the Enter. Up to 127 characters fit.
    let mut buffer = [0u8; 128];
    let texto = read_line(&mut buffer);

    // The reply: exactly what was typed, with the program's prefix.
    println!("eco2: voce digitou: {}", texto);

    // `collect` creates a `Vec`: the first allocation makes the library ask
    // the kernel for memory (the `SYS_ALLOC` syscall).
    let palavras: Vec<&str> = texto.split_whitespace().collect();
    println!("eco2: palavras: {}", palavras.len());

    0
}
```

Run the two together, type a line and Enter, and then another:

```text
os-rust> run eco eco2
digite algo: eco2: digite algo: um dois
voce digitou: um dois
palavras: 2
tres
eco2: voce digitou: tres
eco2: palavras: 1
os-rust>
```

The first line goes to `eco`, which asked first; the second goes to `eco2`.

### Writing a program that cooperates

1. Create `programs/src/bin/meuprograma.rs` as in the previous section and import `yield_now` along with the rest: `use runtime::{entry, println, yield_now};`.
2. Write a loop that does a bit of work, writes a line with `println!`, and calls `yield_now()`.
3. Run `cargo run` and, at the prompt, `run ping meuprograma`: your program's lines alternate with those of `ping`.

## Files

os-rust has files on two volumes (both accept writes, see **Writing files**). This section shows how to **read**. Files live on two volumes, both in FAT16 format, and your program sees them through the same path, without knowing where the bytes come from:

| Volume | What it is |
|---|---|
| `/ram` | the ramdisk, embedded in the boot image |
| `/disco` | the disk, attached to QEMU by `cargo run` (if QEMU starts without it, `/disco` is unavailable and `/ram` keeps working) |

The contents of both come from the repository's `discos/` directory (`discos/ram/` and `discos/disco/`), turned into a volume image by `build.rs` (every time it runs): to give your program a new file, put it there.

### Paths

A path is `/<volume>/<component>/<component>...`, for example `/disco/docs/longo.txt`.

- Each component is an **8.3** name: up to 8 characters, and optionally a dot and up to 3 of extension (letters, digits, `_`, `-`, `~`, `$`). Long names do not exist.
- Upper and lower case make no difference: `/DISCO/DOCS/LONGO.TXT` is the same file. The names the kernel returns come in lowercase.
- `.` and `..` are not accepted: no path leaves the volume.
- At most 64 bytes and 8 components per path, and at most 4 files open at the same time per program.

### The API

```rust
use runtime::{Dir, DirEntry, File, FsError};

let mut arquivo = File::open("/disco/docs/longo.txt")?;   // Result<File, FsError>
let n = arquivo.read(&mut buffer)?;                       // Result<usize, FsError>; 0 = finished

let mut diretorio = Dir::open("/disco")?;                 // Result<Dir, FsError>
while let Some(entrada) = diretorio.next()? {             // Result<Option<DirEntry>, FsError>
    entrada.name();    // &str, e.g.: "leiame.txt"
    entrada.is_dir();  // bool
    entrada.size();    // u32, in bytes (0 for a directory)
}
```

- `File::read` reads **from where the previous read stopped** and advances. It returns how many bytes it read; it only returns `0` when the file is finished. An empty file returns `0` on the first read.
- `Dir::next` hands out one entry at a time, in directory order, without `.` or `..`.
- `File` and `Dir` **close themselves** when they go out of scope. If your program ends (through `exit`, or because of an error) with open files, the kernel closes them.
- Each program has **its own** list of open files: one program's read position is never another's, not even when two programs read the same file at the same time.

### The errors

Every operation returns `Result<_, FsError>`. `FsError` implements `Display`, so `println!("{}", erro)` writes a sentence.

| `FsError` | When |
|---|---|
| `NotFound` | the path (or a component of it) does not exist |
| `NoVolume` | the volume does not exist or is not available (for example, QEMU without a disk, or a faulty volume) |
| `WrongType` | reading a directory as a file, listing a file, or a file in the middle of the path |
| `TooManyOpen` | the program already has 4 open files |
| `PathTooLong` | more than 64 bytes or 8 components |
| `Invalid` | malformed path: empty component, name outside 8.3, `.` or `..`, or no `/` at the start |
| `Io` | error reading the volume: the disk did not respond or the volume structure is corrupted |
| `Exists` | `FileWriter::create` on a name that already exists |
| `NoSpace` | the volume (or directory) has no space left |
| `ReadOnly` | the volume does not accept writes (only happens with a volume of more than 1 sector per cluster) |
| `Busy` | `remove` of a file, or `rmdir` of a directory, that some program has open; `rmdir` of a volume root |
| `NotEmpty` | `rmdir` of a directory that still has entries |

A file error **never** brings down your program or the kernel: you receive the `Err` and decide what to do.

### Example program 8: leitor

Opens a file on the disk and writes its contents to the screen. The file spans more than one disk cluster, and the program neither knows nor needs to know that. File: `programs/src/bin/leitor.rs`. Run with `run leitor`.

```rust
//! `leitor`: opens a file on the disk, reads its contents and writes them to the screen.
//!
//! The program never talks to the disk: it asks the kernel, through syscalls, and the
//! runtime library hides the syscalls behind `File`. The file is
//! read in 128-byte chunks (a stack buffer) until `read` returns `0`,
//! which means "the file is finished". The path is fixed because programs do not
//! receive command-line arguments in this milestone.
//!
//! Run with `run leitor`. To read another file, change `CAMINHO`.

#![no_std]
#![no_main]

use runtime::sys::write;
use runtime::{entry, println, File};

entry!(main);

/// The file the program reads: a two-cluster text, on the disk.
const CAMINHO: &str = "/disco/docs/longo.txt";

fn main() -> i32 {
    // `File::open` returns a Rust error if the file does not exist, if the disk
    // is not there, etc. The file is closed automatically when `arquivo` goes out of scope.
    let mut arquivo = match File::open(CAMINHO) {
        Ok(arquivo) => arquivo,
        Err(erro) => {
            println!("leitor: nao abriu {}: {}", CAMINHO, erro);
            return 1;
        }
    };

    let mut pedaco = [0u8; 128];
    loop {
        match arquivo.read(&mut pedaco) {
            // 0 bytes: the file is finished.
            Ok(0) => return 0,
            // `write` hands the bytes to the screen exactly as they are.
            Ok(n) => {
                write(&pedaco[..n]);
            }
            Err(erro) => {
                println!("leitor: erro de leitura: {}", erro);
                return 2;
            }
        }
    }
}
```

Note that `Err(erro)` appears in two places: when opening (the file may not exist, or the disk may not be there) and when reading. The program warns and exits with a non-zero code, and the system writes `[run] leitor terminou com codigo <n>`.

### Example program 9: listador

Lists a directory on the disk. File: `programs/src/bin/listador.rs`. Run with `run listador`.

```rust
//! `listador`: lists a directory on the disk and writes the name of each entry.
//!
//! A directory is opened like a file, with `Dir::open`, and read one entry
//! at a time with `next`, until it returns `None`. Each entry gives the name, whether it is
//! a directory, and the size. The path is fixed because programs do not receive
//! command-line arguments in this milestone.
//!
//! Run with `run listador`. To list another directory, change `CAMINHO`.

#![no_std]
#![no_main]

use runtime::{entry, println, Dir};

entry!(main);

/// The directory the program lists: the root of the disk.
const CAMINHO: &str = "/disco";

fn main() -> i32 {
    let mut diretorio = match Dir::open(CAMINHO) {
        Ok(diretorio) => diretorio,
        Err(erro) => {
            println!("listador: nao abriu {}: {}", CAMINHO, erro);
            return 1;
        }
    };

    loop {
        match diretorio.next() {
            Ok(Some(entrada)) => {
                let tipo = if entrada.is_dir() { "dir" } else { "arquivo" };
                println!("{} {} {}", tipo, entrada.size(), entrada.name());
            }
            // No more entries.
            Ok(None) => return 0,
            Err(erro) => {
                println!("listador: erro de leitura: {}", erro);
                return 2;
            }
        }
    }
}
```

### Running a program that is on the disk

`run` also accepts the path of an executable file: an argument that starts with `/` is a path, and any other is the name of an embedded program. Try `run /disco/bin/visita`: `visita` exists **only** on the disk (`programs/src/disco/visita.rs`), the kernel never saw it at build time, and yet it runs. The file must be a valid static ELF64 and at most 64 KiB; the two kinds of target can be mixed (`run /disco/bin/visita hello`).

### Writing a program that reads files

1. Put the file you want to read in `discos/disco/` (for example, `discos/disco/notas.txt`; remember the 8.3 name).
2. Create `programs/src/bin/meuleitor.rs` by copying `leitor` and change the `CAMINHO` constant to `"/disco/notas.txt"`.
3. Run `cargo run` and, at the prompt, `run meuleitor`.
4. To just check the file without writing a program, the prompt has `ls <path>` and `cat <path>` (for example, `ls /disco`, `cat /disco/notas.txt`).

## Writing files

The `/ram` and `/disco` volumes accept writes. Writing is simple on purpose: **a file is born empty and only grows at the end; deleting gives the space back; nothing is kept for later** (every `write` already leaves the data on the volume, there is no `flush`). To replace the contents of a file, delete it and create it again.

```rust
use runtime::{mkdir, remove, rmdir, FileWriter, FsError};

let mut arquivo = FileWriter::create("/ram/nota.txt")?;  // Result<FileWriter, FsError>; Exists if the name exists
arquivo.write_all(b"ola, ramdisk\n")?;                   // writes everything, repeating write until done
let n = arquivo.write(b"mais")?;                         // Result<usize, FsError>; may write less than asked

let mut mesmo = FileWriter::append("/ram/nota.txt")?;    // opens an existing file, for appending
remove("/ram/nota.txt")?;                                // Result<(), FsError>
mkdir("/ram/docs/novo")?;                               // creates an empty directory; Exists if the name exists
rmdir("/ram/docs/novo")?;                               // removes an empty directory; NotEmpty if it has entries
```

The rules:

- **Only at the end.** There is no seeking, overwriting in the middle or truncating. `FileWriter::create` fails with `Exists` if the name exists: nothing is overwritten by accident.
- **An open file is not deleted.** `remove` of a file that *any* program has open (including yours) returns `Busy`. Close it first: let the `File` or `FileWriter` go out of scope (`drop(arquivo)`).
- **Full volume.** `write` returns how many bytes fit; the next call returns `NoSpace`. What fit stays written.
- **A reader sees the size from when it opened.** A `File` opened before a `write` does not see what was appended later; a new open sees everything.
- **Directories.** `mkdir` creates an empty directory (the parent must exist; it is not recursive) and `rmdir` removes an **empty** directory that nobody has open. A new directory takes one cluster and does not grow: it holds 14 entries (no free slot, `NoSpace`).
- **What persists.** Rebooting returns `/ram` to its original contents. `/disco` keeps what you write **to the image QEMU is running on**: with `cargo run` (`snapshot=on`) that is discarded when QEMU closes. For a write to survive a restart, run QEMU by hand over a copy of the disk image (README, "Keeping the disk between boots"). Nothing is protected: any file, or empty directory, can be deleted.
- The prompt has the commands `grava <path> [<text>]`, `anexa <path> <text>`, `apaga <path>`, `mkdir <path>` and `rmdir <path>` to do the same by hand.

### Example program 10: gravador

Creates `/ram/gravado.txt` with 20 lines (680 bytes); if the file already exists, it appends one line. File: `programs/src/bin/gravador.rs`. Run with `run gravador` (creates), again (extends), and check with `cat /ram/gravado.txt`. (The comments in the source are in Portuguese, the project's language.)

```rust
//! `gravador`: cria um arquivo no ramdisk e, na vez seguinte, o amplia.
//!
//! Um programa nunca fala com o disco: ele pede ao kernel, por syscalls, e a
//! biblioteca de runtime esconde as syscalls atrás de `FileWriter`, que fecha o
//! descritor sozinho quando sai de escopo. Escrever é sempre **pelo fim**: um
//! arquivo nasce vazio e só cresce acrescentando bytes. Por isso o programa
//! tenta criar primeiro; se o nome já existe (`Exists`), abre o arquivo para
//! acrescentar em vez de sobrescrevê-lo.
//!
//! O `/ram` volta ao conteúdo original a cada boot (o `/disco` também aceita
//! escrita, mas só persiste se o QEMU roda sem `snapshot=on`). O caminho é fixo
//! porque programas não recebem argumentos de linha de comando.
//!
//! Rode com `run gravador` (cria), de novo (amplia) e `run apagador` (remove).

#![no_std]
#![no_main]

use runtime::{entry, println, FileWriter, FsError};

entry!(main);

/// O arquivo que o programa cria e amplia.
const CAMINHO: &str = "/ram/gravado.txt";

fn main() -> i32 {
    match FileWriter::create(CAMINHO) {
        Ok(mut arquivo) => {
            if let Err(erro) = grava_linhas(&mut arquivo) {
                println!("gravador: nao gravou {}: {}", CAMINHO, erro);
                return 1;
            }
            println!("gravador: criei {} (680 bytes)", CAMINHO);
            0
        }
        // O nome já existe: acrescenta ao fim, sem tocar no que já está lá.
        Err(FsError::Exists) => {
            let resultado = FileWriter::append(CAMINHO)
                .and_then(|mut arquivo| arquivo.write_all(b"mais uma linha do gravador\n"));
            match resultado {
                Ok(()) => {
                    println!("gravador: ampliei {} (+27 bytes)", CAMINHO);
                    0
                }
                Err(erro) => {
                    println!("gravador: nao gravou {}: {}", CAMINHO, erro);
                    1
                }
            }
        }
        Err(erro) => {
            println!("gravador: nao gravou {}: {}", CAMINHO, erro);
            1
        }
    }
}

/// Grava as 20 linhas `linha NN: gravada por um programa`, cada uma com 34
/// bytes (33 de texto e a quebra de linha), 680 no total. A linha é montada
/// num buffer da pilha: o programa não precisa de heap.
fn grava_linhas(arquivo: &mut FileWriter) -> Result<(), FsError> {
    for numero in 1..=20u8 {
        let mut linha = *b"linha NN: gravada por um programa\n";
        linha[6] = b'0' + numero / 10;
        linha[7] = b'0' + numero % 10;
        arquivo.write_all(&linha)?;
    }
    Ok(())
}
```

### Example program 11: apagador

Writes to the screen the file that `gravador` created, closes it and deletes it. File: `programs/src/bin/apagador.rs`. Run with `run apagador`.

```rust
//! `apagador`: mostra o arquivo que o `gravador` criou e o apaga.
//!
//! Primeiro lê o conteúdo (em pedaços de 128 bytes, como o `leitor`) e o
//! escreve na tela, o que também prova que um programa lê o que outro gravou.
//! Depois remove o arquivo com `remove`.
//!
//! Se o arquivo não existe, o programa avisa e termina com código 1.
//!
//! Rode com `run gravador` antes, e depois `run apagador`.

#![no_std]
#![no_main]

use runtime::sys::write;
use runtime::{entry, println, remove, File, FsError};

entry!(main);

/// O arquivo que o `gravador` cria.
const CAMINHO: &str = "/ram/gravado.txt";

fn main() -> i32 {
    let mut arquivo = match File::open(CAMINHO) {
        Ok(arquivo) => arquivo,
        Err(FsError::NotFound) => {
            println!("apagador: nao ha {} (rode gravador antes)", CAMINHO);
            return 1;
        }
        Err(erro) => {
            println!("apagador: nao abriu {}: {}", CAMINHO, erro);
            return 2;
        }
    };

    let mut pedaco = [0u8; 128];
    loop {
        match arquivo.read(&mut pedaco) {
            // 0 bytes: o arquivo acabou.
            Ok(0) => break,
            Ok(n) => {
                write(&pedaco[..n]);
            }
            Err(erro) => {
                println!("apagador: erro de leitura: {}", erro);
                return 2;
            }
        }
    }

    // O kernel não apaga um arquivo aberto (um leitor poderia passar a ler
    // clusters reaproveitados): `remove` daria `Busy`. Por isso o arquivo é
    // fechado antes, soltando-o de escopo.
    drop(arquivo);

    match remove(CAMINHO) {
        Ok(()) => {
            println!("apagador: removi {}", CAMINHO);
            0
        }
        Err(erro) => {
            println!("apagador: nao removeu {}: {}", CAMINHO, erro);
            3
        }
    }
}
```

Note that `apagador` closes the file (`drop(arquivo)`) **before** calling `remove`: with the file open, `remove` would return `Busy`.

### Example program 12: caderno

Appends a line with the time (UTC) to `/disco/caderno.txt` on every run (20 bytes per line), then writes the file to the screen and counts the lines. File: `programs/src/bin/caderno.rs`. Run `run caderno` twice and check with `ls /disco` (20, then 40 bytes). It writes to **`/disco`**, so the note only survives to the next boot if QEMU runs by hand over a copy of the image (README, "Keeping the disk between boots"); with `cargo run`, the note lasts until QEMU closes. (The comments in the source are in Portuguese, the project's language.)

```rust
//! `caderno`: acrescenta uma linha com a hora a um arquivo do disco e mostra o
//! arquivo inteiro.
//!
//! Cada vez que roda, o programa anota a hora atual (UTC) em
//! `/disco/caderno.txt`: na primeira vez cria o arquivo, nas seguintes
//! acrescenta ao fim. Depois abre o arquivo, escreve o conteúdo na tela e
//! conta as linhas. É o jeito mais curto de ver a persistência do `/disco`:
//! rode `run caderno` duas vezes e o arquivo terá duas linhas (20 bytes cada).
//!
//! Atenção: enquanto o QEMU roda com `snapshot=on` (é o caso de `cargo run` e
//! `cargo test`), o `/disco` volta ao inicial quando o QEMU fecha. Para a
//! anotação sobreviver a um reinício, rode o QEMU à mão sobre uma cópia da
//! imagem do disco (passo a passo no README, "Preservando o disco entre boots").
//!
//! O escritor é fechado **antes** de o arquivo ser aberto para leitura, e o
//! caminho é fixo porque programas não recebem argumentos de linha de comando.
//!
//! Rode com `run caderno`.

#![no_std]
#![no_main]

use core::fmt::{self, Write};

use runtime::sys::write;
use runtime::{entry, println, time, File, FileWriter, FsError, TimeError};

entry!(main);

/// O arquivo que o programa cria e amplia.
const CAMINHO: &str = "/disco/caderno.txt";

/// Uma linha do caderno: `AAAA-MM-DD HH:MM:SS` (19 caracteres) mais `\n`.
const TAMANHO_DA_LINHA: usize = 20;

/// Por que o programa não conseguiu anotar ou mostrar o caderno.
enum Falha {
    Arquivo(FsError),
    Hora(TimeError),
    /// A linha não coube nos 20 bytes (não deveria acontecer).
    Formato,
}

impl From<FsError> for Falha {
    fn from(erro: FsError) -> Falha {
        Falha::Arquivo(erro)
    }
}

impl From<TimeError> for Falha {
    fn from(erro: TimeError) -> Falha {
        Falha::Hora(erro)
    }
}

impl fmt::Display for Falha {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Falha::Arquivo(erro) => write!(f, "{}", erro),
            Falha::Hora(erro) => write!(f, "{}", erro),
            Falha::Formato => f.write_str("linha com formato inesperado"),
        }
    }
}

/// Um buffer na pilha que recebe a linha formatada: o programa não precisa de heap.
struct Linha {
    bytes: [u8; TAMANHO_DA_LINHA],
    usados: usize,
}

impl Write for Linha {
    fn write_str(&mut self, texto: &str) -> fmt::Result {
        let fim = self.usados + texto.len();
        if fim > TAMANHO_DA_LINHA {
            return Err(fmt::Error);
        }
        self.bytes[self.usados..fim].copy_from_slice(texto.as_bytes());
        self.usados = fim;
        Ok(())
    }
}

fn main() -> i32 {
    match anota().and_then(|_| mostra()) {
        Ok(linhas) => {
            // "1 linha", mas "2 linhas".
            let palavra = if linhas == 1 { "linha" } else { "linhas" };
            println!("caderno: {} {} em {}", linhas, palavra, CAMINHO);
            0
        }
        Err(erro) => {
            println!("caderno: nao gravou {}: {}", CAMINHO, erro);
            1
        }
    }
}

/// Acrescenta a linha com a hora ao caderno, criando o arquivo se ele não existe.
fn anota() -> Result<(), Falha> {
    // O nome já existe (`Exists`): acrescenta ao fim, sem tocar no que já está lá.
    let mut escritor = match FileWriter::create(CAMINHO) {
        Ok(escritor) => escritor,
        Err(FsError::Exists) => FileWriter::append(CAMINHO)?,
        Err(erro) => return Err(erro.into()),
    };
    let agora = time::now()?;
    let mut linha = Linha { bytes: [0; TAMANHO_DA_LINHA], usados: 0 };
    // `{}` escreve `AAAA-MM-DD HH:MM:SS`; o relógio é sempre UTC.
    writeln!(linha, "{}", agora).map_err(|_| Falha::Formato)?;
    escritor.write_all(&linha.bytes[..linha.usados])?;
    // `escritor` sai de escopo aqui e o descritor é fechado, antes de o
    // arquivo ser aberto para leitura.
    Ok(())
}

/// Escreve o caderno na tela, em pedaços de 128 bytes, e devolve quantas
/// linhas ele tem.
fn mostra() -> Result<usize, Falha> {
    let mut arquivo = File::open(CAMINHO)?;
    let mut pedaco = [0u8; 128];
    let mut linhas = 0;
    loop {
        match arquivo.read(&mut pedaco)? {
            // 0 bytes: o arquivo acabou.
            0 => return Ok(linhas),
            n => {
                // `write` entrega os bytes à tela exatamente como estão.
                write(&pedaco[..n]);
                linhas += pedaco[..n].iter().filter(|&&b| b == b'\n').count();
            }
        }
    }
}
```

Note that the writer is closed (it goes out of scope in `anota`) **before** the file is opened for reading in `mostra`.

## The time

A program can ask the kernel for the date and time, which reads them from the computer's clock (the RTC chip). You do not touch the clock: you use `time::now()`, from the runtime library, which makes the `SYS_TIME` syscall for you.

- `time::now()` returns `Result<DateTime, TimeError>`.
- A `DateTime` has the fields `year`, `month`, `day`, `hour`, `minute`, and `second`. Written with `{}`, it comes out in the format `AAAA-MM-DD HH:MM:SS`, for example `2026-10-08 15:04:05`.
- The time is **always UTC**. os-rust knows nothing about time zones or daylight saving time: if you want local time, add the difference yourself.
- `TimeError::Clock` means the clock returned impossible values or did not respond. It is rare, but a careful program handles the error instead of assuming the time always comes.

The `hora` program does just that. Run `run hora` and compare with the prompt's `data` command: the difference is a few seconds. The complete code:

```rust
//! `hora`: writes the current date and time (UTC).
//!
//! The program does not touch the clock: it asks the kernel for the time (`time` syscall, through the
//! runtime library) and writes what it received, in the same format as the prompt's
//! `data` command. Run `run hora` and compare with `data`.

#![no_std]
#![no_main]

use runtime::{entry, println, time};

entry!(main);

fn main() -> i32 {
    // Asks the kernel (`time` syscall) for the date and time. It only fails if the
    // computer's clock returns impossible values.
    match time::now() {
        Ok(agora) => {
            // `{}` writes `AAAA-MM-DD HH:MM:SS`; the clock is always UTC.
            println!("{} UTC", agora);
            0
        }
        Err(erro) => {
            // A non-zero exit code makes the kernel warn at the
            // prompt (`[run] hora terminou com codigo 1`).
            println!("hora: {}", erro);
            1
        }
    }
}
```

To see the time without writing a program, the prompt has the `data` command. The syscall contract (registers, 8-byte buffer format, and errors) is in `SYSCALLS.md`, section 5, `SYS_TIME`.

## Network (TCP)

A program talks to the network through **TCP sockets**. You do not call syscalls 17 to 23 by hand: you use the runtime library's types.

- `Ipv4`: an address, four bytes. `Ipv4::parse("10.0.2.2")` returns `Option<Ipv4>`; written with `{}`, it comes out as `a.b.c.d`. The computer's own address, seen from inside the program, is `127.0.0.1` (the loopback, which works even without a network card); `10.0.2.2` is the real computer when os-rust runs in QEMU.
- `TcpListener::bind(port)` opens a server; `accept()` waits for a connection and returns a `TcpStream`; `set_timeout(ms)` changes the timeout.
- `TcpStream::connect(ip, port)` opens a connection; `read(&mut buffer)` reads what arrived and returns `0` when the other side closed; `write(&bytes)` sends what fits and `write_all(&bytes)` sends everything; `set_timeout(ms)` changes the timeout.
- `NetError` says what went wrong (`TimedOut`, `Refused`, `Reset`, `AddrInUse`, `NoBuffers`, `NoNetwork`, ...) and is written with `{}`.

The rules:

- **There is no infinite wait.** Every wait (`accept`, `read`, `connect`, `write` with a full buffer) has a timeout: 10 seconds by default, 0 to 60 000 ms with `set_timeout`; `connect` waits at most 3 seconds. A forgotten server does not hold the computer, because there is no Ctrl+C.
- **Close what you opened.** `TcpListener` and `TcpStream` close the socket by themselves when they go out of scope (`drop`). A program can have up to 4 sockets open at a time, and the whole kernel has 8 connections; if the program ends without closing, the kernel closes for it.
- **TCP only.** There is no UDP, host names (DNS) or automatic address (DHCP). Waiting for the network does not use CPU: the program stays blocked and the other programs go on.

### Example program 13: servidor

Waits for a connection on port 8000 and answers with a minimal HTTP/1.0 page (88 bytes: 65 of header and 23 of body). File: `programs/src/bin/servidor.rs`. Run with `run servidor cliente`, or alone with `run servidor` (and, in QEMU started by hand with `hostfwd`, `curl http://127.0.0.1:8080/` on the computer; see the README, "Networking in QEMU").

```rust
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
```

### Example program 14: cliente

Connects to a server on port 8000, asks for a page (33 bytes) and writes the response on screen, without the `\r`, up to 1024 bytes. File: `programs/src/bin/cliente.rs`. Just pressing Enter at the address uses `127.0.0.1`.

```rust
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
```

Notice that each wait already has its own timeout (`set_timeout`), that the `servidor` closes the connection and the listener (`drop`) before saying it answered, and that the `cliente` reads until `read` returns `0`, the sign that the server closed.

### Example program 15: ftpd

An FTP server: it exchanges files between the real computer and the `/ram` and `/disco` volumes. Each FTP command (`LIST`, `RETR`, `STOR`, `DELE`, `MKD`...) becomes a file call, and the bytes travel over TCP. File: `programs/src/bin/ftpd.rs`. To run it, start QEMU by hand with `hostfwd` (README, "FTP server"), type `run ftpd`, press Enter at the address question and use `curl ftp://127.0.0.1:2121/` on the computer. It ends with `SITE EXIT` or after 30 s with no client.

```rust
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
```

FTP uses **two connections**, and the program combines the two kinds of socket with the file API:

- The **control connection** (port 21) carries the commands and the answers. The `TcpListener` on 21 stays open for the whole life of the program; each `accept` returns a `TcpStream` that becomes a `Sessao`. Commands arrive in arbitrary pieces, so the session accumulates bytes and only acts on whole lines.
- The **data connection** (port 2100) carries the contents of **one** transfer. `PASV` and `EPSV` open a new `TcpListener` on 2100 and tell the client where to connect; the data command does `accept` on it and **spends** it: the listener and the connection are closed at the end of every transfer, whether it worked or not. A program only has 4 sockets (the listener on 21, the control connection, the listener on 2100 and the data connection), which is why the server serves **one client at a time** and only in passive mode.
- **Validation comes before `150`**: `RETR` opens the file and reads the first chunk, `LIST` reads the directory and `STOR` creates the file before saying `150`. So a missing or invalid name becomes `550` or `553` without opening any transfer.
- **Cleaning up the partial file**: if a `STOR` fails halfway, the file is closed (`drop`) and only then deleted with `remove`, because the kernel does not delete an open file. Since `FileWriter::create` never overwrites, a name that already exists is an error and the old file stays intact.
- There is no infinite wait and no Ctrl+C, so every wait has a timeout and `SITE EXIT` ends the program.

## Writing and building your own program

1. Create a new file in `programs/src/bin/`, named after your program (for example, `programs/src/bin/meuprograma.rs`).
2. Start from the minimal structure in the section above: `#![no_std]`, `#![no_main]`, `entry!(main);`, and `fn main() -> i32`. Do not create a `Cargo.toml`: programs share the one in `programs/`, which already depends on the runtime library.
3. Write your program's logic using `print!`/`println!`, `read_line`, and `Box`/`Vec` if you need them (with `extern crate alloc;`).
4. Run `cargo run` at the repository root. Your program is compiled automatically along with the others, and embedded in the boot image.
5. At the os-rust prompt, type `run meuprograma`. If the name does not match any embedded program, the prompt lists the available names, so check your file's name.

There is no separate manual build step: if `cargo run` works, your program is embedded. This applies to programs inside the repository; to create a program outside it, see the next section.

## Programs outside the repository

Until now, every program lived inside the repository, in `programs/src/bin/`, and went into the kernel on every `cargo run`. This section teaches the other way: create the program **outside** the repository, in a folder of your own, build it for os-rust, and bring it into the system over FTP, **without rebuilding the kernel**. The cycle has four steps:

1. **Build** the program on your computer, for the os-rust user target.
2. **Send** the file to os-rust with the FTP server (`ftpd`).
3. **Run** it at the prompt, with `run /disco/<name>`.
4. **Fix** the code, rebuild, and send it again.

This way is useful when you want to experiment quickly, or when the program should not be mixed in with the project's code. If you just want one more program embedded in the boot image, the previous section is still the simplest way.

### Requirements

- **`rustup`** installed. It downloads, by itself, the nightly toolchain the project pins and the source code of the standard library (`rust-src`) that `build-std` needs.
- **The os-rust repository cloned**, for example at `~/os-rust`. The runtime library, the user target, and the linker script come from it. Your program does not copy the `runtime`: it points to the repository's own, so the program and the kernel are always from the same version.
- **Internet on the first build.** `rustup` downloads the nightly and `rust-src`, and cargo downloads one dependency of the `runtime` (`linked_list_allocator`). After that, everything is cached.
- **QEMU with FTP working**, as the "FTP server" section of `README.en.md` teaches: the QEMU line with `hostfwd` and a copy of the disk, without `snapshot=on`.

You do **not** need `bootimage` to build the external program; it is only needed to build the kernel.

**Warning:** create the project **outside** the repository folder. Cargo looks for a workspace by walking up the folders, and a project created inside os-rust would be treated as part of it, with its configuration, which breaks the recipe (see "Common errors").

### What each file does

A program inside the repository inherits configuration from the folders above it. A project outside inherits nothing, so you must create, in the new folder, every item the repository provides behind the scenes. There are seven:

| File | Where it comes from | What it is for |
|------|---------------------|----------------|
| `rust-toolchain.toml` | copy | Pins the same nightly as the kernel and downloads `rust-src`. Without it, `build-std` does not work. |
| `x86_64-os_rust_user.json` | copy | The user target: x86-64 with no operating system, no SSE, `panic = abort`. |
| `link.ld` | copy of `programs/link.ld` | Places the program at `0x4000_0000`, with sections in separate pages (W^X), as the loader requires. |
| `.cargo/config.toml` | written | Turns on `build-std` (builds `core`, `alloc`, and `compiler_builtins` for the target). |
| `Cargo.toml` | written | The program's name and the `runtime` dependency. |
| `build.rs` | written | Passes `-T link.ld` to the linker. |
| `src/main.rs` | written | The program. |

A few terms, for anyone who has not met them yet:

- **Target** is the description of the machine the compiler generates code for. Here, an x86-64 processor with no operating system underneath, described in the file `x86_64-os_rust_user.json`.
- **`build-std`** is the cargo feature that builds the minimal standard library (`core`, `alloc`, and `compiler_builtins`) for that target, since no ready-made version of it exists.
- **Linker script** (`link.ld`) is the file that tells the linker at which address the program lives and how the sections (code, data) are laid out in memory.
- **W^X** (write or execute, never both) is the rule that a memory page cannot be writable and executable at the same time; that is why each section sits in separate pages.
- **ELF** is the executable file format the compiler produces and the os-rust loader knows how to read.

### Creating the project

The commands below work on Linux, macOS, and in WSL on Windows. The `OSRUST` variable holds the repository folder; adjust the value to your case.

```sh
# 1. onde está o os-rust, e criar o projeto FORA dele
export OSRUST="$HOME/os-rust"
cd "$HOME"
cargo new saudacao --vcs none
cd saudacao

# 2. copiar do repositório a toolchain, o target e o linker script
cp "$OSRUST/rust-toolchain.toml" "$OSRUST/x86_64-os_rust_user.json" "$OSRUST/programs/link.ld" .

# 3. ligar o build-std
mkdir -p .cargo
cat > .cargo/config.toml <<'EOF'
[unstable]
build-std = ["core", "compiler_builtins", "alloc"]
build-std-features = ["compiler-builtins-mem"]
json-target-spec = true
EOF

# 4. Cargo.toml (o caminho do runtime vem de $OSRUST)
cat > Cargo.toml <<EOF
[package]
name = "saudacao"
version = "0.1.0"
edition = "2021"

[dependencies]
runtime = { path = "$OSRUST/runtime" }
EOF

# 5. build.rs: diz ao linker para usar o link.ld
cat > build.rs <<'EOF'
fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    println!("cargo:rustc-link-arg=-T{manifest}/link.ld");
    println!("cargo:rerun-if-changed=link.ld");
}
EOF
```

(The comments inside the block are in Portuguese on purpose: the block is the same, byte for byte, in both guides. The numbered list below explains each step.)

What each step does and why it exists:

1. **Create the project.** `cargo new saudacao --vcs none` creates the `saudacao` folder with a `Cargo.toml` and a `src/main.rs`. The `--vcs none` avoids creating a git repository for nothing. Use up to 8 lowercase letters for the name: the binary is named after the package, and the file name on the os-rust volume must fit 8.3.
2. **Copy three files.** Copying, rather than writing by hand, avoids drift: the toolchain, the target, and the base address stay identical to the kernel's. Without `rust-toolchain.toml`, cargo would use another toolchain and `build-std` would not find `rust-src`.
3. **Turn on `build-std`.** Without the `[unstable]` block, cargo would look for a ready-made standard library for the os-rust target, which does not exist, and the build would fail with `can't find crate for core`.
4. **Write `Cargo.toml`.** It names the program and points to the `runtime` by path. The unquoted `<<EOF` lets the shell replace `$OSRUST` with the variable's value; in the other blocks, the quotes (`<<'EOF'`) prevent that replacement, because the text must stay as it is. The file also does **not** set `[build] target`: the target always comes from the command line.
5. **Write `build.rs`.** It runs before the build and hands the linker the option `-T .../link.ld`. Without it, the linker would use the default Linux address, and the program would hang at startup or be rejected by the os-rust loader.

Step 6 writes `src/main.rs`, replacing the "Hello, world!" that `cargo new` created:

```rust
#![no_std]
#![no_main]

use runtime::{entry, print, println, read_line};

entry!(main);

fn main() -> i32 {
    print!("seu nome: ");
    let mut buffer = [0u8; 128];
    let nome = read_line(&mut buffer);
    println!("ola, {}! este programa foi feito fora do os-rust.", nome);
    0
}
```

It is the same skeleton as the "Minimal structure of a program" section: `#![no_std]` and `#![no_main]` drop the standard library and the conventional `main`, `entry!(main)` connects the entry point to your function, and `main` returns an `i32`, the exit code (`0` is success). The difference is the input: `read_line` takes a buffer of **your own** (here, 128 bytes) and returns the typed text, as in "Example program 1: eco". This program uses only `print!`, `println!`, and `read_line`; it does not need `alloc`. (The program's messages are in Portuguese, like the rest of the os-rust screen output.)

Before building, `ls -a` in the project folder should show:

```text
.cargo/  Cargo.toml  build.rs  link.ld  rust-toolchain.toml  src/  x86_64-os_rust_user.json
```

`Cargo.lock` and the `target/` folder appear after the first build.

### Building

```sh
cargo build --release --target x86_64-os_rust_user.json
wc -c target/x86_64-os_rust_user/release/saudacao
```

The first time takes a while: `rustup` downloads the nightly and `rust-src`, cargo downloads `linked_list_allocator`, and `build-std` builds `core` and `alloc` from scratch. Later builds are quick.

The ELF ends up at `target/x86_64-os_rust_user/release/saudacao`. The middle folder is named after the target, that is, the `.json` name without the extension, and the binary is named after the package. The second command prints the size in bytes: a program like this is a few tens of KB (when this guide was checked, about 17.8 KB). The os-rust loader accepts at most 64 KiB, that is, 65536 bytes; above that, `run` refuses the file.

### Sending it over FTP and running it

1. Start os-rust with the QEMU line from the "FTP server" section of `README.en.md` (copy of the disk, without `snapshot=on`, with `hostfwd`). The line is not repeated here so that there is a single version of it.
2. At the os-rust prompt, type `run ftpd` and press `Enter` at the address question, to accept `127.0.0.1`. The server shows that it is listening on port 21.
3. In another terminal, **within 30 seconds** (`ftpd` gives up if nobody connects in that time), send the program, from the project folder:

```sh
curl -T target/x86_64-os_rust_user/release/saudacao ftp://127.0.0.1:2121/disco/saudacao
```

   The `-T` uploads the file; the destination is `/disco/saudacao`, on the disk volume. Port 2121 on your computer is forwarded by QEMU to port 21 of os-rust.
4. Still within the 30-second window (`ftpd` goes back to waiting for another client after each session), check through FTP itself with `curl ftp://127.0.0.1:2121/disco/`. The listing should show `saudacao` with the same size that `wc -c` gave.
5. Stop the server so the os-rust prompt comes back: `curl -Q "SITE EXIT" ftp://127.0.0.1:2121/`. If you prefer, just wait 30 seconds with no client.
6. At the os-rust prompt, check with `ls /disco` and run the program with `run /disco/saudacao`:

```text
os-rust> run /disco/saudacao
seu nome: Cleuton
ola, Cleuton! este programa foi feito fora do os-rust.
os-rust>
```

### Changing and resending

Changed the text? Rebuild with the same `cargo build`. To resend, **delete the old version first**, because `ftpd` never overwrites a file that already exists (so an interrupted transfer cannot destroy the original):

```sh
curl -Q "DELE /disco/saudacao" ftp://127.0.0.1:2121/
```

and then repeat the `curl -T` from the previous section. `ftpd` serves one session at a time and goes back to waiting for a client, but each `curl` must start within 30 seconds of the previous one; otherwise `ftpd` exits, the prompt returns, and you must run `run ftpd` again.

Remember where the file lives. The `cargo run` disk uses `snapshot=on`, so the uploaded file disappears on the next boot; with the QEMU line from `README.en.md` (copy of the disk, without `snapshot=on`), it stays. `/ram`, on the other hand, goes back to its original contents on every boot, no matter what.

### Rules that still apply

- The syscall contract is the same (`SYSCALLS.md`), and everything this guide teaches about the `runtime` (files, network, time, `alloc`) applies equally to the external program.
- The program receives no command-line arguments.
- The executable file can be at most 64 KiB (65536 bytes).
- At most 4 programs run at the same time.
- When you update the os-rust repository, **rebuild** the external program: the syscall contract may have changed.

### Summary

1. `cargo new saudacao --vcs none`, outside the repository.
2. `cp` of `rust-toolchain.toml`, `x86_64-os_rust_user.json`, and `link.ld`.
3. Write `.cargo/config.toml`, `Cargo.toml`, `build.rs`, and `src/main.rs`.
4. `cargo build --release --target x86_64-os_rust_user.json`.
5. `run ftpd` in os-rust and, within 30 seconds, `curl -T ... ftp://127.0.0.1:2121/disco/saudacao`.
6. `SITE EXIT`, and at the prompt `run /disco/saudacao`. To change the program: `DELE`, rebuild, `curl -T` again.

## Testing

The simplest way to test is manually: `cargo run`, then `run <your program's name>`, watching the screen.

For automated verification, `cargo test` runs the kernel suite, which includes tests covering the loading and execution of user programs (using `hello`, `eco`, and `falha_memoria` as reference), of several programs at the same time (`tests/multitarefa.rs`: `ping`, `pong`, `contador_a`, `contador_b`, `eco2`), and of programs that read files (`tests/sistema_de_arquivos.rs`: `leitor`, `listador`, `visita`) and that ask for the time (`tests/relogio.rs`: `hora`). If you want a test dedicated to your own program, the pattern the project uses is a file in `tests/`, booting the kernel, "typing" the input with `interrupts::push_scancode`, and checking the result on the screen; `tests/user_runtime.rs` is the model.

## Common errors

- **My program does not show up in `run`**: check that the file name in `programs/src/bin/` (without `.rs`) is exactly what you typed after `run`, and that `cargo run` finished without a compile error.
- **`cargo run` fails to compile my program**: the reported error is in your code, not the kernel's; fix it and run `cargo run` again. The kernel does not boot with a program that failed to compile.
- **Linker error saying `_start` is missing**: you forgot `entry!(main);` in your program.
- **My program and another are interleaving output and I wanted a fixed order**: the order between programs is only the `yield_now` round-robin while the timer interrupts nobody; the kernel can interrupt any program at any instruction. If order matters, each program needs to wait for what it needs from another in a different way (two programs only talk over the network, by TCP sockets on `127.0.0.1`; see "Network (TCP)").
- **My program hangs and never returns to the prompt**: check that every code path in your `main` eventually returns (or calls `exit`); a program that never exits keeps occupying the prompt. If it is in `read_line`, it is waiting for you to press Enter.
- **`memory allocation of N bytes failed` and `terminou com codigo 101`**: the program asked for more memory than the 1 MiB heap can hold (the message comes after a `[panic] panicked at ...` line).
- **My program uses a syscall that does not exist**: the kernel ends the program with an error message, the same way it handles an invalid memory access; check the syscall number in `SYSCALLS.md`.
- **`File::open` returns `NoVolume` for `/disco`**: QEMU started without the disk, or the volume is faulty. `cargo run` already attaches the disk by itself; if you run QEMU by hand, attach `target/imagens/disco.img` as an IDE disk (`-drive file=target/imagens/disco.img,format=raw,if=ide,index=1`).
- **`File::open` returns `NotFound` for a file that is in `discos/`**: the name must fit 8.3 (up to 8 characters, a dot, up to 3 of extension), and `cargo run` must have been redone after you added the file.
- **`run /disco/...` says `arquivo grande demais`**: executables read from a file are at most 64 KiB.
- **Cargo cannot find `runtime/Cargo.toml` (program outside the repository)**: `OSRUST` is wrong, or `Cargo.toml` was written while `OSRUST` was empty. Check with `echo "$OSRUST"` and write `Cargo.toml` again.
- **`can't find crate for core`, or cargo asks you to install `rust-src`**: the copied `rust-toolchain.toml` is missing, or the `.cargo/config.toml` with `build-std` is missing, or cargo was run outside the project folder.
- **`relocation R_X86_64_32 out of range`, or the program hangs at startup**: `link.ld` or `build.rs` is not in use. Check that both are at the project root.
- **Link error saying `_start` is missing (program outside the repository)**: `entry!(main);` is missing from `src/main.rs`.
- **Cargo says the package "believes it's in a workspace when it's not"**: the project was created inside the os-rust folder. Create it outside.
- **`run /disco/<name>` says `arquivo grande demais`**: the ELF is over 64 KiB (65536 bytes). Check the size with `wc -c`.
- **`run` says the file is not an ELF or that the executable is invalid**: you sent the wrong file (for example `target/x86_64-os_rust_user/release/saudacao.d`). The ELF is the file without an extension.
- **`curl` ends with an FTP error when uploading**: the file already exists (delete it with `DELE /disco/saudacao` and send again) or `ftpd` had already exited after waiting more than 30 seconds (run `run ftpd` again).
