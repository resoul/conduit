# `cdt.app`

Zero-dependency declarative service coordinator unifying configuration loading & validation, HTTP serving, background cron scheduling, OS signal handling (`SIGINT`/`SIGTERM`), spawned task draining, and reverse-order (`LIFO`) shutdown hooks.

```conduit
import * as app from "cdt.app"
import * as http from "cdt.http"
import * as cron from "cdt.cron"
import * as log from "cdt.log"
import * as validation from "cdt.validation"

config AppConfig {
    port: Int @validation.min(1024) = 8080
}

func health(_req: http.Request) -> http.Response {
    return http.text("ok")
}

router Api {
    get "/health" => health
}

func tick() -> Void {
    log.debug("tick")
}

schedule Jobs {
    interval 10s => tick
}

func init_resources() -> Void {
    log.info("Starting service")
}

func cleanup_resources() -> Void {
    log.info("Shutdown complete")
}

app BillingService {
    config AppConfig
    http Api on "0.0.0.0:8080"
    schedule Jobs
    shutdown 10s
    on start => init_resources
    on stop  => cleanup_resources
}

export func main() -> Void | app.AppError {
    app.run<BillingService>()?
}
```

## Declarative `app` Clauses

| Clause | Description |
| :--- | :--- |
| `config <Struct>` | Loads environment variables (`@env` / `config`) and checks `@validation.*` rules before starting subsystems. Returns `AppError(kind: "config_error", ...)` on failure. |
| `http <Router> on "<addr>"` | Builds the declarative `router` and serves HTTP/1.1 on `<addr>`. Optional; omit for headless workers/schedulers. |
| `schedule <Schedule>` | Starts the declarative `schedule` in a background thread and stops it gracefully during shutdown. |
| `shutdown <duration>` | Maximum wait time for background `spawn` tasks and `worker` pools during shutdown (`ms`, `s`, `m`, `h`; default `5s`). |
| `on start => <fn>` | Startup hook `() -> Void` or `() -> Void \| Error`. Multiple hooks run in declaration order. |
| `on stop => <fn>` | Shutdown hook `() -> Void` or `() -> Void \| Error`. Multiple hooks run in **reverse declaration order (LIFO)** after HTTP, schedule, and spawned tasks drain. |

## Lifecycle Order

1. Load and validate `config` (fail-fast: exits with `"config_error"` before running hooks or opening sockets).
2. Run `on start` hooks in declaration order.
3. Start background `schedule` (if configured).
4. Start `http` listener on `<addr>` (if configured), or wait for `SIGINT`/`SIGTERM` when headless.
5. On signal (`SIGINT`/`SIGTERM`) or `.stop()`: stop accepting new HTTP requests, stop scheduler ticks, wait for active `spawn` / `worker` tasks up to `shutdown`, then run `on stop` hooks in reverse order.

## Types, Functions & Methods

* `App` — Handle returned by `app.start<T>()` for programmatic lifecycle control (useful in integration tests).
* `AppError { kind: String, message: String }` (`"config_error"`, `"startup_error"`, `"bind_error"`, `"schedule_error"`, `"shutdown_error"`).

| Symbol | Signature | Description |
| :--- | :--- | :--- |
| `app.run<T>` | `() -> Void \| AppError` | Runs `app` `T` in the foreground until `SIGINT`/`SIGTERM` and shuts down cleanly |
| `app.start<T>` | `() -> App \| AppError` | Validates `config`, runs `on start`, and launches `schedule` / `http` in the background |
| `App.stop` | `() -> Void \| AppError` | Stops the running `App`, waits for active tasks (`shutdown`), and runs `on stop` hooks |
| `App.is_running` | `() -> Bool` | Returns `true` while the app is active |

## Headless Mode & Testing with `app.start<T>()`

Omitting the `http` clause creates a headless service ideal for background workers, schedulers, and automated `conduit test` suites that verify lifecycle hooks without binding TCP ports:

```conduit
import * as app from "cdt.app"
import * as time from "cdt.time"

app WorkerApp {
    config AppConfig
    schedule Jobs
    shutdown 2s
    on start => init_resources
    on stop  => cleanup_resources
}

test "headless lifecycle starts and drains cleanly" {
    svc := app.start<WorkerApp>()?
    assert(svc.is_running())

    time.sleep(100)?
    svc.stop()?
    assert(!svc.is_running())
}
```

During shutdown (or `svc.stop()`), the coordinator stops accepting new HTTP requests and scheduler ticks, waits up to `shutdown` for active `spawn` and declarative `worker` tasks to finish, and executes `on stop` hooks in reverse declaration order (`LIFO`).

See `examples/app_service/` in the repository for a complete runnable project combining `config`, `router`, `schedule`, `app`, and `conduit test`.
