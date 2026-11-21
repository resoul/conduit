# Conduit Examples

Runnable projects demonstrating the Conduit standard library, DSL blocks, and CLI tooling (`conduit check`, `conduit test`, `conduit routes`, `conduit openapi`, `conduit run`, `conduit db`, `conduit cron`).

| Example | Highlights | Offline Run / Test |
| :--- | :--- | :--- |
| [cli_http](cli_http/README.md) | `cli` DSL, `config` + `conduit env example`, HTTP/1.1 pooled `http.Client` & `router` (`cors`, `group`, `on_start`/`on_stop`, `@http.query`, `@http.status`), `spawn` + cross-thread `cdt.cache`/`cdt.lock`, `process.pipe`, `conduit routes` & `conduit openapi`, `\|>` pipelines, `@validation.*`, `fs`, `conduit test` | `conduit test cli_http` / `conduit routes cli_http/server.cdt` |
| [db_service](db_service/README.md) | PostgreSQL & MySQL wire drivers (`cdt.db`), `defer conn.close()`, `@db.*` models (`update`, `count`, `find_by`, `query_one`), `migration`s, PBKDF2 passwords (`cdt.crypto`), `cdt.time` ISO UTC | `conduit test db_service` (plus `docker compose` for live DB) |
| [auth_api](auth_api/README.md) | Typed `guard` JWT auth (`cdt.jwt`), `on_start`/`on_stop` router hooks, `spawn` async telemetry, `conduit routes` & `conduit openapi`, `cdt.cache` (rate limit, `get_or_set`, WAL `open_file`/`compact`, Snapshot `save`/`load`), OS `cdt.lock`, top-level `job` (`cdt.cron`), `conduit test` | `conduit test auth_api` / `conduit run auth_api/main.cdt` |
| [redis_worker](redis_worker/README.md) | Zero-dependency RESP2 `cdt.redis` client, `defer r.close()`, tagged-union data `enum JobPayload`, JSON/List/Set/Hash queues, Lua distributed locks (`lock`, `with_lock`), `@validation.uuid`, `cdt.lock`, `conduit test` | `conduit test redis_worker` (plus `docker compose` for live Redis 7) |

Run all built-in `.cdt` tests across every example at once:

```bash
conduit test examples
```
