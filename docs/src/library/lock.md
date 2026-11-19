# `cdt.lock`

Process-wide concurrency synchronization (`Mutex`, writer-fair `RwLock`) and inter-process OS advisory file locks (`std::fs::File::lock*`).

Scoped callback helpers (`lock.with`, `lock.file`, `lock.read`, `lock.write`) guarantee automatic unlock even when returning early or propagating errors with `?`.

```conduit
import * as lock from "cdt.lock"
import * as fs from "cdt.fs"

func append_audit(entry: String) -> Void | lock.LockError | fs.FsError {
    lock.file("./audit.lock", () => {
        return fs.append_text("./audit.log", "{entry}\n")
    })?
}

func update_counter(name: String) -> Int | lock.LockError {
    return lock.try_with(name, 200, () => 42)
}
```

## Types

* `Guard` — Opaque handle returned by manual `acquire*` functions.
  * `g.release() -> Void` — Explicitly releases the held lock.
  * `g.is_held() -> Bool` — Returns `true` until `.release()` is called.
* `LockError { kind: String, message: String }` — Error kinds:
  * `"timeout"` — `try_with` or `try_acquire` timed out waiting for the mutex.
  * `"busy"` — Lock is already held (`timeout_ms == 0`, `file_try`, or `file_try_acquire`).
  * `"invalid_input"` — Negative `timeout_ms`.
  * `"io"` — Underlying OS file-lock error.

## Scoped Functions (Recommended)

| Function | Signature | Description |
| :--- | :--- | :--- |
| `with` | `(name: String, cb: () -> R) -> R \| LockError` | Acquires named in-process mutex, runs `cb()`, unlocks automatically |
| `try_with` | `(name: String, timeout_ms: Int, cb: () -> R) -> R \| LockError` | Acquires named mutex within `timeout_ms` (`0` = non-blocking try) |
| `read` | `(name: String, cb: () -> R) -> R \| LockError` | Acquires shared reader access on named in-process `RwLock` |
| `write` | `(name: String, cb: () -> R) -> R \| LockError` | Acquires exclusive writer access (writer-fair) on named `RwLock` |
| `file` | `(path: String, cb: () -> R) -> R \| LockError` | Acquires blocking exclusive OS advisory lock on `path` |
| `file_try` | `(path: String, cb: () -> R) -> R \| LockError` | Non-blocking exclusive OS lock on `path` (`LockError("busy")` if held) |
| `file_shared` | `(path: String, cb: () -> R) -> R \| LockError` | Blocking shared OS reader lock on `path` |

## Manual Guard Functions

| Function | Signature | Description |
| :--- | :--- | :--- |
| `acquire` | `(name: String) -> Guard` | Acquires blocking named in-process mutex |
| `try_acquire` | `(name: String, timeout_ms: Int) -> Guard \| LockError` | Acquires named mutex within `timeout_ms` |
| `file_acquire` | `(path: String) -> Guard \| LockError` | Acquires blocking exclusive OS file lock |
| `file_try_acquire` | `(path: String) -> Guard \| LockError` | Non-blocking exclusive OS file lock |
