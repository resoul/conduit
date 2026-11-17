# Conduit Starter Example

A minimal multi-file Conduit project combining `const`, `enum`, `cdt.cli`, `cdt.env`, `cdt.fs`, `cdt.log`, `cdt.url`, `cdt.validation`, `cdt.json`, `cdt.time`, list aggregates (`.sum()`, `.max()`), and `|>` pipelines, packaged for both `conduit watch` in development and `FROM scratch` (~2–3 MB) in production.

## Local Usage

```bash
# Format & type-check
conduit fmt .
conduit check main.cdt

# Run CLI commands
conduit run main.cdt -- check
conduit run main.cdt -- --verbose run
conduit run main.cdt -- --verbose stream
conduit run main.cdt -- inspect "https://example.com:8443/v1/items?tag=prod&limit=10"

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
