# Conduit Examples

Runnable projects demonstrating the Conduit standard library, DSL blocks, and CLI tooling (`conduit check`, `conduit test`, `conduit run`, `conduit db`, `conduit cron`).

| Example | Highlights | Offline Run / Test |
| :--- | :--- | :--- |
| [cli_http](cli_http/README.md) | `cli` DSL, HTTP/1.1 client & `router` (`cors`, `group`), `|>` pipelines, `env.load` + extended `@validation.*`, `fs`, `conduit test` | `conduit test cli_http` / `conduit run cli_http/main.cdt -- self-test` |
| [db_service](db_service/README.md) | PostgreSQL & MySQL wire drivers (`cdt.db`), `@db.*` models (`update`, `count`, `find_by`, `query_one`), `migration`s, PBKDF2 passwords (`cdt.crypto`), `cdt.time` ISO UTC | `conduit test db_service` (plus `docker compose` for live DB) |
| [auth_api](auth_api/README.md) | HS256 `cdt.jwt` auth middleware, `cdt.cache` (rate limit, `get_or_set`, WAL `open_file`/`compact`, Snapshot `save`/`load`), OS `cdt.lock`, `cdt.cron`, `conduit test` | `conduit test auth_api` / `conduit run auth_api/main.cdt` |
| [redis_worker](redis_worker/README.md) | Zero-dependency RESP2 `cdt.redis` client, JSON/List/Set/Hash queues, Lua distributed locks (`lock`, `with_lock`), `@validation.uuid`, `cdt.lock`, `conduit test` | `conduit test redis_worker` (plus `docker compose` for live Redis 7) |

Run all built-in `.cdt` tests across every example at once:

```bash
conduit test examples
```
