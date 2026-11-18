# `cdt.cron`

Zero-dependency declarative scheduler and 5-field UTC cron engine (`schedule`, `cron.run<T>`, `cron.start<T>`).

```conduit
import * as cron from "cdt.cron"
import * as log from "cdt.log"

schedule Jobs {
    every "*/5 * * * *" => cleanup_sessions
    interval 10s        => heartbeat
}

func cleanup_sessions() -> Void {
    log.info("Cleaning up expired sessions")
}

func heartbeat() -> Void {
    log.debug("tick")
}

export func main() -> Void | cron.CronError {
    cron.run<Jobs>()?
}
```

## Declarative `schedule` Block

* `every "<5-field cron>" => handler` — Standard 5-field UTC cron expression (`min hour dom mon dow`) or macro (`@hourly`, `@daily`, `@weekly`, `@monthly`, `@yearly`). Validated at compile time.
* `interval <N><ms|s|m|h> => handler` — Fixed interval (e.g. `500ms`, `10s`, `5m`, `1h`).

Handlers must have signature `() -> Void` or `() -> Void | Error`. Each execution runs in an isolated VM instance with overlap protection (if a previous run is still active, the next tick is skipped).

## Types

* `Scheduler` — Background scheduler handle returned by `cron.start<T>()`
* `CronError { kind: String, message: String }`

## Functions & Methods

| Symbol | Signature | Description |
| :--- | :--- | :--- |
| `cron.run<T>` | `() -> Void \| CronError` | Runs schedule `T` in the current thread until stopped |
| `cron.start<T>` | `() -> Scheduler \| CronError` | Starts schedule `T` in a background thread |
| `cron.run_once<T>` | `(job: String) -> Void \| CronError` | Executes a single named job immediately |
| `cron.validate` | `(expr: String) -> Bool` | Validates a 5-field cron expression |
| `Scheduler.stop` | `() -> Void` | Signals graceful shutdown and joins the scheduler thread |
| `Scheduler.is_running` | `() -> Bool` | Returns `true` while the scheduler loop is active |

## CLI

```bash
conduit cron list app.cdt
conduit cron run app.cdt cleanup_sessions
conduit cron print app.cdt   # prints crontab lines to stdout
```
