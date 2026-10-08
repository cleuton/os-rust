# Changelog

All notable changes to this project are recorded in this file, one version
at a time. The format loosely follows
[Keep a Changelog](https://keepachangelog.com/), and the versions follow
[Semantic Versioning](https://semver.org/).

## [0.9.1] - 2026-10-08

### Added

- English version of the documentation.

## [0.9.0] - 2026-10-08

Milestone 9: RTC driver. The kernel now **reads the date and time** from the
real-time clock (the CMOS chip), by polling, and offers them to the prompt
and to user programs. `data` shows `AAAA-MM-DD HH:MM:SS UTC`; `run hora`
shows the same, requested from the kernel by a program through a new
syscall. The kernel only knows UTC. This milestone also records in the
roadmap the file writing track: Milestones 10 (ramdisk) and 11 (ATA disk),
planned.

### Added

- RTC driver (`src/rtc.rs`): reads year, month, day, hour, minute and second
  through ports `0x70` and `0x71`. Handles BCD and binary, 12 and 24 hours,
  the century (with a fallback rule: 70 to 99 become 19xx, 00 to 69 become
  20xx), waits for the chip's update to finish and reads twice until the
  readings match (up to 5 attempts, with a wait limit: it never hangs).
  Rejects impossible values, including day against month and the leap year.
  The read is atomic with respect to the scheduler.
- `data` command in the prompt, listed in `help`; an invalid, unstable or
  unresponsive clock gives a clear message, never a panic.
- `SYS_TIME` syscall (10) and the `ERR_CLOCK` error (`-11`): syscall contract
  version 5 (`SYSCALLS.md`), with the 8-byte `DateTime` buffer. Programs from
  versions 1 to 4 keep working.
- `abi` crate: `SYS_TIME`, `ERR_CLOCK`, `TIME_SIZE` and `DateTime` (with
  `Display` in the `AAAA-MM-DD HH:MM:SS` format).
- Runtime library: `time::now()`, `DateTime` and `TimeError`
  (`runtime/src/time.rs`).
- Example program `hora` (embedded), a "A hora" section in
  `GUIA_DO_PROGRAMADOR.md` with the complete code, and a chapter on the RTC
  in `WALKTHROUGH.md`.
- Tests: those in `src/rtc.rs` (over fabricated registers), those of the
  `data` command in `src/shell.rs`, those of contract version 5 in
  `src/syscall.rs`, `tests/relogio.rs` and the guide's in
  `tests/user_runtime.rs`. Also `vga_buffer::screen_count_clock_lines`, which
  exists only for the tests.

### Changed

- The `README.md` roadmap: Milestone 9 stops being a generic "Drivers" and
  becomes "RTC driver" (completed); Milestones 10 (writing to the ramdisk)
  and 11 (writing to the ATA disk) come in, planned. The statement that the
  volumes are read-only now holds "up to Milestone 9".
- The test in `tests/sistema_de_arquivos.rs` that used number 10 as an
  example of a nonexistent syscall now uses 11, the first free number in
  contract version 5.

## [0.8.0] - 2026-10-07

Milestone 8: file system. os-rust now **reads files**, always read-only,
from two FAT16 volumes: `/ram`, a ramdisk embedded in the boot image, and
`/disco`, an ATA disk read via PIO. The same FAT reader reads both. `ls
/ram` and `cat /ram/ola.txt` show the ramdisk; `run /disco/bin/visita`
executes a program that exists **only on the disk**; `run leitor` and `run
listador` read the disk through syscalls. Without the disk, the kernel boots
normally and `ls /disco` says the volume is unavailable.

### Added

- Block device (`src/blockdev.rs`): read a 512-byte sector by block address;
  implemented by the ramdisk and the ATA disk.
- Read-only FAT16 reader (`src/fat.rs`): boot sector validated field by
  field, allocation table, root directory, subdirectories, cluster chains
  and case-insensitive 8.3 names. Treats the volume as untrusted data:
  validated cluster numbers, traversals with a step limit, size checked
  against the chain; a chain with a cycle, outside the volume or shorter
  than the file is a read error, never an infinite loop nor a panic.
- ATA driver via PIO (`src/ata.rs`): `IDENTIFY`, LBA28 read, polling with a
  limit of 100,000 status reads, missing disk detection, no DMA and no disk
  IRQ (the PIC does not change). Reading a sector is atomic with respect to
  the scheduler.
- Volumes and open files (`src/fs.rs`): `/ram` and `/disco`, 8.3 paths of up
  to 64 bytes and 8 levels (`.` and `..` rejected), unavailable volume with
  a reason (the kernel never panics because of a volume) and a table of up
  to 4 open files per program, closed when the program ends.
- Commands `ls <path>` and `cat <path>`; `run` accepts the path of an
  executable (argument that starts with `/`, up to 64 KiB), mixed with names
  of embedded programs.
- Syscalls `SYS_OPEN` (6), `SYS_READ` (7), `SYS_CLOSE` (8) and
  `SYS_READ_DIR` (9) and the errors `ERR_NOENT` to `ERR_IO` (`-4` to `-10`):
  syscall contract version 4 (`SYSCALLS.md`), with the "File system" section
  (volumes, paths, limits, `DirEntryRaw`). Programs from versions 1 to 3
  keep working.
- Runtime library: `File`, `Dir`, `DirEntry` and `FsError`
  (`runtime/src/fs.rs`); descriptors are closed automatically.
- `fatimg` crate: the FAT16 image generator, `no_std` and dependency-free.
  `build.rs` generates the ramdisk and the disk (`target/imagens/`) on every
  `cargo run`/`cargo test`, and the tests use it to fabricate and tamper
  with images. The disk is attached to QEMU through the `bootimage`
  arguments, with no manual step nor new tool.
- Example programs `leitor` and `listador` (embedded) and `visita` (disk
  only, `programs/src/disco/`). The volumes' contents are in `discos/`.
- "Arquivos" section in `GUIA_DO_PROGRAMADOR.md`, with the complete code of
  `leitor` and `listador`, and a chapter in `WALKTHROUGH.md`.
- Tests: `tests/disco_ata.rs`, `tests/sistema_de_arquivos.rs`, the reader's
  tests in `src/fat.rs` (over fabricated and tampered images) and the
  guide's in `tests/user_runtime.rs`. Also `fs::replace_volume`,
  `fs::open_files_in_use` and `fs::mount_state`, which exist only for the
  tests.
- `abi` crate: `SYS_OPEN`, `SYS_READ`, `SYS_CLOSE`, `SYS_READ_DIR`, the seven
  new errors, `MAX_PATH_LEN`, `MAX_OPEN_FILES`, `MAX_EXEC_SIZE` and
  `DirEntryRaw`.

### Changed

- The kernel heap goes from 100 KiB to 16 MiB: `run` reads the whole
  executable into the heap (up to 4 of 64 KiB) and the reader tests
  fabricate volumes of ~2 MiB.
- A task's name stops being a `&'static str` and becomes a `ProgramName` (up
  to 16 bytes, inside the task itself), because the name of a program read
  from a file comes from a typed path.
- `help` lists `ls` and `cat`, and the description of `run` mentions paths.
  The first diagnostic line of each volume (`[fs] /ram: ...`,
  `[fs] /disco: ...`) appears on the serial during boot.
- `[package.metadata.bootimage]` in `Cargo.toml` attaches the disk to QEMU in
  `cargo run` and `cargo test` (and, in the tests, two more positions: a
  corrupted volume image and a channel with no disk).

## [0.7.0] - 2026-10-06

Milestone 7: multitasking. os-rust now runs **several user programs at the
same time**, with the kernel alternating the CPU among them: first
cooperatively (the program yields the CPU through a new syscall) and then
preemptively (a hardware timer takes the CPU from those that do not yield
it). Each program runs in its own memory. `run ping pong` shows the lines of
both alternating; `run contador_a contador_b` shows the output of both
interleaved without either asking for its turn; `run falha_memoria
contador_a` terminates only the one that failed; `run eco eco2` delivers
each typed line to the program that asked first.

### Added

- `SYS_YIELD` syscall (5: yields the CPU to the next ready program) and
  syscall contract version 3 (`SYSCALLS.md`): the new syscall, the rules for
  simultaneous execution (what the program can and cannot assume), the time
  slice, the maximum of 4 programs at the same time and the keyboard rule
  with several programs. Programs from versions 1 and 2 keep working.
- One task per loaded program (`src/task.rs`): its state in ring 3
  (`TaskContext`, the 15 general registers plus the `iretq` frame), its
  execution state and the pending keyboard request.
- Round-robin scheduler (`src/scheduler.rs`): every switch happens at the
  ring 3 → ring 0 boundary, with the kernel entry stack empty, so there is
  no per-task kernel stack; the kernel itself is never switched.
- Own address space for each program (`memory::AddressSpace`): own P4 and P3
  tables, copied from the kernel's, with the user region empty; switching
  tasks switches `CR3`. When it ends, all of the program's frames, including
  those of the page tables, return to the allocator.
- Timer (`src/timer.rs`): PIT at 100 Hz via IRQ0 of the 8259 PIC, 5-tick
  slice (50 ms) and preemption. A tick that interrupts ring 0 only sends the
  EOI and returns.
- `keyboard::LineEditor`: the line assembler extracted from `read_line`,
  used by the scheduler to assemble the line of whoever asked first.
- `run <name> [<name>...]`: loads up to 4 programs, all or nothing, and
  returns the prompt when the last one ends. The same name may repeat.
- Runtime library: `yield_now()`. Each `print!`/`println!` now goes out in a
  single `write` call (up to 256 bytes), so a line is not split by another
  program's output.
- Example programs `ping` and `pong` (alternate with `yield_now`),
  `contador_a` and `contador_b` (count in a long loop without ever yielding
  the CPU) and `eco2` (the sibling of `eco`, for `run eco eco2`).
- "Multitarefa" section in `GUIA_DO_PROGRAMADOR.md`, with the complete code
  of the five new programs, and a chapter in `WALKTHROUGH.md`.
- Tests: `tests/multitarefa.rs` (cooperative context, alternation,
  preemption, memory and failure isolation, keyboard with several programs,
  leak over 100 runs, `run` refusals) and `tests/artefatos.rs` (hygiene of
  the delivered files, with the list embedded by `build.rs`). Also
  `memory::frames_outstanding` and `scheduler::set_poll_hook`, which exist
  only for the tests.
- `abi` crate: `SYS_YIELD`, `MAX_TASKS`, `TIMER_HZ` and `SLICE_TICKS`.

### Changed

- The `syscall` stub builds a `TaskContext` on the kernel stack and returns
  to the program via `iretq` (before, via `sysretq`): `rcx` and `r11` come
  back intact, although the contract still says they are destroyed.
- `SYS_READ_LINE` blocks the task instead of waiting inside the syscall:
  whoever waits uses no CPU, and with all of them waiting the kernel sleeps
  in `hlt`.
- The PIC mask releases IRQ0 together with IRQ1.
- A program's termination message appears at the moment it ends, and the
  prompt returns when the last one ends.
- `run eco xyz` now treats `xyz` as the name of a second program (before,
  the arguments after the first name were ignored).
- The `disponiveis:` list in the `run` messages breaks the line between
  names, because with more programs it exceeded one screen line.
- The frame allocator only advances the counter when it hands out a frame.
- Project version: `0.6.0` → `0.7.0`.

## [0.6.0] - 2026-09-28

Milestone 6: programming interface. Writing, compiling and running a user
program becomes possible without knowing the kernel inside out: the syscall
contract gains keyboard reading and memory for the program, there is a
runtime library, and the guarantee that a failure in the program does not
bring down the kernel gains a demonstration program. `run eco` reads a line
from the keyboard and answers; `run falha_memoria` is terminated for
invalid memory access without bringing down the kernel.

### Added

- Syscalls `SYS_READ_LINE` (3: waits for a line from the keyboard, with echo
  and Backspace done by the kernel, and delivers it to the program) and
  `SYS_ALLOC` (4: extends the program's heap, in a window of up to 1 MiB at
  `0x6000_0000`), and the `ERR_NOMEM` error code (`-3`).
- Syscall contract version 2 (`SYSCALLS.md`): the two syscalls, the new
  error, the heap window, the ELF code and data range, the program `#PF`
  message and a section on the keyboard while the program runs. Version 1
  programs keep working.
- `abi/` crate: the contract constants (numbers, errors, limits), shared by
  the kernel and the runtime library.
- `runtime/` crate: the programs' runtime library (`entry!`,
  `print!`/`println!`, `read_line`, `exit`, a global allocator over
  `SYS_ALLOC` with the same `linked_list_allocator` as the kernel, and a
  `panic!` handler that writes `[panic] <message>` and exits with code 101).
- Example programs `eco` (reads a line, returns the text and counts the
  words with a `Vec`) and `falha_memoria` (writes to `0xdead_beef`).
- `GUIA_DO_PROGRAMADOR.md`: the guide for those who write programs for
  os-rust, with the complete code of the two example programs.
- `interrupts::push_scancode` (public, so tests can "type") and
  `shell::poll_keyboard` (the prompt's keyboard loop, extracted from
  `main.rs`).
- `tests/user_runtime.rs` (29 integration tests in ring 3: keyboard reading,
  memory, invalid `SYS_READ_LINE`, failure isolation, the prompt resuming
  the keyboard, the guide matching the code) and unit tests for
  `keyboard::read_line`, contract v2, the ELF limit, the program `#PF` text
  and the alignment of the entry stack.

### Changed

- `hello` and `crash` now use the runtime library (same observable
  behavior). The `asm!` of the `syscall` instruction, which Milestone 5's
  `hello` carried by hand, now lives in `runtime/src/sys.rs`.
- The message of a program terminated by `#PF` now says `encerrado por erro
  de memoria`; the other exceptions keep the Milestone 5 text.
- An ELF's segments are limited to `[0x4000_0000, 0x6000_0000)` (before they
  went up to the stack), so as not to land on the program's heap.
- The workspace gains the `runtime` and `abi` members; the kernel now
  depends on `abi`.
- Project version: `0.5.0` → `0.6.0`.

### Fixed

- The kernel stacks for entries coming from ring 3 and for the double fault
  had no declared alignment, and the top of the first could land on an
  address that is not a multiple of 16; waiting for a key inside a syscall
  exposed the problem. Both are now `#[repr(align(16))]`.

## [0.5.0] - 2026-09-28

Milestone 5: first user program. os-rust now executes code that is not the
kernel's, in user mode (ring 3): the prompt's `run hello` command loads a
program embedded in the boot image, which writes to the screen through a
system call and hands control back to the prompt.

### Added

- `run <name>` command in the prompt: executes an embedded user program;
  with no argument, or with an unknown name, it lists the available
  programs.
- User mode (ring 3): user code and data segments in the GDT and entry into
  ring 3 (`src/user.rs`), returning to the prompt when the program ends,
  whether by `exit` or by a failure.
- System call mechanism with the `syscall`/`sysret` instruction
  (`src/syscall.rs`) and the first two syscalls, `write` and `exit`.
- Syscall contract version 1 in a single file, `SYSCALLS.md` (numbers,
  registers, error codes, executable format, load region, initial stack);
  automated tests check that the text agrees with the code.
- Hand-written loader for static ELF64 executables (`src/elf.rs`), with
  per-segment permissions (W^X) and rejection of invalid ELFs without
  mapping anything.
- Reference programs `hello` (writes `Ola do ring 3!` and ends) and `crash`
  (executes an invalid instruction on purpose), in the new `programs/`
  crate, compiled for the new target `x86_64-os_rust_user.json`.
- The root `build.rs` compiles the user programs and embeds them in the
  kernel within the same `cargo run`/`cargo test`; a compile error in the
  program stops the build with the error in view.
- `tests/user_mode.rs` (16 integration tests in ring 3), unit tests for the
  ELF reader and for the contract, two tests of the `run` command and one of
  `Writer::write_bytes`.

### Changed

- The GDT and the TSS gain kernel data segments and user code and data
  segments, and a kernel stack for entries coming from ring 3.
- The frame allocator becomes global, with recycling of the frames of
  terminated programs.
- The exception handlers distinguish ring 3 from ring 0: a failure in a user
  program terminates only the program; in ring 0 they remain fatal, as
  before.
- `#DE`, `#SS` and `#NP` now have a handler (without them, the CPU would
  escalate to a double fault).
- The repository becomes a Cargo workspace (the kernel stays at the root;
  `programs/` is the second member).
- Project version: `0.4.2` → `0.5.0`.

## [0.4.2] - 2026-09-25

Cosmetic adjustment: no new kernel capability, no change in observable
behavior.

### Changed

- Code comments in `src/` and `tests/` (GDT/TSS, interrupts, memory, heap,
  serial, keyboard, prompt and all the integration tests): each one now
  states in full, in the comment itself, the reason for the decision it
  documents.
- Project version: `0.4.1` → `0.4.2`.

## [0.4.1] - 2026-09-25

Milestone 4.1: new identity — the project is now called `os-rust` (formerly
`proto-os`), and gains an ASCII logo on the boot screen and in the README.
Maintenance milestone: no new kernel capability.

### Added

- 20-line ASCII logo (`src/logo.txt`/`src/logo.rs`), drawn at the top of the
  screen (`vga_buffer::draw_logo`) before the identification and the prompt,
  at boot time — without sending anything to the serial. Translates each
  `█` character (UTF-8) to byte `0xDB` of code page 437 (VGA's full block),
  one character per block.
- The same logo, identical, at the top of `README.md`.
- `pub const NAME` in `src/lib.rs`, derived from `env!("CARGO_PKG_NAME")` —
  single source of the project name, alongside `VERSION` (which now derives
  name and version, not just the version).
- 4 new tests: logo dimensions (`src/logo.rs`), the logo in the first 20
  lines of the screen with `draw_logo()` isolated, the logo surviving intact
  through the complete boot sequence (identification + prompt already
  written — proves that nothing scrolls over it), and the identification
  compared again against the manifest (`tests/boot_integration.rs`).
- Milestone 4.1 in the Roadmap table and in "Milestone details" of
  `README.md`, between Milestone 4 and Milestone 5.
- New section in `WALKTHROUGH.md` explaining where name and version come
  from, why the `os-rust` package becomes the `os_rust` crate, why the name
  of the target's JSON file decides the name of the subfolder in `target/`,
  and why `█` in UTF-8 is not byte `0xDB` of code page 437.

### Changed

- Package name in `Cargo.toml`: `proto_os` → `os-rust` (the crate in Rust
  code becomes `os_rust`, with an underscore). Project version: `0.4.0` →
  `0.4.1`.
- Custom target specification file: `x86_64-proto_os.json` →
  `x86_64-os_rust.json` (same content); `.cargo/config.toml` updated.
- Repository root folder: `proto-os` → `os-rust`.
- Prompt: `proto-os> ` → `os-rust> `, derived from
  `env!("CARGO_PKG_NAME")` (same source as `NAME`/`VERSION`), never written
  by hand.
- Welcome message, the `sobre` command, the panic and fatal exception
  screens, and the first serial diagnostic line: all now cite `os-rust`,
  reading `NAME`/`VERSION` instead of fixed text.
- `README.md`, `WALKTHROUGH.md` and the previous entries in this file: text
  updated to `os-rust`, without changing any technical content.
- All references to the crate in `src/main.rs` and in each file of
  `tests/`: `proto_os::` → `os_rust::`.

## [0.4.0] - 2026-09-25

Milestone 4: protection — own GDT and TSS, handlers for the main
exceptions, `falha <type>` command and single version identification.

### Added

- Kernel's own GDT (`src/gdt.rs`): kernel code segment and TSS descriptor,
  loaded during boot before the IDT.
- TSS with a dedicated 20 KiB stack in the Interrupt Stack Table
  (`DOUBLE_FAULT_IST_INDEX`), used by the double fault handler — a kernel
  stack overflow no longer causes a triple fault and a silent QEMU restart.
- Handlers for the five main exceptions: breakpoint (`#BP`, already
  existing, now with the screen reduced to one line), invalid instruction
  (`#UD`), general protection (`#GP`), page fault (`#PF`) and double fault
  (`#DF`, now on the dedicated stack). The four fatal ones show a readable
  screen (screen + serial), in the style of the panic screen, with name,
  acronym, instruction address, error code (when there is one) and the
  os-rust version; the page fault screen also shows the fault address and
  the interpretation of the error code in words (read/write, page
  missing/protection violation).
- `falha <type>` command in the prompt: `pagina`, `pilha`, `opcode`,
  `protecao` and `breakpoint`, each one deliberately causing the
  corresponding exception; with no argument or with an unknown type, it
  lists the available types.
- Single version identification (`os_rust::VERSION`, derived from
  `Cargo.toml` at compile time), displayed in the welcome message, in the
  first serial diagnostic line, in the `sobre` command, and in every panic
  or fatal exception screen.
- New serial diagnostic line ("gdt/tss ativos") right after the GDT/TSS
  initialization.
- 7 new tests: 2 dedicated integration tests (`tests/double_fault.rs`,
  proving that the handler runs on the dedicated IST stack;
  `tests/page_fault.rs`, proving the expected fault address), and 5
  unit/integration tests for version and for the `falha` command
  (`src/shell.rs`, `tests/boot_integration.rs`).
- New chapter in `WALKTHROUGH.md` on CPU exceptions, GDT, TSS and the
  Interrupt Stack Table, the double fault with a dedicated stack, how to
  read the error code and the address of a page fault, and how the version
  gets from `Cargo.toml` to the screen.

### Changed

- Project version: `0.3.0` → `0.4.0`.
- `os_rust::init` now calls `gdt::init()` between `serial::init()` and
  `interrupts::init()`.
- The reentrancy guard of panic handling (`panic.rs`) is now shared with the
  fatal exception screen, through `panic::enter_fatal_handler()`.
- `vga_buffer::screen_contains` stops being exclusive to internal tests
  (`#[cfg(test)] pub(crate)`) and becomes `pub`, to be reused by integration
  tests in `tests/`.
- `README.md`: Milestone 4 marked as completed in the milestone table, in
  the details of each milestone and in the Status section; `falha <type>`
  command in the command table; project structure updated.

## [0.3.0] - 2026-09-25

Milestone 3: memory — physical frame allocator, paging and kernel heap.

### Added

- 4 KiB physical frame allocator (`BootInfoFrameAllocator`,
  `src/memory.rs`) from the memory map delivered by the bootloader.
- Translation of virtual to physical address and creation of new mappings
  in the active page table (`memory::translate_addr`, `memory::map_page`),
  using the complete physical memory mapping (the `bootloader`'s
  `map_physical_memory` feature).
- Kernel heap: fixed 100 KiB range (`src/allocator.rs`), mapped at boot,
  with a global allocator (`linked_list_allocator`) — `Box`, `Vec` and the
  rest of the `alloc` crate now work anywhere in the kernel.
- `mem` command in the prompt: shows the usable physical memory, the
  position/size of the heap, a `Box` with its value and address, and a `Vec`
  built from empty with size, capacity and sum.
- New serial diagnostic line at the end of memory initialization.
- 11 new tests: 3 integration tests (`tests/frame_allocator.rs`,
  `tests/paging.rs`, `tests/heap_allocation.rs`) and a unit test of the
  `mem` command in `src/shell.rs`.
- New chapter in `WALKTHROUGH.md` on physical memory, frames and pages, the
  4-level page table, creating mappings, and the heap.

### Changed

- Project version: `0.2.0` → `0.3.0`.
- `os_rust::init` now receives the `BootInfo` delivered by the bootloader
  (memory map and offset of the complete physical memory).
- `Cargo.toml`: new `linked_list_allocator` dependency; `bootloader` gains
  the `map_physical_memory` feature.
- `.cargo/config.toml`: `build-std` gains `"alloc"`.
- `README.md`: Milestone 3 marked as completed in the milestone table, in
  the details of each milestone and in the Status section; `mem` command in
  the command table; project structure updated.

## [0.2.0] - 2026-09-24

Milestone 2: debugging infrastructure and automated tests.

### Added

- Serial output (UART 16550, port `0x3F8`): the kernel now writes
  diagnostic messages about boot, breakpoint and double fault events, and
  panic handling also to the host terminal, without altering the QEMU
  screen.
- `cargo test`: the kernel compiles in test mode, boots in QEMU without a
  graphical window, runs the entire suite and reports success or failure to
  the host through the command's own exit code.
- 23 new tests: unit tests in `src/vga_buffer.rs`, `src/keyboard.rs`,
  `src/shell.rs`, `src/interrupts.rs` and `src/serial.rs`, plus two
  integration tests in `tests/` (`boot_integration.rs` and
  `should_panic.rs`).
- `src/serial.rs` (new module) and `src/lib.rs` (new — reorganization of
  the project into library + binary, necessary for the integration tests; no
  observable behavior change).
- New sections in `README.md` on how to run and interpret `cargo test`, and
  a new chapter in `WALKTHROUGH.md` on the serial port and the test runner.
- This file (`CHANGELOG.md`).

### Changed

- Project version: `0.1.0` → `0.2.0`.
- `README.md`: Milestones 1 and 2 marked as completed in the milestone
  table, in the details of each milestone and in the Status section; project
  structure updated.

## [0.1.0] - 2026-09-23

Milestone 0 (boot and VGA text) and Milestone 1 (interrupts, keyboard and
command prompt).

### Added

- BIOS boot straight into a `no_std`/`no_main` Rust binary, with no
  operating system underneath.
- Text writing to the VGA video buffer (`0xb8000`): welcome message, screen
  scrolling, hardware cursor.
- Panic handling with a readable message on screen.
- IDT, breakpoint and double fault handlers, reprogramming of the 8259 PIC
  with only IRQ1 (keyboard) enabled.
- Scancode translation (Scan Code Set 1, US QWERTY layout) and a fixed
  command prompt: `help`, `clear`, `echo`, `sobre`, `panic`.
