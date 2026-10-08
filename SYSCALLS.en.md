# os-rust syscall contract

**Contract version**: 5

**Valid since**: os-rust 0.9.0 (Milestone 9)

This document is everything a program needs to know to run on os-rust.
The kernel and the programs never depend on anything that is not here. An
**incompatible** change increases the contract version and requires
updating the programs' runtime library (`runtime/`) in the same milestone.
Those who write programs in Rust normally do not call the syscalls directly:
they use the library (see `GUIA_DO_PROGRAMADOR.md`).

## 1. Executable format

- ELF64, little-endian, `e_type = ET_EXEC` (2), `e_machine = EM_X86_64` (62).
  No relocation, no PIE, no dynamic libraries.
- Each `PT_LOAD` must have `p_vaddr` aligned to 4 KiB, `p_filesz ≤ p_memsz`
  (the remainder is zeroed, like `.bss`), and **not share a page** with
  another `PT_LOAD`. The other segment types are ignored.
- All `PT_LOAD`s must fit in `[0x4000_0000, 0x6000_0000)` (the code and data
  range, section 2). A segment outside it is rejected.
- Permissions: a page is writable only if the segment has `PF_W`; executable
  only if it has `PF_X` (W^X). The stack and the heap are writable and never
  executable.
- `e_entry` must be inside an executable segment.
- The executable is embedded in the boot image at compile time, or
  read from a file on a volume (section 9), and executed with
  `run <target> [<target>...]`: several programs can run at the same time
  (section 8). An executable read from a file is at most **65536 bytes**
  (64 KiB) and goes through the same validations; a file that is not a valid
  ELF64, or that exceeds this size, is rejected before any memory mapping.

## 2. Where the program lives

| Item | Value |
|------|-------|
| User region | `[0x4000_0000, 0x8000_0000)` (1 GiB to 2 GiB) |
| Code and data | What the `PT_LOAD`s declare, within `[0x4000_0000, 0x6000_0000)`. Project convention: base `0x4000_0000`. |
| Heap | `[0x6000_0000, 0x6010_0000)` (1 MiB at most), starts empty and grows with `SYS_ALLOC` (section 5). |
| Stack | 4 pages (16 KiB) at the end of the region: `[0x7FFF_C000, 0x8000_0000)`; the page below is not mapped (guard). |
| Anything else | Any access (kernel, VGA, physical memory, heap not yet requested, beyond the end of the heap) causes an exception and the program is terminated. |

Since the region is below 2 GiB, the program can be compiled with Rust's
default code model (`R_X86_64_32S` fits).

## 3. State at the first instruction

| Register | Value |
|----------|-------|
| `rip` | `e_entry` |
| `rsp` | **`0x7FFF_FFF8`** (top of the region − 8): System V ABI function-entry alignment, as if `_start` had been called; `[rsp]` does **not** contain a valid return address. |
| `rflags` | `0x202` (`IF = 1`, rest zero) |
| `rcx` | `e_entry` (the kernel puts it in the initial context, as if the program had just returned from a `syscall` at `e_entry`) |
| `r11` | `0x202` (likewise: the `rflags` of that return) |
| All the others (`rax`, `rbx`, `rdx`, `rsi`, `rdi`, `rbp`, `r8`–`r10`, `r12`–`r15`) | `0` |
| Segments | User code `0x23`, user data/stack `0x1B` (defined by the kernel). |

There is no `argc`/`argv`/environment: nothing on the initial stack.
The program **never returns** from `_start`: it must call `exit`. (The
runtime library takes care of this: the program's `main` returns and the
library calls `exit` with the returned value.)

## 4. How to call

The `syscall` instruction. Register convention:

| Register | On entry | On exit |
|----------|----------|---------|
| `rax` | syscall number | result (see §6) |
| `rdi` | argument 1 | preserved |
| `rsi` | argument 2 | preserved |
| `rdx` | argument 3 | preserved |
| `rcx` | (ignored) | **destroyed** (the CPU stores the return `rip` here) |
| `r11` | (ignored) | **destroyed** (the CPU stores the `rflags` here) |
| `r8`, `r9`, `r10` | (ignored, reserved for arguments 4 to 6) | preserved |
| `rbx`, `rbp`, `r12`–`r15`, `rsp` | | preserved |

During the syscall the kernel runs on its own stack; the program's stack is
never used nor trusted. Interrupts are disabled during the syscall: no
program switch happens in the middle of it, and therefore each call is
atomic with respect to the other programs. The kernel may return `rcx` and
`r11` intact, but the contract does not promise it: a program cannot depend
on their values on return.

Example, in Rust with `asm!`:

```rust
let ret: i64;
unsafe {
    core::arch::asm!(
        "syscall",
        inlateout("rax") 1u64 => ret,       // SYS_WRITE
        in("rdi") ptr, in("rsi") len,
        lateout("rcx") _, lateout("r11") _,
        options(nostack),
    );
}
```

## 5. Syscalls

Contract v5 numbers. **Any other number** (including `0`) terminates the
program (§7).

| No. | Name | Arguments | Result |
|-----|------|-----------|--------|
| 1 | `SYS_WRITE` | `rdi = ptr`, `rsi = len` | bytes written (`≥ 0`) or error (`< 0`) |
| 2 | `SYS_EXIT` | `rdi = code` | does not return |
| 3 | `SYS_READ_LINE` | `rdi = ptr`, `rsi = len` | bytes written (`≥ 1`), `0` if `len == 0`, or error (`< 0`) |
| 4 | `SYS_ALLOC` | `rdi = size` | address of the start of the new area (`> 0`) or error (`< 0`) |
| 5 | `SYS_YIELD` | none | `0` |
| 6 | `SYS_OPEN` | `rdi = path_ptr`, `rsi = path_len` | descriptor (`≥ 0`) or error (`< 0`) |
| 7 | `SYS_READ` | `rdi = fd`, `rsi = ptr`, `rdx = len` | bytes read (`≥ 0`; `0` = end of file) or error (`< 0`) |
| 8 | `SYS_CLOSE` | `rdi = fd` | `0` or error (`< 0`) |
| 9 | `SYS_READ_DIR` | `rdi = fd`, `rsi = ptr`, `rdx = len` | `1` (one entry written), `0` (end of directory) or error (`< 0`) |
| 10 | `SYS_TIME` | `rdi = ptr`, `rsi = len` | `0` (a `DateTime` written) or error (`< 0`) |

### `SYS_WRITE` (1)

Writes `len` bytes, starting at `ptr`, to the screen (VGA text), at the
current cursor, through the same path as the kernel's `print!`. Printable
ASCII bytes (`0x20`–`0x7e`) and `\n` appear as they are; any other byte
appears as the square `0xfe`. UTF-8 is not required.

- `len == 0`: returns `0` without looking at `ptr`.
- `len > 4096`: returns `ERR_INVAL`; nothing is written.
- The entire range `[ptr, ptr + len)` must be inside the user region and
  each page of it mapped and user-accessible; otherwise it returns
  `ERR_FAULT` and **nothing is written**. The program keeps running.
- Success: returns `len`.

### `SYS_EXIT` (2)

Terminates the program. `code` is recorded by the kernel: `0` means success
and shows nothing on screen; any other value shows
`[run] <name> terminou com codigo <code>`. Only this program ends: the
others, if any, keep running, and the prompt returns when the **last** one
ends. All of the program's pages (code, data, stack and heap) are freed
immediately and no program state survives the call.

### `SYS_READ_LINE` (3)

Waits for the person to type a line on the keyboard and delivers it to the
program. The program is **blocked** until Enter and, while waiting, **uses no
CPU**: the other programs keep running. While the line is being typed, the
kernel shows on screen what is typed (echo) and handles Backspace. With more
than one program waiting, the line goes to the one that asked **first**
(section 8).

- `len == 0`: returns `0` without waiting and without looking at `ptr`.
- `len > 4096`: returns `ERR_INVAL`; no key is consumed.
- The entire range `[ptr, ptr + len)` must be inside the user region and
  each page of it mapped, user-accessible **and writable**; otherwise it
  returns `ERR_FAULT` **before waiting**, without consuming any key. The
  program keeps running.
- Accepts printable ASCII characters (`0x20`–`0x7e`), at most
  `min(len, 128) − 1`. Anything beyond that is ignored, without echo (the
  kernel prompt behaves the same way). Backspace deletes the last character,
  if any. Any other key is ignored.
- On Enter, the kernel advances the line on screen and writes to `ptr` the
  typed characters **followed by `\n`** (`0x0a`). It returns `n`, the total
  bytes written: `1 ≤ n ≤ len`. An empty line returns `1` (just the `\n`).
  The bytes from `ptr + n` onward are not touched.
- Typed keys that no program asked for are kept (up to 16) and go to the
  prompt when the last program ends (section 8).

### `SYS_ALLOC` (4)

Gives memory to the program: extends its heap by `size` bytes.

- `size == 0`: returns `ERR_INVAL`.
- `size` is rounded up to a multiple of 4096. The new area is zeroed,
  readable and writable, never executable.
- Returns the address of the start of the new area. The areas from
  successive calls are **contiguous**: the first starts at `0x6000_0000` and
  each one starts where the previous one ended.
- If the program's total pages would exceed 256 (1 MiB), or if physical
  memory runs out, it returns `ERR_NOMEM` and **nothing is allocated** (a
  failing call does not change the heap). The program keeps running and may
  try a smaller request.
- There is no free: memory only returns to the kernel when the program ends.
  Reusing blocks within the heap is the program's job (the runtime library
  does this).

### `SYS_YIELD` (5)

Voluntarily gives up the CPU. The kernel saves the program's state and hands
the CPU to the next ready program, round-robin; the program runs again, at
the instruction after the `syscall`, when its turn comes. If no other
program is ready, it returns immediately to the same program. It never fails
and has no error code. All registers, except `rax` (which is `0` on return),
come back with the value they had.

In the runtime library: `yield_now()`. A program does not need to call it
for the others to run: the timer interrupts those that do not yield
(section 8).

### `SYS_OPEN` (6)

Opens a file **or directory**, read-only, and returns the descriptor: a
number from `0` to `3`, always the **lowest free one**. The read position
starts at `0`. The path (format in section 9) is copied by the kernel before
being interpreted.

- `path_len == 0`: returns `ERR_INVAL`. `path_len > 64`: `ERR_NAMETOOLONG`.
  Neither looks at `path_ptr`.
- The range `[path_ptr, path_ptr + path_len)` must be inside the user region
  and mapped; otherwise it returns `ERR_FAULT`. The program keeps running.
- Malformed path (empty component, invalid character, name outside 8.3, `.`
  or `..`, or no `/` at the start): `ERR_INVAL`. More than 8 components:
  `ERR_NAMETOOLONG`.
- Unknown or unavailable volume: `ERR_NODEV`. Nonexistent path: `ERR_NOENT`.
  A file in the middle of the path: `ERR_TYPE`.
- The program already has 4 open files: `ERR_MFILE`.
- Error reading the volume (disk that does not respond, corrupted
  structure): `ERR_IO`.

### `SYS_READ` (7)

Reads up to `len` bytes from the file open at `fd`, starting at its
position, into `ptr`, and advances the position by what it read. Returns the
number of bytes read, which is only less than `len` at the end of the file
(it never goes past its size); `0` means the file has ended. An empty file
returns `0` on the very first call.

- `len == 0`: returns `0` without looking at `ptr`. `len > 4096`:
  `ERR_INVAL`.
- `[ptr, ptr + len)` must be inside the user region, mapped and
  **writable**; otherwise `ERR_FAULT`, before reading anything from the
  volume.
- `fd` outside `0..=3`, never opened or already closed: `ERR_BADF`.
- `fd` of a directory: `ERR_TYPE`.
- If the read fails after already having delivered bytes in this call, the
  call returns the delivered bytes, and the **next** one returns the error
  (`ERR_IO`).

### `SYS_CLOSE` (8)

Closes the descriptor and releases it for reuse. Closing an already closed
descriptor, or one outside `0..=3`: `ERR_BADF`.

### `SYS_READ_DIR` (9)

Writes to `ptr` **one** entry of the directory open at `fd` (a
`DirEntryRaw`, 20 bytes, section 9) and advances the directory cursor.
Returns `1` when it wrote an entry and `0` when the entries have run out
(nothing is written).

- `len < 20` or `len > 4096`: `ERR_INVAL`.
- `[ptr, ptr + 20)` must be in the user region, mapped and **writable**;
  otherwise `ERR_FAULT`.
- Invalid `fd`: `ERR_BADF`. `fd` of a file: `ERR_TYPE`.
- Deleted entries, long-name entries, volume labels, `.` and `..` do not
  appear.

### `SYS_TIME` (10)

Writes to `ptr` the current date and time, **in UTC**, as a `DateTime`
(8 bytes, `repr(C)`). Returns `0`. Each call reads the clock again; two
consecutive calls never go backwards.

Buffer format:

| Offset | Size | Field |
|--------|------|-------|
| 0 | 2 | full year (for example, 2026), little-endian |
| 2 | 1 | month (1 to 12) |
| 3 | 1 | day (1 to the last day of the month, accounting for leap years) |
| 4 | 1 | hour (0 to 23) |
| 5 | 1 | minute (0 to 59) |
| 6 | 1 | second (0 to 59) |
| 7 | 1 | zero (alignment) |

The checks happen in this order:

- `len != 8` (`TIME_SIZE`): `ERR_INVAL`.
- `[ptr, ptr + 8)` must be in the user region, mapped and **writable**;
  otherwise `ERR_FAULT`.
- The clock returned impossible values, did not stabilize or did not
  respond: `ERR_CLOCK`. The program receives no date in this case.

There is no time zone: the result is always UTC. In the runtime library:
`time::now()`.

## 6. Result and error codes

The result in `rax` is a signed 64-bit integer: `≥ 0` is success (the
meaning depends on the syscall); `< 0` is an error, with the code below.

| Constant | Value | When |
|----------|-------|------|
| `ERR_FAULT` | `-1` | Invalid pointer or range (outside the region, unmapped, crossing unmapped pages, or not writable in `SYS_READ_LINE`, `SYS_READ`, `SYS_READ_DIR` and `SYS_TIME`). |
| `ERR_INVAL` | `-2` | Argument outside the allowed limits (`len > 4096`, `size == 0` in `SYS_ALLOC`, `path_len == 0`, malformed path, or `len != 8` in `SYS_TIME`). |
| `ERR_NOMEM` | `-3` | `SYS_ALLOC` could not give the requested memory (1 MiB heap limit or lack of physical memory). |
| `ERR_NOENT` | `-4` | The path, or a component of it, does not exist. |
| `ERR_NODEV` | `-5` | Unknown volume, or known but unavailable (no disk, invalid volume). |
| `ERR_TYPE` | `-6` | Wrong type: `SYS_READ` on a directory, `SYS_READ_DIR` on a file, or a file in the middle of the path. |
| `ERR_BADF` | `-7` | Invalid descriptor: outside `0..=3`, not open or already closed. |
| `ERR_MFILE` | `-8` | The program already has 4 open files. |
| `ERR_NAMETOOLONG` | `-9` | Path longer than 64 bytes, or with more than 8 components. |
| `ERR_IO` | `-10` | Error reading the volume: disk that does not respond, disk error or corrupted FAT structure. |
| `ERR_CLOCK` | `-11` | `SYS_TIME`: the clock returned impossible values, did not stabilize or did not respond. |

There is no errno: the program receives only the value in `rax`.

## 7. How a program ends

| Reason | Message on screen and serial | Afterwards |
|--------|------------------------------|------------|
| `SYS_EXIT(0)` | nothing on screen (serial logs it) | the prompt returns when the last program ends |
| `SYS_EXIT(n)`, `n ≠ 0` | `[run] <nome> terminou com codigo <n>` | same |
| Invalid memory access (`#PF`) in ring 3 | `[run] <nome> encerrado por erro de memoria: #PF (Page Fault) em <rip>` + `codigo de erro: <código>` + `endereco de falha: <endereço>` | same |
| Other CPU exception in ring 3 (`#DE`, `#UD`, `#GP`, `#SS`, `#NP`) | `[run] <nome> encerrado por erro: <sigla> (<nome da exceção>) em <rip>` (+ error code, when there is one) | same |
| Nonexistent syscall number (anything outside 1 to 10) | `[run] <nome> encerrado: syscall inexistente (<n>)` | same |

The message appears **at the moment** that program ends, on screen and
serial, even if others keep running. A program failure **never** brings down
the kernel or the other programs: only it is terminated and its pages are
freed; the prompt responds again when the last one ends, and the next
program runs normally. File syscall errors (nonexistent path, unavailable
volume, invalid descriptor, invalid pointer…) do **not** terminate the
program: it receives the error code and continues. The files the program
left open are closed when it ends, for any of the reasons above. Anything
that is not one of the lines above (for example, an infinite loop) is
outside the contract: there is no total time limit, only the slice of each
turn (section 8).

## 8. Several programs at the same time

`run <target> [<target>...]` loads all the requested programs before
starting any of them and only returns the prompt when the last one ends. A
`<target>` that starts with `/` is the path of an executable file (section
9); any other is the name of an embedded program, and the two kinds can be
mixed on the same line. The programs start in the given order. The same
program may appear more than once: each occurrence is an independent
instance.

What the program **can** assume:

- It has its own memory: the same address map as section 2, but no one else
  sees or alters those pages, and it does not see the others'.
- It resumes exactly where it stopped, with all registers, the stack and
  memory intact, after any switch (by `SYS_YIELD`, by timer or by keyboard
  wait).
- Each `SYS_WRITE` is delivered whole to the screen, without being split by
  another program's output. (The runtime library joins each
  `print!`/`println!` into a single call of up to 256 bytes, so a short line
  is never split.)
- `SYS_YIELD`, with more programs ready, passes the turn round-robin, in the
  order they were given to `run`.

What the program **cannot** assume:

- No execution order between programs beyond the `SYS_YIELD` round-robin. A
  program that yields the CPU soon (writes a line and yields, like `ping`)
  is only interrupted by the timer if it computes for 40 ms or more, and
  then its order is that of the round-robin. A program that never yields the
  CPU is interrupted by the timer at any instruction and resumed later.
- No exact timing: the slice is **5 ticks** and the timer runs at **100 Hz**
  (one tick every 10 ms). The timer only takes the CPU from a program after
  five ticks have been counted since it was put on the CPU, that is, after
  40 to 50 ms of continuous computation. Both values may change in future
  milestones.
- No means of communication between programs (it does not exist in this
  version).

Limit: at most **4** programs at the same time. Asking for more, a name that
does not exist, a path that could not be read, a file that is too large or
that is not a valid ELF64 rejects the whole `run` and **no** program is
started.

### Keyboard

The keyboard is delivered to the program that is waiting for a line in
`SYS_READ_LINE`. If more than one is waiting, the one that asked **first**
receives it; the others keep waiting and only start receiving afterwards. A
key never goes to two programs nor is lost. A waiting program consumes no
CPU.

The prompt does not read the keyboard while any program is alive. What no
program asked for is not lost: it stays in the keyboard queue (16 slots; if
it fills up, the oldest key is discarded) and the prompt receives it when
the last program ends, by `exit` or by error. A program does not end in the
middle of a line (it stays blocked in `SYS_READ_LINE` until Enter); what is
left over is type-ahead, which the prompt receives intact, and whatever is
typed afterwards appears in it without loss.

## 9. File system

The file system is **read-only**. There is no syscall to create, write,
rename or delete: no file changes while os-rust runs.

### Volumes

| Prefix | What it is | Available |
|--------|------------|-----------|
| `/ram` | FAT16 ramdisk embedded in the boot image | always |
| `/disco` | ATA disk (primary channel, slave) with a FAT16 volume | only if QEMU attached a disk with a valid volume |

An unavailable volume (no disk, device that is not an ATA disk, invalid boot
sector, read error) makes operations on it return `ERR_NODEV`; the other
volume keeps working and the kernel is not affected.

### Paths

- Format: `/<volume>/<component>[/<component>...]`. `/ram` and `/ram/` are
  the root of the volume.
- Case-insensitive, even in the volume prefix: `/DISCO/OLA.TXT` and
  `/disco/ola.txt` are the same file.
- Each component is an **8.3** name: 1 to 8 characters, and optionally a dot
  and up to 3 of extension, with letters, digits, `_`, `-`, `~` and `$`.
  There are no long names: FAT long-name entries are ignored.
- `.` and `..` are invalid (`ERR_INVAL`): no path leaves the volume.

### Limits

| Limit | Value |
|-------|-------|
| Path length | **64 bytes** |
| Components per path | **8** |
| Open files per program | **4** |
| Executable read from a file | **65536 bytes** |
| Entries walked per directory | 512 (a larger directory, or one with a cycle, is a read error) |

The names returned by `SYS_READ_DIR` come in lowercase. The volume is FAT16
(512 bytes per sector); the FAT is read as untrusted data: a cluster chain
with a cycle, that leaves the volume, or that is shorter than the file size
is `ERR_IO`, never an infinite loop.

### `DirEntryRaw`

`SYS_READ_DIR` writes 20 bytes (`#[repr(C)]`, `size` in little-endian):

| Offset | Size | Field |
|-------:|-----:|-------|
| 0 | 12 | `name`: `nome.ext` in lowercase, terminated and padded with zeros |
| 12 | 1 | `kind`: `1` file, `2` directory |
| 13 | 3 | zeros |
| 16 | 4 | `size`: bytes (`0` for a directory) |

### Each program has its own table

Descriptors and read positions belong **to the program**: two programs that
open the same file have independent positions, and a program never sees
another's descriptors. In the runtime library: `File::open`, `File::read`,
`Dir::open`, `Dir::next` (descriptors are closed automatically).

## 10. History

| Version | Milestone | Change |
|---------|-----------|--------|
| 1 | 5 (os-rust 0.5.0) | First version: `SYS_WRITE`, `SYS_EXIT`, `syscall`/`sysret`, static ELF64. |
| 2 | 6 (os-rust 0.6.0) | `SYS_READ_LINE` (3) and `SYS_ALLOC` (4); `ERR_NOMEM` (`-3`); heap window `[0x6000_0000, 0x6010_0000)`; ELF segments limited to `[0x4000_0000, 0x6000_0000)`; `#PF` message shows "erro de memoria"; section on the keyboard. v1 programs keep working. |
| 3 | 7 (os-rust 0.7.0) | `SYS_YIELD` (5); several programs at the same time (up to 4, each in its own memory, 5-tick slice, timer at 100 Hz); termination message at the moment the program ends; keyboard to the program that asked first. v1 and v2 programs keep working. |
| 4 | 8 (os-rust 0.8.0) | `SYS_OPEN` (6), `SYS_READ` (7), `SYS_CLOSE` (8), `SYS_READ_DIR` (9); errors `ERR_NOENT` (`-4`) to `ERR_IO` (`-10`); `/ram` and `/disco` volumes (FAT16, read-only); per-program file table (up to 4); `run` accepts file paths (executable of up to 64 KiB). v1, v2 and v3 programs keep working. |
| 5 | 9 (os-rust 0.9.0) | `SYS_TIME` (10); `ERR_CLOCK` error (`-11`); 8-byte `DateTime`, always in UTC. v1 to v4 programs keep working. |
