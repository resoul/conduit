# `cdt.cache`

Process-wide, thread-safe in-memory key-value cache with TTL expiration, $O(\log N)$ LRU eviction, stampede-protected `get_or_set`, atomic Snapshots (`save`/`load`), and durable Write-Ahead Logging (`open_file`/`compact`).

Named caches opened via `cache.open(name)` or `cache.open_file(name, path)` are shared across HTTP router worker threads and `cdt.cron` background jobs within the same process.

```conduit
import * as cache from "cdt.cache"

struct Session {
    user_id: String
    role: String
}

func main() -> Void | cache.CacheError {
    sessions := cache.open("sessions", 1000)
    sessions.set_json("tok_1", Session(user_id: "u1", role: "admin"), 60000)?
    s := sessions.get_json<Session>("tok_1")?

    // Atomic rate-limiting window (60s TTL on first increment)
    hits := sessions.incr("rate:u1", 1, 60000)?

    // Stampede-safe singleflight population
    val := sessions.get_or_set("cfg", 30000, () => "loaded-once")?
    print("Hits={hits}, cfg={val}")
}
```

## Persistence Modes

1. **Snapshot (`save` / `load`)**: Atomically writes all non-expired entries (`{"version":1,"entries":[...]}`) using a sibling temp file, `sync_all()`, `rename`, and an advisory `<path>.lock` file lock. Expired entries are automatically discarded on `load`.
2. **WAL / Append-Only Log (`open_file` / `compact`)**: Every mutation appends a single compact JSONL record to disk and flushes immediately. On restart, `cache.open_file` replays live records and ignores expired ones. Call `c.compact()` anytime (and automatic compaction triggers past threshold) to rewrite the WAL cleanly without dead history.

## Types

* `Cache` — Opaque thread-safe handle to a named cache store.
* `CacheError { kind: String, message: String }` — Error kinds: `"invalid_input"`, `"invalid_value"`, `"overflow"`, `"io"`, `"corrupt_wal"`, `"corrupt_snapshot"`, `"encode"`, `"decode"`, `"dropped"`.

## Module Functions

| Function | Signature | Description |
| :--- | :--- | :--- |
| `open` | `(name: String, max_entries?: Int) -> Cache` | Opens or reuses a process-wide named cache (`0` = unbounded) |
| `open_file` | `(name: String, path: String, max_entries?: Int) -> Cache \| CacheError` | Opens a WAL-backed cache, replaying `path` on startup |
| `drop` | `(name: String) -> Bool` | Removes named cache from the process registry |

## `Cache` Methods

| Method | Signature | Description |
| :--- | :--- | :--- |
| `get` | `(key: String) -> String?` | Returns cached string or `nil` if absent/expired (updates LRU) |
| `set` | `(key: String, val: String, ttl_ms?: Int) -> Void \| CacheError` | Sets key (optional TTL in ms; `0`/omitted = no expiry) |
| `get_json<T>` | `(key: String) -> T? \| CacheError` | Fetches key and decodes JSON into `T?` |
| `set_json<T>` | `(key: String, val: T, ttl_ms?: Int) -> Void \| CacheError` | Encodes `val` to JSON and stores with optional TTL |
| `get_or_set` | `(key: String, ttl_ms: Int, cb: () -> String \| E) -> String \| E \| CacheError` | Returns cached hit or runs `cb()` under a per-key stampede lock (errors are not cached) |
| `incr` | `(key: String, delta?: Int, ttl_ms?: Int) -> Int \| CacheError` | Atomically increments integer key (preserving existing TTL; applying `ttl_ms` if newly created) |
| `del` | `(key: String) -> Bool \| CacheError` | Removes key; returns whether it existed |
| `has` | `(key: String) -> Bool` | Checks non-expired presence |
| `ttl` | `(key: String) -> Int?` | Remaining TTL in ms, `-1` if persistent, `nil` if missing/expired |
| `len` | `() -> Int` | Prunes expired keys and returns live entry count |
| `keys` | `(prefix?: String) -> [String]` | Returns sorted live keys matching optional prefix |
| `clear` | `() -> Void \| CacheError` | Clears all entries (and appends WAL `clear` if file-backed) |
| `prune` | `() -> Int` | Evicts all expired entries and returns count removed |
| `save` | `(path: String) -> Int \| CacheError` | Atomically saves snapshot to `path`, returning entry count |
| `load` | `(path: String) -> Int \| CacheError` | Loads snapshot from `path` (skipping expired entries) |
| `compact` | `() -> Int \| CacheError` | Atomically compacts WAL log to live keys only |
