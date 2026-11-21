# Control Flow

Conduit provides structured control flow mechanisms designed for clear, linear execution paths and concise transformations.

---

## Conditionals (`if` / `else`)

Conditionals evaluate a boolean condition:

```conduit
if score >= 100 {
    print("Top tier")
} else {
    print("Normal tier")
}
```

The condition must evaluate strictly to `Bool`. Conduit does not perform implicit truthy/falsy conversions on integers or strings.

---

## Loops (`for` / `in` and `while`)

The `for` loop iterates over integer ranges (`start..end` or `N` for `0..N`, like Go 1.22), lists, `Map<K, V>` entries (in deterministic insertion order), and `Chan<T>` channels, with optional tuple destructuring or wildcard form (`for in N` / `for range N`):

```conduit
for i in 5 {
    print(i) // 0, 1, 2, 3, 4
}

for in 3 {
    print("tick") // repeat 3 times without a loop variable
}

for range 3 {
    print("also repeats 3 times")
}

for item in [1, 2, 3, 4, 5] {
    if item == 2 {
        continue // Skip to next iteration
    }
    if item > 4 {
        break    // Terminate loop
    }
    print(item)
}

for (idx, word) in ["a", "b"].enumerate() {
    print("{idx}: {word}")
}

for (k, v) in Map.from([("x", 1), ("y", 2)]) {
    print("{k}={v}")
}
```

Use `while` loops for condition-controlled iteration (`Bool` condition required):

```conduit
var attempt = 0
while attempt < 3 {
    attempt = attempt + 1
}
```

- `break`: Immediately exits the nearest enclosing loop.
- `continue`: Skips the remainder of the current iteration and advances to the next element.

Using `break` or `continue` outside a loop body results in a compile-time error.

---

## Guard Clauses (`guard ... else`)

The `guard` statement enforces preconditions at the beginning of functions, ensuring invalid states are handled immediately before running core logic:

```conduit
func check_positive(n: Int) -> Int {
    guard n > 0 else {
        return 0 // Must terminate or return from the enclosing block
    }
    return n
}
```

> [!IMPORTANT]
> The `else` block of a `guard` statement **must** exit the current function or control flow (e.g., via `return`, `break`, or `continue`). Falling through an `else` block without exiting is diagnosed as a compile-time error.

---

## Deferred Cleanup (`defer`)

The `defer` statement registers an expression or block to run when the enclosing function exits:

```conduit
func handle_conn(db: db.Conn) -> Void | db.DbError {
    defer db.close()

    tx := db.begin()?
    defer {
        print("finishing handler")
    }

    // Both defers run in LIFO order on normal exit, early `return`, or `?` propagation
}
```

- **LIFO order**: Defers run in reverse order of registration (most recently executed `defer` runs first).
- **All exit paths**: Defers trigger on normal function fallthrough, explicit `return` (after evaluating the return value), and `?` error propagation.
- **Compile-time restrictions**: `return`, `?`, and nested `defer` statements are not allowed inside a `defer` body.

---

## Background Tasks (`spawn`)

Use `spawn` to launch a concurrent background task on a dedicated OS thread without blocking the current function or HTTP request handler:

```conduit
import * as cache from "cdt.cache"
import * as time from "cdt.time"

func send_welcome_async(user_id: String) -> Void {
    spawn {
        _s := time.sleep(50)
        c := cache.open("audit")
        _r := c.set(user_id, "welcomed")
    }
}

func notify(msg: String) -> Void {
    print(msg)
}

func main() -> Void {
    spawn notify("running in background")
}
```

- **Forms**: Both block form (`spawn { ... }`) and single-expression form (`spawn send_email(id)`) are supported.
- **Lexical captures**: Captured values are copied by value into the background VM (thread-safe shared primitives such as `cdt.cache`, `cdt.lock`, `cdt.db.Pool`, `cdt.http.Client`, and `cdt.http.Router` handles share underlying host state).
- **Graceful draining**: Before a program exits or an `http.serve` server shuts down and runs `on stop` hooks, the runtime waits up to 5 seconds for active `spawn` tasks to finish (or up to the configured `shutdown <duration>` inside a declarative [`app`](../library/app.md) block).
- **Compile-time isolation**: Loop control (`break`, `continue`) cannot cross a `spawn` boundary, and `return` inside `spawn` returns from the spawned task (`Void`) rather than the enclosing function.

### Passing Data Between Threads

Conduit threads do not share mutable local variables directly. You have three complementary tools for coordinating work between threads:

1. **Declarative Worker Pools (`worker`)** — Top-level [`worker`](../library/chan.md#declarative-worker-blocks) declarations automatically spawn `concurrency` worker threads reading from `Chan<T>`, with built-in `retry` attempts and `backoff` delays.
2. **Channels (`cdt.chan` / `Chan<T>`)** — Go-style thread-safe FIFO queues for typed messaging with blocking `send`/`recv`, non-blocking `try_send`/`try_recv`, `recv_timeout(ms)`, and `for item in ch` iteration:

```conduit
import * as chan from "cdt.chan"

struct WorkerResult {
    worker: String
    total: Int
}

func main() -> Void {
    ch := chan.open<WorkerResult>(4)

    spawn {
        for i in 3 {
            ch.send(WorkerResult(worker: "w{i}", total: (i + 1) * 10))
        }
        ch.close()
    }

    for res in ch {
        print("{res.worker} = {res.total}")
    }
}
```

3. **Shared In-Memory Cache (`cdt.cache` + `cdt.lock`)** — Ideal for shared counters, rate limits, and stateful key-value stores accessed concurrently across handlers and `spawn` tasks.

---

## Pipeline Operator (`|>`)

The pipe operator `|>` passes the result of the left-hand expression as the first argument to the right-hand function:

```conduit
func double(x: Int) -> Int {
    return x * 2
}

func add_ten(x: Int) -> Int {
    return x + 10
}

// 5 |> double (10) |> add_ten (20)
result := 5 |> double |> add_ten
```

Pipelines can be written with or without parentheses on single-argument functions:

```conduit
"Hello, Pipeline!" |> print()
```

---

## Runnable Example

```conduit
func double(x: Int) -> Int {
    return x * 2
}

func check_positive(n: Int) -> Int {
    guard n > 0 else {
        return 0
    }
    return n
}

func main() -> Void {
    for item in [1, 2, 3, 4, 5] {
        if item == 2 {
            continue
        }
        if item > 4 {
            break
        }
        print(item)
    }
    print(check_positive(-5))
    print(5 |> double)
}
```

### Running the Example

```bash
conduit run control_flow.cdt
```

Output:

```text
1
3
4
0
10
```

