# Conduit Database & Migrations Demo (`cdt.db` + `cdt.crypto`)

Demonstrates zero-dependency PostgreSQL and MySQL connectivity (`cdt.db`), dialect-aware declarative `migration` blocks, transactions, typed `query<User>`, PBKDF2-SHA256 password hashing (`cdt.crypto`), and the `conduit db` CLI.

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
