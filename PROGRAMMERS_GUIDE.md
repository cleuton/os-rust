# Programmer's Guide - os-rust

This guide is for anyone who wants to write a program that runs inside os-rust, without needing to understand the kernel internals. If you want to know how boot, memory, or interrupts work, the right document is `WALKTHROUGH.md`. This one assumes you just want to write code that runs in user mode, using the project's runtime library.

## Prerequisites

The same three as in `README.md`: Rust with the nightly toolchain pinned by the project (`rust-toolchain.toml`), `bootimage`, and QEMU. If you already run `cargo run` successfully at the repository root, you are ready to follow this guide.

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

## Writing and building your own program

1. Create a new file in `programs/src/bin/`, named after your program (for example, `programs/src/bin/meuprograma.rs`).
2. Start from the minimal structure in the section above: `#![no_std]`, `#![no_main]`, `entry!(main);`, and `fn main() -> i32`. Do not create a `Cargo.toml`: programs share the one in `programs/`, which already depends on the runtime library.
3. Write your program's logic using `print!`/`println!`, `read_line`, and `Box`/`Vec` if you need them (with `extern crate alloc;`).
4. Run `cargo run` at the repository root. Your program is compiled automatically along with the others, and embedded in the boot image.
5. At the os-rust prompt, type `run meuprograma`. If the name does not match any embedded program, the prompt lists the available names, so check your file's name.

There is no separate manual build step: if `cargo run` works, your program is embedded.

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
