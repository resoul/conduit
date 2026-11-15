# cdt.fs: reading and buffered writing

## Reading lines

`fs.read_lines(path: String) -> [String] | FsError` reads a UTF-8 file into a list.
It removes LF and CRLF line endings, preserves empty lines, and keeps a final
line without a newline. A trailing line ending does not add an extra empty item;
an empty file produces an empty list. Invalid UTF-8 returns `FsError`.

The returned strings share an owned snapshot of the file's text. Each line has
its own reference-counted descriptor, but no separate copy of its characters.
Changing or removing the file does not change returned strings. String operations,
map keys, JSON encoding and `Writer.write_lines` work as with ordinary strings.

Keeping even one line alive retains the backing text of the whole file. Use
`fs.each_line` or `Reader.read_line` when processing a large file without retaining
its contents. Producing a changed string does not modify other lines.

## Buffered writing

```conduit
import * as fs from "cdt.fs"

func save(path: String, lines: [String]) -> Void | fs.FsError {
    writer := fs.open_write(path)?
    writer.write_lines(lines)?
    writer.flush()?
    writer.close()
}
```

`fs.open_write(path)` creates or truncates a file. `fs.open_append(path)` appends
to an existing file, creating it when necessary. Both return `Writer | FsError`
and use a 64 KiB buffer.

| Writer method | Result | Behavior |
| --- | --- | --- |
| `write(data: String)` | `Void \| FsError` | Writes the string unchanged. |
| `write_line(data: String)` | `Void \| FsError` | Writes the string followed by LF (`\n`). |
| `write_lines(lines: [String])` | `Void \| FsError` | Writes each string followed by LF, in order, in one native call. |
| `flush()` | `Void \| FsError` | Flushes buffered bytes to the underlying file. |
| `close()` | `Void` | Closes the writer. Call `flush()?` first to handle buffered write errors. |

`write_lines` leaves the input list and strings unchanged. An empty list writes
nothing. Empty strings produce blank lines; existing newlines inside strings are
preserved, including a trailing newline (another LF is still appended).

The method uses the existing writer buffer without joining the list into a new
string or flushing after each line. It stops on the first write error; an error
can leave a partially written batch. Errors from buffered data can surface later
in `flush()`. A closed writer returns `FsError`, including for an empty batch.
Flushing does not guarantee durable storage on disk.

For streaming producers, call `write_lines` with successive batches instead of
collecting the entire output in memory.
