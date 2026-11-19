# Conduit Auth API Showcase (`cdt.jwt` + `cdt.cache` + `cdt.lock` + `cdt.http` + `cdt.cron`)

End-to-end zero-dependency backend showcase combining:

* **`cdt.jwt`**: HS256 token issuance (`jwt.sign`) and verification middleware (`jwt.verify<Claims>`).
* **`cdt.cache`**: In-memory rate limiting (`incr`), stampede-safe profile caching (`get_or_set`), WAL persistence (`open_file` + `compact`), and atomic snapshots (`save` / `load`).
* **`cdt.lock`**: OS advisory file lock (`lock.file`) around append-only audit logging.
* **`cdt.http` & `cdt.validation`**: Typed route validation (`422`), `group "/api"` auth middleware (`401`), and `cors "*"`.
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

# Or start the live HTTP server
conduit run main.cdt -- serve
```
