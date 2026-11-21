# Standard Library Overview

Conduit includes a suite of standard libraries providing high-performance primitives implemented directly on the host engine.

All standard libraries reside in the reserved `cdt.*` namespace.

---

## Design Principles

1. **Host-Native Efficiency**: Standard library operations execute directly in Rust native modules (`src/native/`), avoiding bytecode overhead for JSON, time, and validation operations.
2. **Strict Type Encapsulation**: Host resources (such as monotonic clock measurements or system timestamps) are wrapped in opaque types (`time.Instant`, `time.Timestamp`, `time.Duration`). Users cannot forge or modify raw internal state, preventing undefined behavior.
3. **Explicit Error Modeling**: Standard operations that can fail return typed errors (such as `time.TimeError`, `json.DecodeError`, `validation.ValidationError`) rather than throwing exceptions or crashing the runtime.

---

## Available Modules

| Module | Namespace | Description |
| :--- | :--- | :--- |
| [cdt.json](json.md) | `cdt.json` | Typed JSON decoding (`decode<T>`), deterministic `encode`, and dynamic `json.Value` trees. |
| [cdt.time](time.md) | `cdt.time` | System clock, monotonic timestamps, high-resolution duration calculation, and epoch conversion. |
| [cdt.validation](validation.md) | `cdt.validation` | Schema validation, field rule annotations, issue aggregation, and standard rules (`min`, `max`, `email`). |
| [cdt.env](env.md) | `cdt.env` | Typed environment configuration (`@env`, `load<T>`), `get`/`require`, and CLI `args()`. |
| [cdt.cli](cli.md) | `cdt.cli` | Typed CLI parser & dispatcher (`cli { ... => handler }`, `cli.dispatch<T>()`, `@cli.*`), subcommands, and `--help`. |
| [cdt.fs](fs.md) | `cdt.fs` | Fast basic and buffered streaming file I/O (`read_text`, `write_atomic`, `each_line`, `Reader`, `Writer`). |
| [cdt.path](path.md) | `cdt.path` | Cross-platform lexical path manipulation (`join`, `dirname`, `basename`, `ext`, `stem`, `normalize`). |
| [cdt.process](process.md) | `cdt.process` | External command execution (`run`, `status`, `pipe`), working directory (`cwd`, `set_cwd`), `pid`, and `exit`. |
| [cdt.crypto](crypto.md) | `cdt.crypto` | Hardware-accelerated SHA-256/HMAC, PBKDF2-SHA256 password hashing, SIMD Base64, Hex, and OS-backed CSPRNG (`uuid_v4`, `random_int`). |
| [cdt.url](url.md) | `cdt.url` | URL parser (`Url`), RFC 3986 percent-encoding, and query-string encoding/decoding. |
| [cdt.csv](csv.md) | `cdt.csv` | RFC 4180 CSV parsing (`parse`, `parse_with`, `parse_records`) and serialization (`stringify`, `stringify_with`). |
| [cdt.math](math.md) | `cdt.math` | `Float` math functions (`sqrt`, `pow`, `sin`, `cos`, `ln`, `floor`, `ceil`, `round`, `hypot`) and constants (`pi`, `e`, `tau`). |
| [cdt.log](log.md) | `cdt.log` | Leveled `stderr` logger (`debug`, `info`, `warn`, `error`, `set_level`, `set_json`). |
| [cdt.http](http.md) | `cdt.http` | Zero-dependency HTTP/1.1 client (`get`, `post`, `request_timeout`, `get_bytes`, native `https://` TLS 1.2/1.3), declarative `router { ... }` (`http.build<T>()`, `http.serve<T>()`, typed `guard` blocks, `on_start`/`on_stop`), `Router` middleware, and graceful shutdown (`SIGINT`/`SIGTERM`). |
| [cdt.db](db.md) | `cdt.db` | Zero-dependency SQL driver for PostgreSQL and MySQL (`connect`, `exec`, `query<T>`, `@db.*` table models, transactions) and declarative `migration`s. |
| [cdt.cron](cron.md) | `cdt.cron` | Zero-dependency declarative `schedule` and `job` blocks, 5-field UTC cron & fixed-interval runner (`run<T>`, `start<T>`, `run_once<T>`). |
| [cdt.jwt](jwt.md) | `cdt.jwt` | Zero-dependency HS256/HS512 JWT signing (`sign`, `sign_alg`), verification (`verify<T>`, `verify_alg<T>`), and payload inspection. |
| [cdt.lock](lock.md) | `cdt.lock` | Process-wide named mutexes (`with`, `try_with`), writer-fair `RwLock` (`read`, `write`), and OS advisory file locks (`file`, `file_try`, `file_shared`, `Guard`). |
| [cdt.chan](chan.md) | `cdt.chan` | Thread-safe typed FIFO channels (`Chan<T>`, `chan.open<T>`, `send`/`try_send`, `recv`/`recv_timeout`, `close`, `for item in ch`) and declarative `worker` pools (`concurrency`, `retry`, `backoff`). |
| [cdt.cache](cache.md) | `cdt.cache` | Process-wide thread-safe TTL + LRU cache (`open`, `get_or_set`, `incr`) with atomic snapshots (`save`/`load`) and durable WAL (`open_file`/`compact`). |
| [cdt.redis](redis.md) | `cdt.redis` | Zero-dependency RESP2 TCP client (`connect`, KV, JSON, Hash, List, Set, raw `cmd`) and Lua-verified distributed locks (`lock`, `unlock`, `extend`, `with_lock`). |
| [cdt.app](app.md) | `cdt.app` | Unified service lifecycle coordinator (`app Name { config, http, schedule, shutdown, on start, on stop }`, `app.run<T>`, `app.start<T>`). |

