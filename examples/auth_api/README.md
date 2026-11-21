# Conduit Auth API Showcase (`cdt.jwt` + `cdt.cache` + `cdt.lock` + `cdt.http` + `cdt.cron`)

End-to-end zero-dependency backend showcase combining:

* **`cdt.jwt`**: HS256 token issuance (`jwt.sign`) and verification middleware (`jwt.verify<Claims>`).
* **`cdt.cache`**: In-memory rate limiting (`incr`), stampede-safe profile caching (`get_or_set`), WAL persistence (`open_file` + `compact`), and atomic snapshots (`save` / `load`).
* **`cdt.lock`**: OS advisory file lock (`lock.file`) around append-only audit logging.
* **`cdt.http` & `cdt.validation`**: Typed route validation (`422`), `group "/api"` auth middleware (`401`), `on_start` / `on_stop` lifecycle hooks, `spawn` async login telemetry, and `cors "*"`.
* **`cdt.cron` & `cdt.time`**: Scheduled WAL compaction job (`schedule Maintenance`) and UTC ISO-8601 timestamps.

## Usage

```bash
cd examples/auth_api

# Type-check, inspect HTTP routes, export OpenAPI 3.0.3 & inspect cron schedule
conduit check main.cdt
conduit routes main.cdt
conduit openapi main.cdt --title "Conduit Auth API" --version "1.0.0"
conduit cron list main.cdt

# Run built-in .cdt tests
conduit test .

# Run offline self-test (exercises login, JWT middleware, rate limit, WAL, snapshot & file lock)
conduit run main.cdt
```

## Run the live server

The example serves one configured account. Set these variables in the server environment before running `conduit run main.cdt -- serve`.

| Variable | Value |
| --- | --- |
| `JWT_SECRET` | A persistent, randomly generated signing key of at least 32 characters. Generate one with `openssl rand -hex 32` and store it in your secret manager. |
| `AUTH_EMAIL` | Email of the account allowed to log in. |
| `AUTH_PASSWORD_HASH` | Encoded password hash for the account. Generate it with `crypto.hash_password(password)` and preserve the complete value, including the dollar signs. |
| `AUTH_ROLE` | Optional. Defaults to `member` and also accepts `admin`. |

When upgrading a running example, rotate `JWT_SECRET` to invalidate tokens issued by the previous login handler.

Send `email` and `password` to `POST /auth/login`. A successful login returns a one-hour bearer token carrying the configured role. Unknown accounts and incorrect passwords both receive HTTP 401. The offline self-test generates its own signing key and password hash.
