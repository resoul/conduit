# `cdt.redis`

Zero-dependency RESP2 TCP Redis client with string, JSON, Hash, List, Set, raw command, and Lua-verified distributed lock APIs.

```conduit
import * as redis from "cdt.redis"

struct JobPayload {
    id: Int
    kind: String
}

func main() -> Void | redis.RedisError {
    r := redis.connect("redis://127.0.0.1:6379/0")?
    r.ping()?

    r.set_json("job:1", JobPayload(id: 1, kind: "email"), 300)?
    loaded := r.get_json<JobPayload>("job:1")?

    n := r.with_lock("lock:job:1", 5000, () => {
        return r.incr("jobs:processed")
    })?

    r.close()
}
```

## Connection URL Format

* `redis://[:password@]host[:port][/db][?timeout_ms=5000]`
* `rediss://[:password@]host[:port][/db][?tls_verify=full|ca|none&ca_file=/path/to/ca.pem]`
* `redis://username:password@host:6379/1` (ACL `AUTH user pass` + `SELECT 1`)
* Non-loopback connections using plaintext `redis://` or `tls_verify=none` emit a one-time warning per host; use `rediss://` (default `tls_verify=full`, or `tls_verify=ca&ca_file=...` for internal CAs) in non-local environments (Conduit 0.4.0 will enforce verified TLS by default for non-loopback hosts).

## Types

* `Client` — Thread-safe Redis connection handle.
* `Lock { key: String, token: String }` — Distributed lock handle acquired via `Client.lock(key, ttl_ms)`.
  * `l.unlock() -> Bool | RedisError` — Safely unlocks only if the stored value still equals `token`.
  * `l.extend(ttl_ms: Int) -> Bool | RedisError` — Refreshes TTL only if still owner.
* `RedisError { kind: String, message: String }` — Error kinds: `"invalid_url"`, `"connect"`, `"auth"`, `"io"`, `"protocol"`, `"redis_error"`, `"encode"`, `"decode"`, `"invalid_input"`, `"lock_busy"`, `"closed"`.

## Module Functions

* `redis.connect(url: String) -> Client | RedisError`

## `Client` Methods

### Connection & Raw
* `ping() -> String | RedisError`
* `cmd(args: [String]) -> String? | RedisError` — Sends any raw RESP2 command (arrays formatted as JSON).
* `close() -> Void`

### Keys & Strings / JSON / Counters
* `get(key: String) -> String? | RedisError`
* `set(key: String, val: String) -> Void | RedisError`
* `set_ex(key: String, val: String, ttl_secs: Int) -> Void | RedisError`
* `set_nx(key: String, val: String, ttl_secs?: Int) -> Bool | RedisError`
* `del(key: String) -> Bool | RedisError`
* `exists(key: String) -> Bool | RedisError`
* `expire(key: String, ttl_secs: Int) -> Bool | RedisError`
* `ttl(key: String) -> Int | RedisError`
* `incr(key: String) -> Int | RedisError`
* `incr_by(key: String, delta: Int) -> Int | RedisError`
* `decr(key: String) -> Int | RedisError`
* `keys(pattern: String) -> [String] | RedisError`
* `get_json<T>(key: String) -> T? | RedisError`
* `set_json<T>(key: String, val: T, ttl_secs?: Int) -> Void | RedisError`

### Hashes, Lists & Sets
* `hset(key: String, field: String, val: String) -> Bool | RedisError`
* `hget(key: String, field: String) -> String? | RedisError`
* `hgetall(key: String) -> Map<String, String> | RedisError`
* `hdel(key: String, field: String) -> Bool | RedisError`
* `lpush(key: String, val: String) -> Int | RedisError`
* `rpush(key: String, val: String) -> Int | RedisError`
* `lpop(key: String) -> String? | RedisError`
* `rpop(key: String) -> String? | RedisError`
* `lrange(key: String, start: Int, stop: Int) -> [String] | RedisError`
* `sadd(key: String, member: String) -> Bool | RedisError`
* `srem(key: String, member: String) -> Bool | RedisError`
* `smembers(key: String) -> [String] | RedisError`
* `sismember(key: String, member: String) -> Bool | RedisError`

### Distributed Locks
* `lock(key: String, ttl_ms: Int) -> Lock? | RedisError` — Executes `SET key <random_token> NX PX ttl_ms`. Returns `nil` if lock is already held.
* `unlock(lock: Lock) -> Bool | RedisError` — Lua atomic compare-and-delete.
* `extend(lock: Lock, ttl_ms: Int) -> Bool | RedisError` — Lua atomic compare-and-`PEXPIRE`.
* `with_lock(key: String, ttl_ms: Int, cb: () -> R) -> R | RedisError` — Acquires lock (`"lock_busy"` error if unavailable), releases internal connection mutex so `cb()` can run commands on the same `Client`, and always runs Lua unlock afterward.
