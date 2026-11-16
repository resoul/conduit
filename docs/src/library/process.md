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
| `cwd` | `() -> String \| ProcessError` | Current working directory |
| `set_cwd` | `(dir: String) -> Void \| ProcessError` | Changes current working directory |
| `pid` | `() -> Int` | Current OS process ID |
| `exit` | `(code: Int) -> Void` | Terminates the process immediately with `code` |
