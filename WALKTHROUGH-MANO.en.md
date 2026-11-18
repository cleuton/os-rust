[VERSÃO EM PORTUGUÊS](WALKTHROUGH-MANO.md)

# How `mano` works, inside

This document explains `programs/src/bin/mano.rs`, the os-rust text editor, for people who
can program but have never written an editor. The user manual is in
[`MANO.en.md`](MANO.en.md); here the subject is the code. The file has about a thousand
lines and follows the same order as this text. Every function name quoted in backticks with
parentheses, like `divide()`, exists in the file. On-screen messages are Portuguese without
accents, because VGA text mode only shows ASCII; they are quoted as the program writes them.

## 1. Why it is a user program

An editor needs no privilege: it reads keys, writes to the screen, and reads and writes
files, and the kernel already offers all of that to any program through the contract
syscalls (`SYSCALLS.md`, version 9). So `mano` **did not change the kernel**: it runs in
ring 3, like `hello`, and an error in it (a `panic`, running out of memory) ends only it.

Through the runtime library (`runtime/`) the program uses these syscalls:

| What for | In the library | Syscall |
|---|---|---|
| wait for a key (arrows and Ctrl included), no echo | `read_key` | `SYS_READ_KEY` |
| clear the screen, move the cursor, choose the color | `screen::clear`, `screen::move_to`, `screen::set_color` | `SYS_SCREEN` |
| write the bytes of a screen line | `runtime::sys::write` | `SYS_WRITE` |
| read the `exec` arguments | `args::read` | `SYS_ARGS` |
| open and read a file | `File::open`, `File::read` | `SYS_OPEN`, `SYS_READ`, `SYS_CLOSE` |
| write a file from scratch | `FileWriter::overwrite`, `write_all` | `SYS_OPEN_WRITE`, `SYS_WRITE_FILE` |

The program starts with `#![no_std]` and `#![no_main]`, uses `extern crate alloc` to get
`Vec` and `String` (the runtime provides the allocator) and hooks `main` to the entry point
with `entry!(main)`.

## 2. The editor state

Everything the editor knows fits in one `struct Editor`:

- `linhas`: the text, as a list of lines (`Vec<Vec<u8>>`), each **without** the `\n`. It is
  never empty: a text with nothing is a list with one empty line. Lines hold **bytes**, not
  characters, so the editor can take any file.
- `soma`: the sum of the sizes of all the lines, kept up to date on every edit, to measure
  the text without adding everything again.
- `cy` and `cx`: the cursor line and column (in bytes). `cx` can equal the line length,
  which is the cursor just after the last byte.
- `desejada`: the column `Up` and `Down` try to return to. If you are at column 10 and go
  down to a 3-column line, the cursor goes to column 3; going down again, it returns to 10.
  Only horizontal movement keys, editing keys, search and jump-to-line change it.
- `topo` and `esquerda`: the first visible line and column (the two windows).
- `nome`, `modificado` and `mensagem`: the current path (if any), the `Modificado` flag and
  the text of line 22, which lasts until the next key.
- `ultimo`: the last searched or replaced term, remembered until the program ends.
- `area` and `corte_seguido`: the clipboard (the cut lines) and the flag that the previous
  key was a `Ctrl+K` that cut.
- `destaque`: the span (line, column, size) shown in reverse video.

The other types are small: `Fluxo` (continue or quit), `Limite` (which text limit an
operation would exceed), `Resposta` (`Sim`, `Nao`, `Todos`, `Cancela`), `Rodape` (which
shortcut panel to draw) and `ErroAbrir` (why a file did not open).

`novo()` builds an editor with the given text, the cursor at the start and nothing modified.

## 3. The main loop

`main` does, in this order:

1. Reads the arguments with `args::read`. No word: new text. One: it is the file. More than
   one: a usage error. Usage errors (`uso()` writes `mano: <message>` and returns code 1) are
   decided **before** any screen call, so the prompt comes back clean.
2. Opens the file with `abre()`. If it does not exist, it starts an empty text with that name
   and the message `Novo arquivo` (like `nano`); the file only comes to exist when saved.
3. Clears the screen and draws.
4. Repeats: waits for a key (`read_key` blocks without spending CPU), hands it to `trata()`,
   adjusts the windows with `rola()` and redraws with `redesenha()`.
5. On exit, `screen::reset_color` and `screen::clear` leave the screen clean and in the
   default color, and `main` returns 0.

Input fields and one-key questions are **not states**: `campo()` and `pergunta()` are nested
loops that also call `read_key` and draw, and only return when there is an answer. So
`Ctrl+O` is, in essence, "call `campo()`, and with what comes back, call `grava()`", with no
state machine.

## 4. Drawing the screen, line by line

The kernel **erases nothing by itself** and only shows printable ASCII. So the editor redraws
all 25 lines on every key, with no optimization, and each line is padded with spaces up to
column 79 so nothing is left over from the previous key.

`desenha()` paints, top to bottom:

- `desenha_titulo()`: line 0, 80 bytes in reverse video: ` mano  `, the name (or `Novo
  buffer`) and, if the text changed, `Modificado` at columns 69 to 78.
- `desenha_texto()`: lines 1 to 21. Each one is an 80-byte window on the stack (`[u8; 80]`),
  filled with spaces (`em_branco()`) and with the piece of the text line starting at
  `esquerda` (`copia()`), and written **in one go**. If the line holds the `destaque`, it goes
  out in up to three writes: before, the span in reverse video, and after.
- `desenha_mensagem()`: line 22, in yellow.
- `desenha_rodape()`: lines 23 and 24, two shortcut panels. `desenha_quadro()` writes each
  shortcut in 13 columns, the key in reverse video (`^O`) and the label in light gray.

Finally `desenha()` puts the hardware cursor in place (`vai_para()`): in the text, or at the
end of the field on line 22.

**Why line 24 can never go past column 79.** The kernel only breaks the line when a new
character arrives with the column already at 80. Filling column 79 of line 24 does not
scroll the screen, but the 81st byte would, and the whole screen would move up one line. So
every line has **exactly** 80 bytes and `desenha_quadro()` pads with spaces up to column 79,
never beyond.

**Bytes that are not text.** The file can have any byte (tabs, the `\r` of a CRLF file,
UTF-8). The editor writes raw bytes with `escreve()` (the `write` syscall, not `print!`, which
only takes text) and it is the kernel that swaps anything that is not printable ASCII for the
square `0xfe`. Each byte takes one column, so the editor's column arithmetic stays right.

The colors are the three constant pairs `CORPO`, `REVERSO` and `MENSAGEM`, chosen with
`cor()`.

## 5. Key dispatch and how Ctrl arrives

`read_key` returns a `KeyEvent`: the key (`Key::Char('a')`, `Key::Enter`, `Key::Left`, ...) and
a boolean `ctrl`. **Ctrl arrives as a lowercase letter plus a bit**: `Ctrl+X` is
`Key::Char('x')` with `ctrl: true`. So `Ctrl+H` and `Backspace` are never confused (one is
`Char('h')` with `ctrl`, the other is `Key::Backspace`), and `Ctrl+\`, `Ctrl+_` and `Ctrl+/`
arrive as `Char('\\')`, `Char('_')` and `Char('/')` with `ctrl`.

`trata()` is a `match` on the (key, ctrl) pair, one group of keys at a time: text, movement,
cut and paste, search and replace, file. Everything that is not in the table falls into
`_ => {}`: **it does nothing**. Before the `match`, `trata()` clears the message and the
highlight (they last until the next key) and remembers whether the previous key was a cut
(only `Ctrl+K` continues a sequence of cuts).

## 6. Editing lines: insert, split, join

- `insere()`: `Vec::insert` of the byte at `cx`, and the cursor advances.
- `tabula()`: inserts 1 to 4 spaces, up to the next column that is a multiple of 4. The editor
  never inserts byte `0x09`, because the kernel would show it as `0xfe`.
- `parte_linha()` (`Enter`): `split_off(cx)` cuts the line at the cursor and the rest becomes a
  new line right below.
- `apaga_esquerda()` and `apaga_direita()`: delete one byte; at the start (or end) of the line
  they **join** with the neighbor (`remove` the line and `extend_from_slice` into the other).
  At the start (or end) of the file they do nothing, and do **not** mark the text modified.

The text only becomes `Modificado` when the list of lines really changes.

## 7. Moving the cursor and scrolling

`esquerda()`, `direita()`, `inicio()` and `fim()` change `cx` (and `cy` at the edges) and set the
desired column. `cima()`, `baixo()` and `pagina()` change `cy` and take `cx` to `min(desejada,
line length)` **without changing `desejada`**; `PageUp` and `PageDown` move 20 lines, limited to
the first and the last.

`rola()` moves the windows **by the minimum**: if the cursor left through the top, `topo = cy`;
through the bottom, `topo = cy - 20`; the same, horizontally, with `esquerda` and column 79. There
is no line wrapping: a 200-column line only shows up piece by piece.

`mostra_posicao()` (`Ctrl+C`) and `vai_para_linha()` (`Ctrl+_`) complete the group.

## 8. Cut and paste with accumulation

`corta_linha()` takes the cursor line out and puts it in `area`. If the previous key was also a cut
(`corte_seguido`), the line is **appended**; otherwise `area` is emptied first: that is what makes
three `Ctrl+K` in a row a single sequence. `cola()` **clones** the lines of `area` above the cursor
line and does not empty the area, so you can paste many times.

## 9. Search

`busca()` asks for the term with `pede_termo()` (which shows the last term in brackets and, with an
empty field, reuses it) and looks with `acha()`, which finds the term in one line from a column. The
search starts **one byte after the cursor** (otherwise a repeated `Ctrl+W` would always find the same
span): the rest of the current line, then the following lines and, if nothing was found, one lap from
the start, with the message `Busca reiniciada`. Found: the cursor goes to the start of the match and
the `destaque` shows it in reverse video until the next key.

## 10. Replace

`substitui()` walks the text **once, from start to end**, with a (line, column) pair. At each match
the cursor goes to it, the `destaque` marks it and `pergunta()` decides. After a replacement the next
search on the same line starts at `column + new text size`: **the new text is never searched again**.
That is what makes `a` by `aa` end (each original `a` becomes `aa` once) and what keeps several
matches on the same line right, even when the new text has a different size. `A` turns on a flag that
skips the following questions. Before each replacement the editor checks the text limit (section 12):
if it would be exceeded, the walk **ends** with the limit message.

## 11. Opening and saving files

**Opening.** `le_arquivo()` reads the **whole** file in 4096-byte pieces (the maximum for one `read`)
and closes it when the function returns, because closing before saving is required: saving to a file
that is still open gives `ERR_BUSY`. Then `divide()` cuts the bytes at every `\n`, **discarding
nothing**: `a\nb\n` becomes `a`, `b` and an empty line; an empty file becomes one empty line. `abre()`
puts the two together and checks the limits.

**Saving.** `grava()` validates the name (`nome_valido()`: it starts with `/`) before opening anything;
if the name differs from the current one, it opens the file to see whether it exists (and asks before
overwriting); builds the bytes with `junta()` (the lines joined by `\n`, and a final `\n` if the last
line is not empty) and writes them with `grava_arquivo()`, which uses `FileWriter::overwrite` and
`write_all`. `grava_pedindo_nome()` (`Ctrl+O`) and `grava_direto()` (`Ctrl+S`) choose whether to ask
for the name. `sai()` (`Ctrl+X`) asks before discarding changes.

**The final `\n` and the empty file.** Since `divide()` and `junta()` are inverses, every empty file
or one ending in `\n` comes back identical; one that did not end in `\n` gets one. A text of one
empty line writes 0 bytes.

**The risk of writing over a zeroed file.** `FileWriter::overwrite` empties the file **before**
writing, and os-rust has no atomic write (temporary file and swap). If the write fails halfway, the
file ends up shorter or empty. The editor's defense is to keep the whole text in memory and **not**
clear the `Modificado` flag until the write has gone well.

## 12. The limits and why they exist

The text has at most 65536 **written** bytes and 4096 lines (the constants `MAX_BYTES` and
`MAX_LINHAS`). They exist so the editor fits in the program's 1 MiB heap with room to spare (the
text, the clipboard and the read and write buffers together add up to much less) and so a file is
never opened without being able to write it back.

`medida()` computes the written size (the sum of the lines, one `\n` between them and another at the
end if the last line is not empty), and `cabe()` checks it. **Every operation that grows the text**
(typing, `Enter`, `Tab`, paste, insert file, replace) computes the size of the text that **would
result** and calls `cabe()` **before** changing anything; if it would be exceeded, `recusa()` shows
the message (`Limite de 65536 bytes atingido` or `Limite de 4096 linhas atingido`) and nothing changes.

## 13. Function by function map

| Function | What it does |
|---|---|
| `divide()` | cuts a file's bytes into lines, at every `\n` |
| `junta()` | joins the lines with `\n`, plus a final `\n` if the last is not empty |
| `medida()` | the size the text would have when written |
| `cabe()` | checks both limits on the text that would result |
| `acha()` | finds the term in a line, from a column |
| `nome_valido()` | the path starts with `/` |
| `texto_do_limite()` | the message for an exceeded limit |
| `le_arquivo()` | reads the whole file and closes it |
| `abre()` | reads, splits and checks the limits |
| `grava_arquivo()` | writes the bytes from scratch |
| `escreve()`, `vai_para()`, `cor()` | the three screen gestures: write bytes, move the cursor, choose the color |
| `em_branco()`, `copia()` | the 80-byte window and filling it |
| `desenha_mensagem()`, `desenha_quadro()`, `desenha_rodape()` | line 22 and the shortcut panels |
| `desenha_linha_de_ajuda()`, `desenha_ajuda()` | the `Ctrl+G` screen |
| `uso()` | writes a usage error before the screen and returns 1 |
| `novo()` | creates the editor |
| `ultima_vazia()` | is the last line of the text empty? |
| `rola()` | moves the windows by the minimum |
| `desenha_titulo()`, `desenha_texto()`, `desenha()`, `redesenha()` | the 25 screen lines |
| `campo()`, `pergunta()`, `pede_termo()` | the prompts: text field, one-key question, search term |
| `recusa()` | shows which limit was reached |
| `insere()`, `tabula()`, `parte_linha()` | character, `Tab`, `Enter` |
| `apaga_esquerda()`, `apaga_direita()` | `Backspace`, `Delete` (join lines) |
| `esquerda()`, `direita()`, `cima()`, `baixo()`, `inicio()`, `fim()`, `pagina()` | movement |
| `mostra_posicao()`, `vai_para_linha()` | `Ctrl+C`, `Ctrl+_` |
| `corta_linha()`, `cola()`, `insere_arquivo()` | `Ctrl+K`, `Ctrl+U`, `Ctrl+R` |
| `busca()`, `substitui()` | `Ctrl+W`, `Ctrl+\` and `Ctrl+T` |
| `grava()`, `grava_pedindo_nome()`, `grava_direto()`, `sai()` | save and quit |
| `ajuda()` | `Ctrl+G`: shows the help and waits for a key |
| `trata()` | key dispatch |
| `main()` | arguments, open, main loop and exit |
