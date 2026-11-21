# `cdt.process`

Run external commands, inspect working directory, and exit with a status code.

```conduit
import * as process from "cdt.process"

func run_git_status() -> String | process.ProcessError {
    out := process.run("git", ["status", "--short"])?
    if out.code != 0 {
        return process.ProcessError("non_zero", out.stderr)
    }
    return out.stdout
}
```

## Types

* `Output { code: Int, stdout: String, stderr: String }`
* `ProcessError { kind: String, message: String }` (`"not_found"`, `"permission_denied"`, `"spawn_failed"`, `"invalid_input"`, `"io"`)

## API

| Function | Signature | Description |
| :--- | :--- | :--- |
| `run` | `(cmd: String, args: [String]) -> Output \| ProcessError` | Runs `cmd` and captures exit code, `stdout`, and `stderr` |
| `status` | `(cmd: String, args: [String]) -> Int \| ProcessError` | Runs `cmd` inheriting stdio and returns exit code |
| `pipe` | `(commands: [[String]]) -> Output \| ProcessError` | Runs a pipeline of commands (`cmd1 \| cmd2 \| ...`) without a shell and captures the final `Output` |
| `cwd` | `() -> String \| ProcessError` | Current working directory |
| `set_cwd` | `(dir: String) -> Void \| ProcessError` | Changes current working directory |
| `pid` | `() -> Int` | Current OS process ID |
| `exit` | `(code: Int) -> Void` | Terminates the process immediately with `code` |

## Pipelines (`process.pipe`)

Use `process.pipe` to stream `stdout` from one command directly into the `stdin` of the next through OS pipes—without invoking a shell or buffering intermediate outputs in memory:

```conduit
import * as process from "cdt.process"

func top_authors(dir: String) -> String | process.ProcessError {
    out := process.pipe([
        ["git", "-C", dir, "log", "--format=%an"],
        ["sort"],
        ["uniq", "-c"],
        ["sort", "-rn"],
        ["head", "-n", "5"],
    ])?
    return out.stdout
}
```

- Each element of `commands` is `[program, arg1, arg2, ...]` and must be non-empty.
- Arguments are passed directly to the OS (no shell interpolation), so spaces and special characters in paths or values are safe.
- `Output` contains `code`, `stdout`, and `stderr` of the **last** command in the pipeline.
