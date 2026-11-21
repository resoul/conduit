# Conduit Declarative App Lifecycle Demo (`cdt.app`)

Demonstrates a unified declarative service using `app` (`config` + `@validation.*` fail-fast startup, `http` router serving, `schedule` background ticks, `shutdown` task draining, and reverse-order `LIFO` `on start` / `on stop` hooks) alongside `app.run<T>()` and programmatic `app.start<T>()` / `.stop()` testing.

## Offline Checks & Tests

```bash
cd examples/app_service
conduit check main.cdt
conduit routes main.cdt
conduit env example main.cdt
conduit test .
```

## Live Run

```bash
cd examples/app_service
conduit run main.cdt
# Press Ctrl+C (SIGINT) or send SIGTERM to trigger graceful shutdown
```
