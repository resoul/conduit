# Conduit Starter Example

A minimal multi-file Conduit project combining `cdt.cli`, `cdt.env`, `cdt.fs`, `cdt.validation`, `cdt.json`, `cdt.time`, and `|>` pipelines, packaged for both `conduit watch` in development and `FROM scratch` (~2–3 MB) in production.

## Local Usage

```bash
# Type-check
conduit check main.cdt

# Run CLI commands
conduit run main.cdt -- check
conduit run main.cdt -- --verbose run
conduit run main.cdt -- --verbose stream

# Auto-reload on file changes
conduit watch main.cdt -- --verbose run

# Build standalone native binary
conduit build main.cdt -o worker
./worker --verbose run
```

## Docker Compose

```bash
# Development with auto-reload (conduit watch)
docker compose up dev

# Production FROM scratch container (~2-3 MB)
docker compose up --build prod
```
