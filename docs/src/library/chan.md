# `cdt.chan`

Thread-safe bounded and unbounded FIFO channels (`Chan<T>`) for passing typed values between `spawn` threads, HTTP workers, and the main thread.

```conduit
import * as chan from "cdt.chan"

struct WorkItem {
    id: Int
    label: String
}

func main() -> Void {
    ch := chan.open<WorkItem>(8)

    spawn {
        for i in 3 {
            ch.send(WorkItem(id: i, label: "task-{i}"))
        }
        ch.close()
    }

    for item in ch {
        print("{item.id}: {item.label}")
    }
}
```

## Types & Constructors

* `Chan<T>` — Typed handle to a thread-safe FIFO channel (safe to capture in `spawn` and share across threads without JSON serialization).
* `chan.open<T>(capacity?: Int) -> Chan<T>` (also callable as `Chan<T>(capacity?)` or `chan.Chan<T>(capacity?)`):
  * If `capacity` is omitted or `<= 0`, creates an **unbounded** channel.
  * If `capacity > 0`, creates a **bounded** channel of up to `capacity` elements (`send` blocks when full until space becomes available or `close()` is called).
  * `Void` is not allowed as an element type.

## `Chan<T>` Methods

| Method | Signature | Description |
| :--- | :--- | :--- |
| `send` | `(value: T) -> Bool` | Enqueues `value` (blocking if bounded buffer is full); returns `false` if closed |
| `try_send` | `(value: T) -> Bool` | Non-blocking enqueue; returns `false` if closed or buffer is full |
| `recv` | `() -> T?` | Waits for and removes the next element, returning `nil` only after closed and drained |
| `try_recv` | `() -> T?` | Non-blocking receive; returns `nil` immediately if empty |
| `recv_timeout` | `(timeout_ms: Int) -> T?` | Waits up to `timeout_ms` milliseconds for an element (`nil` on timeout or closed & empty) |
| `close` | `() -> Void` | Marks the channel closed and wakes all waiting senders and receivers |
| `len` | `() -> Int` | Number of queued items currently in the channel |
| `cap` | `() -> Int` | Buffer capacity (`0` if unbounded) |
| `is_empty` | `() -> Bool` | `true` if `len() == 0` |
| `is_closed` | `() -> Bool` | `true` after `close()` has been called |

## Iterating with `for`

Use `for item in ch { ... }` (or `for _ in ch` / `for in ch`) to receive every item in order until the channel is closed and drained:

```conduit
import * as chan from "cdt.chan"

func sum_worker(ch: Chan<Int>, out: Chan<Int>) -> Void {
    var total = 0
    for n in ch {
        total = total + n
    }
    out.send(total)
}
```

## Declarative `worker` Blocks

Use `worker` to declare a typed channel consumer with built-in worker pools (`concurrency`), error retries (`retry`), and sleep delays (`backoff`)—without writing manual `spawn` loops:

```conduit
import * as chan from "cdt.chan"

error JobError {
    message: String
}

worker SendEmail(job: String, prefix: String = "[mail]") concurrency 4 retry 2 backoff 50ms -> Void | JobError {
    if job == "bad" {
        return JobError(message: "invalid recipient")
    }
    print("{prefix} sent {job}")
}

func main() -> Void {
    jobs := chan.open<String>(16)
    spawn {
        jobs.send("alice@example.com")
        jobs.send("bad")
        jobs.close()
    }

    processed := SendEmail(jobs)
    print("Succeeded: {processed}")
}
```

* Syntax: `[export] worker Name[<T, ...>](msg: MsgType, extra...: ExtraType [= default]) [concurrency N] [retry R] [backoff D<ms|s|m|h>] [-> Void | ErrorType ...] { ... }`.
* Inside the body, `msg` is each received value from the channel; you can use `?`, early `return`, and `defer` per item when returning `Void | Error`.
* Calling `Name(ch, ...extra)` consumes `Chan<MsgType>` until `ch` is closed and drained across `concurrency` workers (default `1`) and returns `Int` (the count of successfully completed items).
* If an item fails, it is retried up to `retry` times (default `0`), sleeping `backoff` between attempts. Permanently failing items are skipped and do not increment the returned count.
