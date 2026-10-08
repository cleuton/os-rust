# os-rust

```text
                    #####
                   // : \\
                ..-  --:  -..
           //--/ .:==███==+. \--\\
          //-\ +- ███ █ ███ -+ /-\\
           \\ | ██  @@@@@  ██ | //
            | | @  @     @  @ | |
            | | @██@ ███ @██@ | |
            | | @  @@@@@@   @ | |
           // | ██ @  █  % ██ | \\
          \\.- -- ███ █ ███ -- -.//
           \\--\. -- ███ -- ./--//
                 --. -.- .--
                   \\ : //
                    #####
                                         []
 .o:o.   .s::.       [.o= []  |]   .s:: =++=t
[o   o] [:___  ===== [r   []  |]  [:___  []
[o   o]     :]       []   []  |]      :] []
  o:o   ::__:        []    uu/|]  ::__:   \ut
```

![](imagem.jpg)

os-rust started as a minimal demonstration of "programming without an
operating system" in Rust: a binary that boots directly via BIOS in a QEMU
virtual machine, writes text to the screen using the VGA video buffer and
reads the keyboard through a hardware interrupt (IRQ1) to feed a minimal
command prompt, with no operating system underneath. From here on, the
project evolves in small steps toward a larger goal: see the Vision section
right below.

If you have never seen how a kernel/bootloader works, also read
[`WALKTHROUGH.md`](./WALKTHROUGH.md): it explains what is happening behind
the boot and the code.

## Vision

The central goal of os-rust is to allow someone to write a program, compile
that program separately from the kernel and run it on os-rust, in user mode,
using a documented programming interface. It is not a general-purpose
kernel: it is an educational kernel that evolves in small milestones, each
one designed to fit in a lesson, always prioritizing the shortest path to
running user programs before features such as a file system or full
multitasking.

## Status

The project documentation is also available in English (see the `ENGLISH VERSION` link at the top of each `.md` file).

**Current version: 0.9.1.** Milestones 0 (boot in VGA text mode, with a
welcome message, scrolling and readable panic handling), 1 (interrupts,
keyboard and command prompt), 2 (debugging infrastructure: serial output
and automated tests inside QEMU), 3 (memory: physical frame allocator,
paging and kernel heap), 4 (protection: own GDT and TSS, handlers for the
main exceptions, double fault with a dedicated stack), 4.1 (new identity:
the project is now called os-rust, with the logo above appearing on screen
at every boot) and 5 (first user program: ring 3, `write` and `exit`
syscalls, static ELF64 loader and the `run hello` command), 6 (programming
interface: keyboard reading and memory for programs, runtime library, and
failure isolation demonstrated with `run eco` and `run falha_memoria`) and
7 (multitasking: several programs at the same time, each in its own memory,
with cooperative and preemptive context switching, demonstrated with `run
ping pong` and `run contador_a contador_b`) and 8 (file system: a ramdisk
and an ATA disk, both read-only FAT16, with `ls`, `cat`, file syscalls and
the execution of a program read from the disk, demonstrated with `run
/disco/bin/visita`) and 9 (RTC driver: the kernel reads the date and time
from the computer's clock, shown by the `data` command and offered to
programs through the `SYS_TIME` syscall, with `run hora`) are completed and
are what this repository runs today. Milestones 10 and 11 (file writing) are
planned. The original talk demo, in the format used in class, is preserved
in the git tag `v1.0-demo` and can still be used as is.

## Roadmap

os-rust advances in numbered milestones, each ending in something visible in
QEMU. The table below summarizes the first thirteen planned milestones; the
details of each follow.

| Milestone | Goal | Demonstrable | Status |
|---|---|---|---|
| 0. Boot and VGA text | Boot via BIOS and write text in VGA mode, with readable panic handling. | Welcome message, text scrolling and a readable panic screen. | Completed |
| 1. Interrupts, keyboard and prompt | Handle hardware interrupts, read the keyboard and offer a prompt of fixed commands. | Type `help` at the prompt and see the response. | Completed |
| 2. Debugging infrastructure | Have serial output and automated tests running inside QEMU. | `cargo test` running kernel tests inside QEMU. | Completed |
| 3. Memory | Allocate physical frames and pages, and have a heap allocator inside the kernel. | `Vec` and `Box` working inside the kernel, visible through a prompt command. | Completed |
| 4. Protection | Have own GDT and TSS and handle the main exceptions, including page fault and double fault with a dedicated stack. | Trigger a page fault and see a readable message instead of a reboot. | Completed |
| 4.1. New identity | Rename the project to os-rust everywhere (package, crate, target, folders, prompt, messages, documentation) and add the logo to the boot screen and the README. | `cargo run` shows the logo, then `os-rust v0.4.1` and the `os-rust> ` prompt. | Completed |
| **5. First user program (central milestone)** | Run the first user program in protected mode (ring 3), using a syscall mechanism and an ELF64 executable loader, embedded in the boot image. | The `run hello` command at the prompt executes a user-mode program that prints to the screen and returns to the prompt. | Completed |
| 6. Programming interface | Extend the syscall contract (keyboard, memory, exit code) and offer a runtime library for those who write programs. | A program written by a student reads keyboard input and responds (`run eco`); a program with invalid memory access is terminated without bringing down the kernel (`run falha_memoria`). | Completed |
| 7. Multitasking | Switch context between more than one loaded program, first cooperatively and then preemptively. | Two programs interleaving output on the screen (`run ping pong`, `run contador_a contador_b`). | Completed |
| 8. File system | Read files from a file system, first an embedded ramdisk and then a read-only disk driver. | List files and execute a program read from the disk (`ls /ram`, `cat /ram/ola.txt`, `run /disco/bin/visita`). | Completed |
| 9. RTC driver | Read the date and time from the real-time clock (CMOS chip) by polling and offer them to the prompt and to user programs through a new syscall. | `data` shows the date and time in UTC; `run hora` shows the same, requested from the kernel by a program. | Completed |
| 10. Ramdisk writing | Write to FAT16, creating, writing, extending and deleting files, applied to the `/ram` volume. | Create a file in `/ram`, show it with `cat` and delete it, also from a user program. | Planned |
| 11. ATA disk writing | Write sectors to the ATA disk and make `/disco` writable through the same writing layer, with persistence across boots. | Write a file to `/disco`, restart QEMU and read the file. | Planned |

### Milestone details

**Milestone 0. Boot and VGA text.** Completed. BIOS boot, welcome message,
text scrolling and a readable panic screen. Does not depend on any other
milestone.

**Milestone 1. Interrupts, keyboard and prompt.** Completed. IDT, 8259 PIC,
IRQ1 handler, scancode translation and a prompt with fixed commands.
Demonstrable: type `help` and see the response. Depends on Milestone 0.

**Milestone 2. Debugging infrastructure.** Completed. Serial output and
automated tests running inside QEMU, with the result reported to the host.
Demonstrable: `cargo test` running kernel tests in QEMU. Depends on
Milestone 1.

**Milestone 3. Memory.** Completed. Physical frame allocator from the
bootloader's memory map (using the full physical memory mapping that the
`bootloader`'s `map_physical_memory` feature provides), translation/creation
of mappings in the active page table, and a fixed heap mapped at boot (100
KiB in this milestone; 16 MiB since Milestone 8), with a global allocator
(`Box`, `Vec`, `String`, ...). Demonstrable: the `mem` command at the prompt
shows the usable physical memory, the position/size of the heap, a `Box`
with its address, and a `Vec` built from empty. Depends on Milestone 2.

**Milestone 4. Protection.** Completed. Own GDT (kernel code segment + TSS
descriptor) and a TSS with a dedicated 20 KiB stack in the Interrupt Stack
Table for the double fault; handlers for the five main exceptions (`#BP`,
`#UD`, `#GP`, `#PF`, `#DF`), with a readable screen (screen + serial) for
the four fatal ones. Demonstrable: the `falha <tipo>` command at the prompt
triggers each exception on purpose — `falha pagina` shows a readable message
instead of a reboot, and `falha pilha` proves that a kernel stack overflow
no longer restarts QEMU. Depends on Milestone 3.

**Milestone 4.1. New identity.** Completed. Maintenance milestone: the
project is now called `os-rust` everywhere (package in `Cargo.toml`, crate
`os_rust`, custom target `x86_64-os_rust.json`, repository root folder,
prompt, kernel messages and documentation), the version goes up to 0.4.1,
and the ASCII logo above appears on the boot screen, before the
identification and the prompt. No new kernel capability. Demonstrable:
`cargo run` shows the complete logo, then `os-rust v0.4.1`, then the
`os-rust> ` prompt. Depends on Milestone 4.

**Milestone 5. First user program (central milestone of the roadmap).**
Completed. Ring 3, syscall mechanism (`syscall`/`sysret`), first version of
the syscall contract with `write` and `exit`, static ELF64 loader and a
program embedded in the boot image, compiled separately from the kernel and
embedded at compile time. Contract v1 is in [`SYSCALLS.md`](SYSCALLS.md).
Demonstrable: the `run hello` command at the prompt executes a user-mode
program that prints to the screen and returns to the prompt; `run crash`
shows that a failure in a program does not bring down the kernel. Depends on
Milestone 4. This is the milestone that delivers the central goal described
in the Vision section.

**Milestone 6. Programming interface.** Completed. Extended syscall contract
(version 2 of [`SYSCALLS.md`](SYSCALLS.md): `SYS_READ_LINE` reads a line from
the keyboard and `SYS_ALLOC` gives memory to the program), a runtime library
for programs (`runtime/`: entry point with `entry!`, `print!` and
`println!`, `read_line` and an allocator that enables `Box` and `Vec`), and
isolation: a failure in the program terminates the program, not the kernel.
Demonstrable: `run eco` reads a line from the keyboard and responds; `run
falha_memoria` accesses invalid memory and is terminated without bringing
down the kernel, with the prompt responding right afterwards. Those who want
to write their own program follow the
[`GUIA_DO_PROGRAMADOR.md`](GUIA_DO_PROGRAMADOR.md). Depends on Milestone 5.

**Milestone 7. Multitasking.** Completed. More than one program loaded at
the same time, each in its own memory (one page table per program), with
context switching first cooperative (the `SYS_YIELD` syscall, contract
version 3 of [`SYSCALLS.md`](SYSCALLS.md)) and then preemptive (the PIT
timer, at 100 Hz, through IRQ0 of the 8259 PIC, takes the CPU from those
that do not yield it after a 50 ms slice). The scheduler is round-robin, the
kernel itself is never switched, and the keyboard goes to the program that
asked first. Demonstrable: `run ping pong` shows the lines of both
alternating (each yields the CPU to the other); `run contador_a contador_b`
shows the output of both interleaved without either asking for its turn;
`run falha_memoria contador_a` terminates only the one that failed. Depends
on Milestone 5.

**Milestone 8. File system.** Completed. The kernel now reads files,
**read-only** (until Milestone 9; writing arrives with Milestones 10 and
11), from two fixed FAT16 volumes: `/ram`, a ramdisk embedded in the boot
image, and `/disco`, an ATA disk read via PIO (I/O ports, by polling, with
no DMA and no disk interrupt). The same FAT reader reads both, through a
minimal block device abstraction. The `ls` and `cat` commands list and show
files; four new syscalls (`SYS_OPEN`, `SYS_READ`, `SYS_CLOSE`,
`SYS_READ_DIR`, contract version 4 of [`SYSCALLS.md`](SYSCALLS.md)) let a
program open, read and list, each with its own file table; and `run`
accepts the path of an executable. The volume images are generated inside
`cargo run`/`cargo test` by the `fatimg` crate, from the `discos/`
directory, with no new tool. Demonstrable: `ls /ram` and `cat /ram/ola.txt`
show the ramdisk; `run /disco/bin/visita` executes a program that **exists
only on the disk** (the kernel never saw it at compile time); `run leitor`
and `run listador` read the disk through syscalls. Without the disk, the
kernel boots normally and `ls /disco` says the volume is unavailable.
Depends on Milestone 5.

**Milestone 9. RTC driver.** Completed. The kernel now has its fifth driver
(after VGA, serial, PS/2 keyboard and ATA disk): the real-time clock, the
CMOS chip that keeps the date and time with its own battery. The chip is
read through two I/O ports (`0x70` selects the register, `0x71` delivers the
value), by polling, without the IRQ8 interrupt and without touching the PIC.
The driver handles values in BCD or binary, 12- or 24-hour time, the
two-digit year (with the century register, when it exists), waits for the
end of the update the chip performs every second and reads twice until the
readings match, and rejects impossible dates (month 0, February 31). The
kernel only knows UTC: there is no time zone. A new syscall, `SYS_TIME`
(contract version 5 of [`SYSCALLS.md`](SYSCALLS.md)), delivers the time to a
program, and the runtime library hides it behind `time::now()`.
Demonstrable: `data` shows `AAAA-MM-DD HH:MM:SS UTC` and, a few seconds
later, a later time; `run hora` shows the same time, requested from the
kernel by a program. Depends on Milestone 5.

**Milestone 10. Ramdisk writing.** Planned. The first half of file writing,
on the volume that lives in memory and is lost on restart, where making
mistakes costs nothing. Scope: the FAT16 writing layer (allocating and
freeing clusters, updating both copies of the FAT, creating, writing,
extending and deleting files), applied to the `/ram` volume; prompt commands
to create and delete files; new syscalls for creation, writing and removal
(contract version 6); an example program that writes a file and another
program that reads it. `/disco` remains read-only. Demonstrable: create a
file in `/ram`, show it with `cat` and delete it, also from a user program.
Depends on Milestone 8.

**Milestone 11. ATA disk writing.** Planned. The second half: bringing the
same layer to the real disk. Scope: sector writing via PIO in the ATA driver
(including the disk cache flush command); `/disco` writable through the same
layer as Milestone 10; directory creation and removal; the order of writes
that keeps the volume consistent if interrupted; and persistence across two
QEMU boots over the same image, with the decision of how `build.rs` stops
regenerating the disk when the goal is to preserve it (today it regenerates
it on every `cargo run`/`cargo test`, which would erase any write). The
tests always operate on a copy of the image. Contract version 7, if there
are new syscalls (for example, for directories). Demonstrable: write a file
to `/disco`, restart QEMU and read the file. Depends on Milestone 10.

### Why this order

The first user program (Milestone 5) depends neither on a file system nor on
multitasking: until a file system exists, the program travels embedded in
the boot image itself, and running one program at a time is enough to prove
that user mode works end to end. That is why the file system (Milestone 8)
and multitasking (Milestone 7) come after the first user program, not
before.

### Programming interface

The programming interface between the kernel and user programs (the syscall
contract: numbers, semantics, register conventions, error codes, executable
format, load region and initial stack) is documented in a single versioned
file, [`SYSCALLS.md`](SYSCALLS.md), since Milestone 5 (contract version 1:
`write` and `exit`). The current version is 5, from Milestone 9, which adds
to what was already there (reading a line from the keyboard and memory, in
Milestone 6; yielding the CPU, in Milestone 7; reading files, in Milestone
8) the time (`SYS_TIME`). It is the only source of this interface: no
syscall exists without being in it, and an automated test checks that the
text keeps matching the code. An incompatible change to the contract
increases its version and requires updating, in the same milestone, the
programs' runtime library (`runtime/`), which uses the same constants as the
kernel (`abi` crate).

Those who want to **write their own user program** should follow the
[`GUIA_DO_PROGRAMADOR.md`](GUIA_DO_PROGRAMADOR.md): the minimal structure of
a program, the runtime library (`print!`, `read_line`, `Box`/`Vec`), a
summary of the contract and the step-by-step to compile and run with `run
<name>`. [`WALKTHROUGH.md`](WALKTHROUGH.md) explains the kernel from the
inside; the guide explains how to program for it, without needing to
understand the GDT, IDT or the ELF64 loader.

### About changing this order

The milestone table above is the project's official reference. Changing the
order of the milestones or inserting a new milestone requires updating this
table in the README before any implementation begins.

## Prerequisites

You will need three things: Rust (with the **nightly** toolchain), the
`bootimage` tool and **QEMU**. The step-by-step below assumes you have never
installed any of the three.

### 1. Rust + nightly toolchain

If you do not have Rust installed yet, install it via
[rustup](https://rustup.rs/):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

This project already pins the exact nightly toolchain it needs through the
`rust-toolchain.toml` file at the repository root: you do **not** need to
run `rustup default nightly` or anything similar; `cargo` detects this file
automatically and downloads the right toolchain the first time you compile
the project. You only need to make sure `rustup` itself is installed (step
above).

If you prefer to install the toolchain manually before compiling, use the
same version pinned in `rust-toolchain.toml`:

```sh
rustup toolchain install "$(grep channel rust-toolchain.toml | cut -d'"' -f2)" \
  --component rust-src,llvm-tools-preview
```

The toolchain is pinned to an exact date (not a floating "nightly") because
the format of the custom target specification JSON file used in this project
is unstable and changes from time to time between nightly versions: a fixed
date guarantees that `cargo run` works the same on any machine, today and a
year from now.

### 2. `bootimage`

`bootimage` is the tool that turns the kernel binary into a bootable disk
image and knows how to call QEMU. Install the exact version tested by this
project with:

```sh
cargo install bootimage --version 0.10.5 --locked
```

### 3. QEMU

QEMU is the virtual machine emulator used to "boot" the system without
needing physical hardware.

- **Ubuntu/Debian**:
  ```sh
  sudo apt install qemu-system-x86
  ```
- **Fedora**:
  ```sh
  sudo dnf install qemu-system-x86
  ```
- **Arch Linux**:
  ```sh
  sudo pacman -S qemu-full
  ```
- **macOS** (via [Homebrew](https://brew.sh/)):
  ```sh
  brew install qemu
  ```
- **Windows**: download the installer from
  [qemu.org/download](https://www.qemu.org/download/#windows) and make sure
  `qemu-system-x86_64.exe` is available in the `PATH`.

Confirm that QEMU is accessible:

```sh
qemu-system-x86_64 --version
```

## Building and running

With the three prerequisites above installed, from the repository root run:

```sh
cargo run
```

This will: compile the kernel for this project's custom bare-metal target
(`x86_64-os_rust.json`), generate a boot image with `bootimage`, and open a
QEMU window that boots via BIOS straight into that binary. Within a few
seconds you should see the os-rust logo (the 20 lines of the symbol and the
name, at the top of the screen), followed by the identification line
`os-rust v0.9.1` — the project's current version — and the `os-rust> `
prompt ready for typing, not an ordinary terminal.

The same `cargo run` also compiles the runtime library (`runtime/`) and the
user programs (the `programs/` folder, for the `x86_64-os_rust_user.json`
target) and embeds them in the boot image, and also generates the ramdisk
and disk images (`discos/`, through the `fatimg` crate) and attaches the
disk to QEMU (`target/imagens/disco.img`, through the `bootimage` arguments
in `Cargo.toml`): there is no extra manual step, nor a new tool. Always run
`cargo run` and `cargo test` from the repository root: the disk path is
relative to it. If a user program does not compile, `cargo run` stops with
the compiler error instead of generating an image with an outdated program.

Click on the QEMU window to make sure it has keyboard focus and type a
command. The supported keyboard layout is **US QWERTY, ASCII only**: there
is no support for accents, ABNT2 or other layouts.

At the same time, the terminal where you ran `cargo run` starts showing
diagnostic messages written by the kernel (start of boot with the version,
GDT/TSS active, interrupts enabled, memory initialized, prompt ready, and
any breakpoint, invalid instruction, protection violation, page fault,
double fault or panic that happens) — a text channel separate from the QEMU
screen, which can be scrolled, copied and pasted. No additional manual step
is needed for this: it is the same `cargo run` as always.

### Available commands

| Command | What it does |
|---|---|
| `help` | Lists the available commands |
| `clear` | Clears the screen and repositions the prompt at the top |
| `echo <text>` | Writes `<text>` on the next line |
| `sobre` | Shows a short description of os-rust, including the current version |
| `panic` | Triggers a deliberate panic (same error screen as panic handling) |
| `mem` | Shows usable physical memory, position/size of the heap, a `Box` and a `Vec` |
| `falha <tipo>` | Deliberately triggers a CPU exception: `pagina` (`#PF`), `pilha` (`#DF`), `opcode` (`#UD`), `protecao` (`#GP`) or `breakpoint` (`#BP`); with no argument or with an unknown type, lists the available types |
| `ls <path>` | Lists the entries of a directory (type, size and name), for example `ls /ram` or `ls /disco/docs`; with no argument shows the usage and the state of the volumes; a nonexistent path, a file in place of a directory and an unavailable volume give a clear message |
| `cat <path>` | Shows the contents of a file, for example `cat /ram/ola.txt` (bytes outside printable ASCII appear as `■`); same error messages as `ls` |
| `data` | Shows the date and time of the computer's clock, in UTC, in the format `AAAA-MM-DD HH:MM:SS UTC`; if the clock is invalid or does not respond, shows a clear message. Extra arguments are ignored |
| `run <target> [<target>...]` | Executes user programs in user mode (ring 3), up to 4 at the same time (the same target may repeat), and returns to the prompt when the last one ends. A target that starts with `/` is the path of an ELF64 executable (up to 64 KiB) read from a volume, like `/disco/bin/visita`; any other is the name of a program embedded in the image. The two can be mixed (`run /disco/bin/visita hello`). With no argument, with an invalid target or with more than 4 targets, it starts none |

### User programs: `hello`, `crash`, `eco`, `falha_memoria`, `ping`, `pong`, `contador_a`, `contador_b`, `eco2`, `leitor`, `listador`, `hora` and `visita`

The `run` command executes programs written outside the kernel. Each one
lives in `programs/src/bin/` and is compiled as a static ELF64 executable:

- **`hello`** (`run hello`): writes `Ola do ring 3!` and ends. Demonstrates
  the complete path: the program runs in ring 3, asks the kernel to write to
  the screen with the `write` syscall and ends with the `exit` syscall, and
  the prompt responds again. Since Milestone 6 it uses the runtime library
  (`println!`); the raw `syscall` instruction lives in
  `runtime/src/sys.rs`.
- **`crash`** (`run crash`): executes an invalid instruction on purpose. The
  kernel shows `[run] crash encerrado por erro: #UD (Invalid Opcode) em
  <endereço>` and the prompt keeps working (including `run hello` right
  afterwards). Compare with `falha opcode`, which triggers the same error
  **inside the kernel** and stops everything: an error in a program never
  brings down the kernel.
- **`eco`** (`run eco`): asks for a text (`digite algo: `), waits for you to
  type and press Enter (the text appears as you type, and Backspace erases),
  and responds `voce digitou: <text>` and `palavras: <n>`. Demonstrates
  keyboard reading by syscall (`SYS_READ_LINE`) and dynamic memory inside a
  program (the word count uses a `Vec`, through the library's allocator,
  which asks for memory with `SYS_ALLOC`). While it waits, the keyboard is
  its own; when it ends, it returns to the prompt.
- **`falha_memoria`** (`run falha_memoria`): writes to an address that is not
  its own (`0xdeadbeef`). The kernel shows `[run] falha_memoria encerrado
  por erro de memoria: #PF (Page Fault) em <endereço>`, the error code and
  the `endereco de falha`, terminates only the program, and the prompt keeps
  working (`run hello` and `run eco` right afterwards).
- **`ping`** and **`pong`** (`run ping pong`): each one writes four lines
  (`ping 1`, `pong 1`, ...) and **yields the CPU** (`yield_now`, the
  `SYS_YIELD` syscall) to the other after each line, so the lines appear
  alternating. They demonstrate cooperative context switching.
- **`contador_a`** and **`contador_b`** (`run contador_a contador_b`): count
  in a long loop and write eight lines each (`A: 1`, `B: 1`, ...) **without
  ever yielding the CPU**. The lines of both appear interleaved anyway,
  because the timer interrupts each program after a time slice. They
  demonstrate preemption.
- **`eco2`** (`run eco eco2`): the sibling of `eco`, with the `eco2:` prefix
  in the response. Type a line and Enter, and then another: the first goes
  to `eco`, which asked first, and the second to `eco2`.

- **`leitor`** (`run leitor`): opens `/disco/docs/longo.txt`, a file that
  takes up more than one cluster of the disk, and writes the contents to the
  screen, reading in 128-byte chunks through the file syscalls. The program
  never talks to the disk: it only asks the kernel. `run leitor leitor` runs
  two readers at the same time, each with its own read position.
- **`listador`** (`run listador`): lists the root of the disk, one entry per
  line (`dir 0 bin`, `dir 0 docs`, `arquivo <size> leiame.txt`).
- **`hora`** (`run hora`): asks the kernel for the date and time (`SYS_TIME`
  syscall, through `time::now()`) and writes them as `AAAA-MM-DD HH:MM:SS
  UTC`, the same format as the `data` command; if the clock is invalid, it
  writes `hora: relogio invalido` and exits with code 1.
- **`visita`** (`run /disco/bin/visita`): writes `visita: fui carregado do
  disco!`. It exists **only on the disk**: the source is in
  `programs/src/disco/`, outside `programs/src/bin/`, so it is neither in
  the list of `run` with no argument nor in the kernel image.

### Files: the `/ram` and `/disco` volumes

The contents of the two volumes come from the repository's `discos/`
directory (`discos/ram/` and `discos/disco/`), turned into a FAT16 image
during `cargo run`/`cargo test`; the disk also receives the `visita`
executable and a 65,537-byte file (`docs/grande.bin`) that exists only to
prove the rejection of an executable that is too large. A path is
`/<volume>/<component>/...`, with 8.3 names (up to 8 characters, a dot and
up to 3 of extension), case-insensitive, at most 64 bytes and 8 levels; `.`
and `..` are not accepted. Each program can have 4 open files.

Demonstration:

```text
os-rust> ls /ram
os-rust> cat /ram/ola.txt
os-rust> ls /disco
os-rust> run /disco/bin/visita
os-rust> run leitor
os-rust> run listador
os-rust> run /disco/bin/visita hello
```

To see the kernel without the disk, run QEMU by hand without the second
`-drive` (`bootimage run`... or `qemu-system-x86_64 -drive
format=raw,file=<bootimage image>`): the boot is normal, `ls /ram` works and
`ls /disco` says `volume indisponivel: /disco (sem disco)`. Limitations:
read-only (nothing is created, changed or deleted), only short 8.3 names
(FAT long-name entries are ignored), one FAT16 volume per source, no block
cache and no partitions.

To see multitasking by hand, run `cargo run` and type `run ping pong`, `run
contador_a contador_b`, `run eco eco2` and `run falha_memoria contador_a`
(the error message appears immediately and the counter completes the
output).

How a program requests services from the kernel (syscall numbers,
registers, error codes, where it is loaded) is in
[`SYSCALLS.md`](SYSCALLS.md). To write your own program, see the
[`GUIA_DO_PROGRAMADOR.md`](GUIA_DO_PROGRAMADOR.md).

Backspace erases the last typed character; Enter executes the line. An
unrecognized command shows an error message suggesting `help`.

To end the demonstration, type `panic` or simply close the QEMU window. Both
are a normal end of execution, not an error.

## Running the automated tests

With the same three prerequisites from the previous section installed, run:

```sh
cargo test
```

This compiles the kernel in a special test mode, boots in QEMU **without
opening any window** (it works the same in a session without a graphical
display, such as a remote terminal over SSH) and automatically executes all
the kernel's tests. At the end, QEMU shuts itself down — there is no need to
close anything manually.

The command compiles and runs several test binaries in sequence (the kernel
library, the production binary and each file inside `tests/`); for each of
them, the terminal shows a `Running <N> tests` line with the total number of
tests in that binary, followed by one line per test ending in `[ok]` when it
passes. It is normal to see this sequence repeat several times in a single
`cargo test` call — each binary restarts the kernel from scratch, so each
one has its own count.

If a test fails (a check that should be true is not, for example), the
terminal shows the name of the test, the word that indicates failure, the
file and line where the check failed, and the failure message; the QEMU of
that binary shuts down immediately, without running the remaining tests of
that binary.

The result of everything reaches you through the **exit code** of the
`cargo test` command itself, the same way as any other terminal command:
after running `cargo test`, type

```sh
echo $?
```

A `0` means that all tests of all binaries passed. A nonzero number means
that at least one test failed, hung (a test that never ends is interrupted
after a maximum time) or caused a CPU exception without a handler. This is
useful for automating checks: a script can run `cargo test` and decide what
to do just by looking at that exit code, without needing to interpret the
text.

After running `cargo test`, `cargo run` keeps working normally — no
test-only code or device is part of the image used by the normal execution
of the kernel.

### What the Milestone 5 tests cover

To check user mode by hand, run `cargo run` and type `run hello` (message on
screen and prompt back) and `run crash` (error message and prompt back). The
automated suite (`cargo test`) covers, in addition to the previous tests:

- `tests/user_mode.rs` (runs programs in ring 3 inside QEMU): `hello` writes
  the message and hands control back (transition to ring 3, `write`,
  `exit`); a nonexistent name lists the programs; the user memory region is
  free after each program, including running `hello` 50 times in a row;
  `write` with a kernel pointer or a size that is too large returns an error
  and the program continues; an invalid ELF is rejected without mapping
  anything; and each program failure (`crash`, privileged instruction,
  division by zero, access to kernel memory, invalid stack, `int` without a
  gate, nonexistent syscall) terminates only the program, and `hello` runs
  again afterwards.
- `src/elf.rs`: one check for each rule of the ELF reader (magic, class,
  type, machine, sizes, alignment, region, overlap, entry point, number of
  segments).
- `src/syscall.rs`: `SYSCALLS.md` agrees with the code's constants (syscall
  numbers, error codes, user region, version).
- `src/shell.rs`: `run` with no argument and `run` with an unknown name list
  the programs. `src/vga_buffer.rs`: `write_bytes` replaces non-ASCII bytes
  with the square `0xfe`.

### What the Milestone 6 tests cover

To check by hand, run `cargo run` and type `run eco` (type a text and Enter:
it comes back on screen) and `run falha_memoria` (memory error message,
prompt back; `run hello` and `run eco` work afterwards). The automated suite
(`cargo test`) covers, in addition to the previous tests:

- `tests/user_runtime.rs` (programs in ring 3 inside QEMU; the tests "type"
  by pushing scancodes onto the keyboard queue): `eco` returns exactly the
  typed text (with Shift and Backspace, and an empty line) and counts the
  words with a `Vec`; the memory syscall returns the start of the heap,
  contiguous areas rounded to pages, writable and zeroed memory,
  `ERR_INVAL` for size zero and `ERR_NOMEM` above 1 MiB, and returns the
  frames at the end (150 runs of 1 MiB without exhausting memory); a write
  past the end of the heap is `#PF`; `SYS_READ_LINE` with a kernel pointer,
  a read-only page or a size that is too large returns an error without
  waiting for a key; `falha_memoria` ends in `#PF` at address `0xdeadbeef`
  without bringing down the kernel (the next program runs, including 40
  failures in a row); the prompt reads the keyboard again after a program,
  and keys typed in advance reach it intact; `GUIA_DO_PROGRAMADOR.md`
  contains, literally, the code of `eco` and `falha_memoria`; the program
  names are unique.
- `src/keyboard.rs`: `read_line` returns exactly what was typed, with Shift,
  Backspace, character limit, ignored keys, and returns with interrupts off.
  `src/syscall.rs`: `SYSCALLS.md` (version 2) agrees with the constants
  (numbers 3 and 4, `ERR_NOMEM`, heap window, line and heap limits).
  `src/elf.rs`: a segment that invades the heap is rejected. `src/gdt.rs`:
  the top of the kernel entry stack is a multiple of 16. `src/shell.rs`:
  `run` lists `eco` and `falha_memoria`, and a program's `#PF` shows `erro
  de memoria`.

### What the Milestone 9 tests cover

To check by hand, run `cargo run`, type `data`, wait a few seconds and type
`data` again (the second time is later; compare with `date -u` on the
computer), then `run hora` (the same time, within a few seconds'
difference). The automated suite (`cargo test`) covers, in addition to the
previous tests, without depending on the real time:

- `src/rtc.rs`: the conversion over fabricated registers: BCD and binary, 12
  and 24 hours (the four combinations give the same instant), midnight and
  noon, impossible fields, day against month and the leap year, the century
  rule, and the repetition of the reading when the chip is updating
  (including giving up after 5 attempts);
- `src/shell.rs`: the `data` command (format, error messages, extra
  arguments ignored) and its presence in `help`;
- `src/syscall.rs`: contract version 5 matches the code (number, error,
  8-byte layout);
- `tests/relogio.rs` (inside QEMU): reading in a plausible range and not
  going backwards, `SYS_TIME` with a valid buffer, wrong size and invalid
  pointer, `run hora`, two `hora` at the same time, and the programs from
  previous milestones;
- `tests/user_runtime.rs`: the code of `hora` in the guide is identical to
  the repository's.

### What the Milestone 8 tests cover

To check by hand, run `cargo run` and the demonstration from the **Files**
section above. The automated suite (`cargo test`) covers, in addition to the
previous tests:

- `src/fat.rs` (the FAT reader, over images fabricated in memory by the
  `fatimg` crate and tampered with byte by byte): valid boot sector and each
  kind of invalid; root and subdirectory; case-insensitive 8.3 names; empty
  file, one-cluster, multi-cluster and of a size that is not a multiple of
  the cluster; reading in chunks; chain with a cycle, that leaves the
  volume, that falls on a free or reserved cluster, shorter than the file
  size; directory with a cycle; long-name, deleted and label entries
  ignored; nonexistent path, wrong type and device error. `src/fs.rs`: path
  validation (limits, `.` and `..`, case), ramdisk reading, file table
  (limit, invalid descriptor, independent positions, `Drop`). `src/ata.rs`:
  the wait with an exact limit, without a clock. `src/blockdev.rs` and
  `src/shell.rs` (`ls` and `cat` with each error).
- `tests/disco_ata.rs` (inside QEMU, with the extra disks from
  `Cargo.toml`): boot sector and known files read from the disk via PIO;
  absent drive detected without hanging; corrupted volume rejected without a
  panic; the same FAT reader over the ramdisk and over the disk.
- `tests/sistema_de_arquivos.rs`: `ls`, `cat` and `run` through the prompt
  (disk and ramdisk), rejection of a file that is not an ELF and of an
  executable that is too large, rejection of the whole command with an
  invalid target, mix of embedded and path, 100 runs of a program from the
  disk without leaking frames; each error of the file syscalls (hand-built
  ELFs), the 4-file limit and the release when ending by `exit` and by
  error; `leitor`, `listador` and two readers at the same time; no disk,
  invalid volume and corrupted FAT (clear message, no panic nor infinite
  loop).
- `tests/user_runtime.rs`: `GUIA_DO_PROGRAMADOR.md` contains, literally, the
  code of `leitor` and `listador`; `visita` is not among the embedded
  programs. `src/syscall.rs`: `SYSCALLS.md` (version 4) agrees with the
  constants (syscalls, errors, limits, volumes).
- `tests/artefatos.rs` keeps checking the delivered files, including the new
  ones (`fatimg/`, `discos/`, the kernel modules and the tests).

### What the Milestone 7 tests cover

To check by hand, run `cargo run` and type `run ping pong`, `run contador_a
contador_b`, `run falha_memoria contador_a` and `run eco eco2`. The
automated suite (`cargo test`) covers, in addition to the previous tests:

- `tests/multitarefa.rs` (several programs in ring 3 at the same time,
  inside QEMU; the support programs are hand-built ELFs):
  - **cooperative**: context switching preserves the registers, stack and
    memory of two programs; `ping` and `pong` alternate the output;
    `SYS_YIELD` with no other program ready returns immediately; the one
    that ends first does not hinder the other; simultaneous programs do not
    see each other's memory;
  - **`run`**: a single program is the same as Milestone 6; unknown name and
    more than 4 programs reject the whole request without starting any (and
    without leaking frames); the same name twice are independent instances;
    100 runs of two programs return all the frames; the prompt reads the
    keyboard again;
  - **preemptive**: `contador_a` and `contador_b` (which do not yield the
    CPU) interleave because of the timer; a short program ends before a long
    one; the tick in ring 0 does not switch context; the end of interrupt is
    never left pending; each `write` comes out whole even under preemption;
  - **failures**: a failure terminates only the one that failed, with the
    message immediately, and its frames return while the others stay alive;
    one program's `exit` does not terminate the others;
  - **keyboard**: the line goes to the program that asked first, with no
    loss nor duplication; a half-typed line belongs to whoever asked first;
    programs blocked on the keyboard consume no CPU (the kernel sleeps, and
    each idle loop comes from an interrupt); a key typed while another
    program computes wakes up the one waiting.
- `tests/artefatos.rs`: no delivered file of the project mentions the tool
  used to write the specifications, nor points to the specification folders
  (the file list is embedded by `build.rs`).
- `tests/user_runtime.rs`: `GUIA_DO_PROGRAMADOR.md` contains, literally, the
  code of `ping`, `pong`, `contador_a`, `contador_b` and `eco2`; the program
  names are unique.
- `src/syscall.rs`: `SYSCALLS.md` (version 3) agrees with the constants
  (`SYS_YIELD`, maximum programs, slice and timer frequency).
  `src/memory.rs`: a new and destroyed address space returns all the frames,
  and a page of one space does not show up in the others. `src/task.rs`: the
  context layout is what the assembly assumes. `src/timer.rs`: the PIT
  divisor gives 100 Hz.

To understand from the inside how this mechanism works (the serial port,
the test executor without a standard library, and how the result travels
from the kernel to the exit code of `cargo test`), see the corresponding
chapter in [`WALKTHROUGH.md`](./WALKTHROUGH.md).

## Project structure

- `src/lib.rs`: declares the kernel modules, exposes the name (`NAME`) and
  the version identification (`VERSION`, both derived from `Cargo.toml` at
  compile time) and the welcome message (`print_welcome`), initializes the
  serial port, the GDT/TSS, interrupts, memory (frames, paging, heap), the
  file volumes, the timer and the syscall mechanism, and contains the test
  infrastructure (the test executor, panic handling in test mode and the
  communication with QEMU about success or failure).
- `src/main.rs`: the production binary — boot entry point; clears the
  screen, writes the welcome message, draws the logo and runs the idle loop
  that feeds the command prompt with the keyboard.
- `src/logo.rs` / `src/logo.txt`: the canonical text of the ASCII logo (20
  lines), included at compile time without any Rust escape.
- `src/vga_buffer.rs`: all the logic for writing text to the VGA video
  buffer (`0xb8000`): colors, line advance, scrolling, backspace, hardware
  cursor, and the drawing of the logo (`draw_logo`) directly on lines 0-19,
  translating `█` to byte `0xDB` of code page 437.
- `src/serial.rs`: text writing to the serial port (UART 16550), used for
  diagnostics during normal execution and to report test results.
- `src/panic.rs`: what happens when the system encounters an unrecoverable
  error (panic); shows a readable message on screen (and also on serial)
  instead of hanging or restarting without explanation. The reentrancy guard
  that avoids hanging while rewriting the screen is shared with the fatal
  exception screens of `interrupts.rs`.
- `src/gdt.rs`: the GDT (kernel code and data segments, user code and data
  segments, TSS descriptor) and the TSS, with a dedicated 20 KiB stack in
  the Interrupt Stack Table for the double fault and another for entries
  into the kernel coming from ring 3.
- `src/interrupts.rs`: the IDT, the exception handlers (`#BP`, `#DE`,
  `#UD`, `#GP`, `#SS`, `#NP`, `#PF`, `#DF`, the latter running on the
  dedicated TSS/IST stack) and the reprogramming of the 8259 PIC for the
  keyboard (IRQ0 and IRQ1). The fatal exceptions share a single function
  that builds the exception screen (screen + serial); when the exception
  comes from a user program (ring 3), the handler terminates only the
  program.
- `src/keyboard.rs`: translation of scancodes (Scan Code Set 1) to ASCII,
  US QWERTY layout, the line assembler (`LineEditor`, with echo and
  Backspace) that the scheduler uses for the line of whoever asked first,
  and `read_line`, the blocking read of a single reader.
- `src/memory.rs`: the physical frame allocator from the bootloader's memory
  map (global, with frame recycling), and the translation/creation of
  mappings in the active page table (using the full physical mapping),
  including the pages of user programs, and the address space of each
  program (`AddressSpace`: own P4 and P3 tables, created, activated and
  destroyed).
- `src/elf.rs`: the static ELF64 executable reader (hand-written) and the
  validation rules.
- `src/user.rs`: the user memory layout (code and data, heap and stack), the
  loader (one address space per program), the program's heap (`grow_heap`),
  `run_images`/`run_all` (several programs at the same time) and the entry
  into and exit from ring 3 (`enter_user`, `resume_task`, `leave_user`).
- `src/task.rs`: what the kernel keeps for each loaded program: its state in
  ring 3 (`TaskContext`), what point of its life it is at and the pending
  keyboard request.
- `src/scheduler.rs`: the round-robin scheduler: picks the next task,
  switches context (by `SYS_YIELD`, by the timer or by keyboard wait),
  delivers the typed line to whoever asked first, sleeps with `hlt` when all
  are waiting, and returns the resources of whoever ends.
- `src/timer.rs`: the PIT (100 Hz, IRQ0) and the timer interrupt stub, which
  only switches tasks when it interrupts ring 3.
- `src/blockdev.rs`: the block device (read a 512-byte sector by block
  address), implemented by the ramdisk (`RamDisk`) and the ATA disk.
- `src/fat.rs`: the read-only FAT16 reader, which treats the volume as
  untrusted data (chains with a step limit, validated cluster numbers).
- `src/ata.rs`: the ATA disk driver via PIO and polling, with a wait limit
  and absence detection, atomic with respect to the scheduler.
- `src/rtc.rs`: the real-time clock driver (ports `0x70`/`0x71`, BCD and
  binary, 12 and 24 hours, update window, century), by polling.
- `src/fs.rs`: the `/ram` and `/disco` volumes, the paths and the open file
  table of each program.
- `src/syscall.rs`: the `syscall` mechanism (entry through the instruction,
  return through `iretq`), the dispatcher and the `write`, `exit`,
  `read_line`, `alloc`, `yield`, `open`, `read`, `close`, `read_dir` and
  `time` syscalls.
- `src/programs.rs`: the table of embedded programs, generated by
  `build.rs`.
- `src/allocator.rs`: the fixed range of virtual addresses of the heap, the
  global allocator (`Box`, `Vec`, `String`, ...) and the handling of an
  exhausted heap.
- `src/shell.rs`: the line buffer and the command prompt (`help`, `clear`,
  `echo`, `sobre`, `panic`, `mem`, `falha <tipo>`, `ls`, `cat`, `data`,
  `run <target>`).
- `abi/`: the syscall contract constants (numbers, errors, limits), shared
  by the kernel and the runtime library.
- `runtime/`: the user programs' runtime library (`entry!`,
  `print!`/`println!`, `read_line`, `yield_now`, `File`/`Dir` for reading
  files, `time::now()`, global allocator, `panic` handler).
- `programs/`: the user programs crate (`hello`, `crash`, `eco`,
  `falha_memoria`, `ping`, `pong`, `contador_a`, `contador_b`, `eco2`,
  `leitor`, `listador`, `hora`, one file per program in `src/bin/`, plus
  `visita` in `src/disco/`, which only goes to the disk), with the linker
  script (`link.ld`); compiled for the user target by the root `build.rs`,
  never directly.
- `GUIA_DO_PROGRAMADOR.md`: the guide for those who write programs for
  os-rust.
- `fatimg/`: the FAT16 image generator (`no_std`, dependency-free), used by
  `build.rs` to generate the ramdisk and the disk and by the tests to
  fabricate and tamper with images.
- `discos/`: the contents of the volumes (`discos/ram/` and
  `discos/disco/`).
- `build.rs`: compiles `programs/` (with a nested `cargo`), embeds the ELFs
  in the kernel, generates the ramdisk images (embedded in the kernel) and
  the disk (`target/imagens/`, attached to QEMU) and the list of delivered
  files that `tests/artefatos.rs` checks.
- `SYSCALLS.md`: the syscall contract (version 4).
- `tests/`: the integration tests, each one starting the kernel from scratch
  in its own binary — a boot test (which also checks the version in the
  welcome message), a test whose expected result is a panic, tests of the
  frame allocator, paging and heap, and two dedicated protection tests:
  `double_fault.rs` (proves the handler runs on the dedicated IST stack) and
  `page_fault.rs` (proves the expected fault address), `user_mode.rs` (runs
  user programs in ring 3), `user_runtime.rs` (keyboard reading, memory,
  isolation and the guide), `multitarefa.rs` (several programs at the same
  time: context switching, preemption, isolation and keyboard),
  `disco_ata.rs` (the ATA driver), `sistema_de_arquivos.rs` (`ls`, `cat`,
  `run` by path, file syscalls and invalid volumes) and `artefatos.rs`
  (hygiene of the delivered files).
- `x86_64-os_rust.json`: the custom bare-metal target specification (no
  operating system underneath).
- `x86_64-os_rust_user.json`: the user programs' (ring 3) target
  specification, distinct from the kernel's target.
- `.cargo/config.toml`: configures `cargo run`/`cargo test` to use
  `bootimage` as the *runner* automatically, and enables compiling the
  `alloc` crate for this custom target.
- `CHANGELOG.md`: the project's change history, version by version.
