# Conduit Redis Worker & Distributed Locks Demo (`cdt.redis` + `cdt.chan` + `cdt.validation` + `cdt.lock`)

Demonstrates zero-dependency RESP2 TCP communication with Redis (`cdt.redis`), declarative `worker` channel pools and `cdt.chan` (`Chan<T>`) concurrency, typed JSON payloads (`set_json` / `get_json<T>`), tagged-union data `enum`s, Lists/Sets/Hashes, Lua-verified distributed locks (`lock` / `extend` / `unlock` / `with_lock`), extended `@validation.uuid` & `@validation.one_of` checks, and `conduit test`.

## Offline Checks & Tests

```bash
cd examples/redis_worker
conduit check main.cdt
conduit test .
```

## Live Redis via Docker Compose

```bash
cd examples/redis_worker
docker compose up -d --wait

conduit run main.cdt
# or custom URL:
REDIS_URL="redis://:conduit_redis_secret@127.0.0.1:16379/0" conduit run main.cdt

docker compose down
```
