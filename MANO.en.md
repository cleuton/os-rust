[VERSÃO EM PORTUGUÊS](MANO.md)

# `mano`: the os-rust text editor

`mano` is a simple full-screen text editor in the style of `nano`. It is a **user
program** (it runs in ring 3, like `hello` or `teclas`) and it is built into the boot
image: after `cargo run` it is ready, with no file to copy, no FTP and no dependency on
`/disco`. To see how it was written, read [`WALKTHROUGH-MANO.en.md`](WALKTHROUGH-MANO.en.md).

The messages `mano` shows on screen are in Portuguese without accents (the VGA text mode
only shows ASCII), like the rest of os-rust. They are quoted here exactly as the program
writes them.

## What it is, and what it is not

`mano` does the basics: type, delete, move the cursor, cut and paste lines, search,
search and replace, insert a file, save and quit. The shortcuts are the `nano` ones
whenever the os-rust keyboard allows it.

It has **no** undo, text selection, copy, justify, spell checker, several files at once,
backward search, case-insensitive search, regular expressions, auto-indent or line
wrapping. It does not use Alt, F1 to F12 or Caps Lock, because the kernel keyboard does
not deliver those keys.

## How to open it

```text
os-rust> run mano                    (new text, no name)
os-rust> exec mano                   (the same)
os-rust> exec mano /ram/nota.txt     (opens the file; if it does not exist, starts a new one with that name)
```

- `run` does not pass arguments; to open a file use `exec`.
- The path is **absolute** (starts with `/`), has at most **40 bytes** and follows the
  os-rust file system format: 8.3 names, at most 8 components (`/ram/nota.txt`,
  `/disco/docs/leiame.txt`). There is no current directory.
- If the file does not exist, `mano` opens an empty text **with that name** and shows
  `Novo arquivo`. The file only comes to exist when you save.
- One word only: `exec mano /ram/a /ram/b` gives `mano: uso: exec mano [arquivo]`.
- Run `mano` **alone**: it uses the whole screen, and the kernel has only one cursor. Two
  programs drawing at the same time mix up the screen.

Usage errors (relative path, path over 40 bytes, a directory, a file that is too big, a
path outside the format) appear as **one line** `mano: <message>`, the kernel shows
`[run] mano terminou com codigo 1`, and the prompt comes back as usual. No file is
created or changed.

## The screen

```text
 mano  /ram/nota.txt                                              Modificado        <- line 0: title
um                                                                                  <- lines 1 to 21:
dois                                                                                   the text
tres
...
                                                                                    <- line 22: messages
^G Ajuda     ^O Gravar    ^W Buscar    ^K Cortar    ^C Posicao   ^X Sair            <- line 23: shortcuts
^R Ler       ^\ Subst.    ^U Colar     ^_ Linha     ^Y Pag ant   ^V Pag seg         <- line 24: shortcuts
```

| Region | Lines | What it shows |
|---|---|---|
| Title bar | 0 | ` mano  ` and the file name (or `Novo buffer`), in reverse video; `Modificado` on the right when the text changed since the last save |
| Text | 1 to 21 | 21 lines of 80 columns; the text found by a search appears in reverse video |
| Messages | 22 | the current message (gone at the next key), or the question and the field you type |
| Shortcuts | 23 and 24 | six shortcuts per line, with the key in reverse video |

Long lines do **not** wrap: when the cursor goes past column 79 the window scrolls
horizontally by the minimum needed (all lines together). Going back to the start of the
line scrolls it back.

## The shortcuts

`Ctrl+X` means "hold Ctrl and press X". With Ctrl, a letter counts as lowercase.

| Key | What it does |
|---|---|
| printable character | inserts at the cursor |
| `Enter` | splits the line at the cursor (no auto-indent) |
| `Backspace`, `Ctrl+H` | deletes to the left; at the start of a line, joins it with the previous one |
| `Delete`, `Ctrl+D` | deletes under the cursor; at the end of a line, joins it with the next one |
| `Tab` | inserts spaces up to the next column that is a multiple of 4 (`mano` never writes byte `0x09`) |
| arrows, `Ctrl+B`, `Ctrl+F`, `Ctrl+P`, `Ctrl+N` | move the cursor (left, right, up, down) |
| `Home`, `Ctrl+A` | start of the line |
| `End`, `Ctrl+E` | end of the line |
| `PageUp`, `Ctrl+Y` | 20 lines up |
| `PageDown`, `Ctrl+V` | 20 lines down |
| `Ctrl+_`, `Ctrl+/` | go to a line (asks for the number) |
| `Ctrl+C` | shows `linha L/T, coluna C/N`; inside a field or question, it cancels |
| `Ctrl+K` | cuts the whole line |
| `Ctrl+U` | pastes the cut lines, above the cursor line |
| `Ctrl+W` | search |
| `Ctrl+\`, `Ctrl+T` | search and replace |
| `Ctrl+R` | insert a file at the cursor |
| `Ctrl+O` | save (asks for the name) |
| `Ctrl+S` | save (without asking, if the text already has a name) |
| `Ctrl+X` | quit |
| `Ctrl+G` | help |

Any other key (`Insert`, `Esc` outside a field, `Ctrl+Z`, ...) **does nothing**.

**Why `Ctrl+T`?** `Ctrl+\` needs the backslash key, which sits in different places on
different physical keyboards, and the kernel only understands the US layout, by key
position. `Ctrl+T` does the same and is always at hand. (In `nano`, `Ctrl+T` does something
else, which `mano` does not have.) To go to a line there are `Ctrl+_` (Shift and hyphen)
and `Ctrl+/`.

### Cut and paste

`Ctrl+K` cuts the cursor line. Several `Ctrl+K` **in a row** accumulate the lines in the
clipboard, in order; any other key ends the sequence, and the next `Ctrl+K` starts a new
clipboard. `Ctrl+U` pastes the lines above the cursor line and does **not** empty the
clipboard: you can paste many times. Cutting the only line of a text leaves it empty;
`Ctrl+K` on a text that is already a single empty line does nothing.

## Search

1. `Ctrl+W`. Line 22 shows `Buscar: `.
2. Type the term and press `Enter`.

The search looks for the **exact** text (upper and lower case are different), forward,
from **one byte after the cursor**, going around the file at most once. Found: the cursor
goes to the start of the match, which stays in reverse video until the next key. Went
past the end and wrapped to the start: `Busca reiniciada`. Not found: `"<term>" nao
encontrado`.

To find the **next** match, press `Ctrl+W` and `Enter` with the field empty: the prompt
shows the last term (`Buscar [dois]: `) and `Enter` repeats it.

Example, with `um`, `dois`, `tres`, `dois`: `Ctrl+W`, `dois`, `Enter` goes to line 2;
`Ctrl+W`, `Enter` goes to line 4; once more, it wraps back to line 2 with `Busca
reiniciada`.

## Search and replace

1. `Ctrl+\` (or `Ctrl+T`).
2. The term (the last one appears in brackets; an empty `Enter` uses it).
3. `Substituir por: `, the new text (it may be empty).

`mano` walks through the **whole file, from start to end, once**, wherever the cursor was.
At each match it shows it in reverse video and asks:

```text
Substituir este? (S sim, N nao, A todos, Ctrl+C cancela)
```

`S` replaces this one; `N` skips it; `A` replaces this one and all the following without
asking; `Ctrl+C` or `Esc` stop (what was already replaced **stays**). The new text is
**never searched again**: `a` by `aa` ends. At the end: `Substituicoes: N` or `Nenhuma
ocorrencia`.

Example, with `gato`, `casa gato`, `rato`: `Ctrl+\`, `gato`, `Enter`, `cao`, `Enter`, `S`,
`N` replaces only the first one and shows `Substituicoes: 1`.

## Save and quit

- **`Ctrl+O`** asks for the name (already filled with the current one). If the name is
  **different from the current one** and the file **already exists**, it asks `Arquivo
  existe. Sobrescrever? (S sim, N nao)`. When it saves it shows `Gravadas N linhas`, where
  N is the number of line breaks written. **`Ctrl+S`** saves directly, without asking, if
  the text already has a name.
- **`Ctrl+X`** quits at once if nothing changed. If something changed, it asks `Gravar o
  texto alterado? (S sim, N nao, Ctrl+C cancela)`: `S` saves and quits (if the text has no
  name, it asks for one; it only quits if it saved), `N` quits without saving, `Ctrl+C` or
  `Esc` go back to editing.
- On quit, `mano` clears the screen and the prompt comes back as after `clear`.

**The final `\n`.** On open, the file is split at every `\n`. On save, the lines are joined
with `\n` and, if the last one is not empty, a `\n` is added at the end (like `nano`). So
an empty file, or one that ends in `\n`, comes back **identical**, byte for byte, and a
file that did not end in `\n` gets one. A 0-byte file stays at 0 bytes (`Gravadas 0
linhas`). A text with only `ola` is written as `ola\n`.

**Saving empties the file before writing.** The os-rust file system has no atomic write:
if saving fails halfway (for example, a full volume), the file may end up shorter or
empty. The text stays whole on screen and stays `Modificado`, so you can try again
somewhere else.

## Limits

- The text has at most **65536 bytes** (counted as the file would be written, with one
  `\n` per line) and **4096 lines**. A bigger file is refused on open. A keystroke, paste,
  file insertion or replacement that would go past the limit is refused with `Limite de
  65536 bytes atingido` (or `... 4096 linhas ...`) and **nothing changes**.
- Paths of up to **40 bytes**, absolute, 8.3 names.
- Only **printable ASCII** comes in from the keyboard. File bytes the kernel does not print
  (tabs, the `\r` of a CRLF file, UTF-8 accents) are **preserved**, take one column each
  and show as the square `■` (byte `0xfe`); they come back unchanged on save.
- No Alt, no undo, no selection.
- A file open in another program cannot be saved (`em uso`).

## Differences from `nano`

| `nano` | `mano` |
|---|---|
| undo, redo, selection, copy | do not exist |
| `Alt+...`, `F1` to `F12` | do not exist (the kernel does not deliver them) |
| `Ctrl+T` runs commands | `Ctrl+T` does the same as `Ctrl+\` (search and replace) |
| search with options (case, backward, regular expressions) | exact text only, forward |
| replace asks one by one, or all | the same (`S`, `N`, `A`) |
| soft wrap of long lines | the window scrolls horizontally |
| several files open | one |
| opens any size | up to 65536 bytes and 4096 lines |

## Common problems

- **`mano: use um caminho absoluto, como /ram/nota.txt`**: there is no current directory.
  Write the whole path, starting with `/`.
- **`Erro ao gravar: em uso`**: another program has the file open. Wait for it to finish.
- **`Erro ao gravar: sem espaco`**: the volume is full. The file may have ended up shorter;
  the text is still on screen.
- **`Erro ao gravar: caminho invalido`**: the name has a component longer than 8 characters,
  an extension longer than 3, or an invalid character.
- **`/disco` forgets what is saved.** `cargo run` starts QEMU with `snapshot=on`: what is
  written to `/disco` disappears when QEMU closes. To keep it, see "Keeping the disk
  between boots" in the [README](README.en.md).
- **`Ctrl+\` does not work on my keyboard.** The kernel understands the US layout, by key
  position. Use `Ctrl+T`.
- **Accents turn into `■`.** VGA text mode only shows ASCII. The bytes stay intact in the
  file.

## Taking an edited file out

`/ram` goes back to its initial state on every boot. To get a file out of os-rust, use the
FTP server: the "FTP server" section of the [README](README.en.md) shows how to download
`/ram/nota.txt` with `curl`.
