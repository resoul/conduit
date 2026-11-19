# Conduit Database & Migrations Demo (`cdt.db` + `cdt.validation` + `cdt.crypto`)

Demonstrates zero-dependency PostgreSQL and MySQL connectivity (`cdt.db`), `@db.*` + `@validation.*` schema models (`create_table`, `insert`, `update`, `count`, `find_by`, `query_one`, `all`), dialect-aware declarative `migration` blocks, transactions, PBKDF2-SHA256 password hashing (`cdt.crypto`), UTC ISO timestamps (`cdt.time`), `conduit test`, and the `conduit db` CLI.

## Offline Checks & Tests

```bash
cd examples/db_service
conduit check main.cdt
conduit test .
```

## PostgreSQL & MariaDB / MySQL via Docker

```bash
cd examples/db_service
docker compose up -d --wait

# PostgreSQL 16
DATABASE_URL="postgres://conduit:secret@127.0.0.1:15432/conduit_demo" conduit run main.cdt
conduit db status main.cdt --url "postgres://conduit:secret@127.0.0.1:15432/conduit_demo"

# MariaDB 11.8 (MySQL wire protocol)
DATABASE_URL="mysql://conduit:secret@127.0.0.1:13306/conduit_demo" conduit run main.cdt
conduit db status main.cdt --url "mysql://conduit:secret@127.0.0.1:13306/conduit_demo"
conduit db rollback main.cdt --url "mysql://conduit:secret@127.0.0.1:13306/conduit_demo" --steps 1

docker compose down
```
