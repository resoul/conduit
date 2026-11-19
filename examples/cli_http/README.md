# Conduit Unified HTTP Client & Server Demo (`cdt-curl`)

A full-featured, zero-dependency HTTP/1.1 CLI client, router server, and Conduit stdlib showcase (`cdt.http`, `cdt.cli`, `cdt.env`, `cdt.fs`, `cdt.json`, `cdt.url`, `cdt.validation` extended rules, `cdt.time`, `cdt.log`, `|>` pipelines, `cors`, `group`, `conduit test`, and Docker `FROM scratch`).

## Local Usage

```bash
cd examples/cli_http

# Format, type-check & run built-in tests
conduit fmt .
conduit check main.cdt
conduit test .

# Inspect the HTTP router table and generate OpenAPI 3.0.3 JSON spec
conduit routes server.cdt
conduit openapi server.cdt --title "CdtCurl Demo API" --version "1.0.0"

# HTTP router & middleware self-test
conduit run main.cdt -- self-test

# Config, JSON pipeline & file streaming
conduit run main.cdt -- check
conduit run main.cdt -- events
conduit run main.cdt -- stream

# Start local demo server (in one terminal)
conduit run main.cdt -- serve --addr 127.0.0.1:8080

# Query it with the HTTP client (in another terminal)
conduit run main.cdt -- -i get http://127.0.0.1:8080/users/42
conduit run main.cdt -- -j post http://127.0.0.1:8080/echo -d '{"ok":true}'
conduit run main.cdt -- req PATCH http://127.0.0.1:8080/items/9
conduit run main.cdt -- bytes http://127.0.0.1:8080/
conduit run main.cdt -- post http://127.0.0.1:8080/shutdown
```

## Docker Compose

```bash
cd examples/cli_http

# Development with auto-reload (conduit watch)
docker compose up dev

# Production FROM scratch container (~2-3 MB)
docker compose up --build prod
```
