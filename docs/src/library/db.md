# `cdt.db`

Zero-dependency SQL database driver supporting PostgreSQL and MySQL, plus declarative schema migrations (`migration`) and CLI commands (`conduit db`).

```conduit
import * as db from "cdt.db"

struct User {
    id: Int
    name: String
    active: Bool
    bio: String?
}

migration 1 "create_users" {
    up {
        db.exec("CREATE TABLE IF NOT EXISTS users (id SERIAL PRIMARY KEY, name TEXT NOT NULL, active BOOLEAN NOT NULL, bio TEXT)")?
    }
    down {
        db.exec("DROP TABLE IF EXISTS users")?
    }
}

func main() -> Void | db.DbError {
    conn := db.connect("postgres://conduit:secret@127.0.0.1:5432/app")?
    conn.migrate()?
    conn.exec("INSERT INTO users (name, active, bio) VALUES (?, ?, ?)", [db.text("Alice"), db.bool(true), db.null()])?
    users := conn.query<User>("SELECT id, name, active, bio FROM users")?
    conn.close()
}
```

## Connection URLs

| Backend | URL Format | Notes |
| :--- | :--- | :--- |
| PostgreSQL | `postgres://user:pass@host:5432/dbname` / `postgresql://...` | Native v3.0 wire protocol (SCRAM-SHA-256, MD5, cleartext); translates `?` to `$1, $2, ...` |
| MySQL | `mysql://user:pass@host:3306/dbname` | Native Protocol v10 (`mysql_native_password`, `caching_sha2_password` fast auth); prepared statements |

## Types

* `Connection` — Active database connection handle
* `Param` — Typed SQL bind parameter (`db.null()`, `db.int(v)`, `db.float(v)`, `db.bool(v)`, `db.text(v)`)
* `DbError { kind: String, message: String, code: String }` (`"connect_error"`, `"invalid_url"`, `"query_error"`, `"protocol"`, `"decode"`, `"tx"`, `"migration"`, `"closed"`)

## Functions

| Function | Signature | Description |
| :--- | :--- | :--- |
| `connect` | `(url: String) -> Connection \| DbError` | Opens a PostgreSQL or MySQL connection |
| `null` | `() -> Param` | SQL `NULL` parameter |
| `int` | `(v: Int) -> Param` | 64-bit integer parameter |
| `float` | `(v: Float) -> Param` | 64-bit float parameter |
| `bool` | `(v: Bool) -> Param` | Boolean parameter |
| `text` | `(v: String) -> Param` | UTF-8 text parameter |

## `Connection` Methods

| Method | Signature | Description |
| :--- | :--- | :--- |
| `exec` | `(sql: String, params?: [Param]) -> Int \| DbError` | Executes a statement and returns affected row count |
| `query<T>` | `(sql: String, params?: [Param]) -> [T] \| DbError` | Executes a query and decodes rows into structs `T` |
| `query_one<T>` | `(sql: String, params?: [Param]) -> T? \| DbError` | Executes a query and decodes the first row as `T?` (`nil` if empty) |
| `query_rows` | `(sql: String, params?: [Param]) -> [Map<String, Any>] \| DbError` | Executes a query and returns dynamic row maps |
| `begin` | `() -> Void \| DbError` | Begins a transaction |
| `commit` | `() -> Void \| DbError` | Commits the active transaction |
| `rollback` | `() -> Void \| DbError` | Rolls back the active transaction |
| `migrate` | `() -> Int \| DbError` | Applies pending declarative `migration` blocks (returns count applied) |
| `dialect` | `() -> String` | Returns `"postgres"` or `"mysql"` |
| `last_insert_id` | `() -> Int` | Returns the last inserted row ID (MySQL) |
| `close` | `() -> Void` | Closes the connection |
| `create_table<T>` | `() -> Void \| DbError` | Creates table & indexes from `@db.*` struct model `T` |
| `drop_table<T>` | `() -> Void \| DbError` | Executes `DROP TABLE IF EXISTS` for model `T` |
| `insert<T>` | `(row: T) -> Int \| DbError` | Inserts struct `T` (omits `@db.auto` field if `0`) |
| `update<T>` | `(row: T) -> Int \| DbError` | Updates non-PK columns of `T` matching `@db.pk` (or `id`), returns affected count |
| `count<T>` | `() -> Int \| DbError` | Returns total row count (`SELECT COUNT(*)`) for model `T`'s table |
| `all<T>` | `() -> [T] \| DbError` | Selects all rows from model `T`'s table |
| `find<T>` | `(id: Any) -> T? \| DbError` | Finds a row by `@db.pk` (or `id`) |
| `find_by<T>` | `(column: String, value: Any) -> [T] \| DbError` | Queries rows where validated model column matches `value` |
| `delete<T>` | `(id: Any) -> Int \| DbError` | Deletes a row by `@db.pk` (or `id`) |

## Table & Model Schemas (`@db.*`)

Annotate a `struct` with `@db.table("name")` and fields with `@db.pk`, `@db.auto`, `@db.unique`, `@db.index`, `@db.col("col")`, `@db.default("expr")`, or `@db.fk("table.col")`:

```conduit
@db.table("users")
struct User {
    id: Int @db.pk @db.auto
    email: String @db.unique @db.index
    active: Bool = true
    bio: String?
}

migration 1 "create_users" {
    up { db.create_table<User>()? }
    down { db.drop_table<User>()? }
}
```

Use `db.ddl<User>("postgres")` or `db.ddl<User>("mysql")` to inspect the generated `CREATE TABLE` / `CREATE INDEX` SQL.

## Declarative Migrations & CLI

Top-level `migration <version> "<name>"` declarations support either `up { ... }` / `down { ... }` blocks (with `db: db.Connection` in scope) or shorthand SQL strings:

```conduit
migration 2 "seed_admin" {
    up "INSERT INTO users (name, active) VALUES ('Admin', true)"
    down "DELETE FROM users WHERE name = 'Admin'"
}
```

Applied migrations are tracked in the `schema_migrations` table. You can manage migrations from the CLI as well as via `conn.migrate()`:

```sh
conduit db status app.cdt --url postgres://conduit:secret@127.0.0.1:5432/app
conduit db migrate app.cdt --url postgres://conduit:secret@127.0.0.1:5432/app [--dry-run]
conduit db rollback app.cdt --url postgres://conduit:secret@127.0.0.1:5432/app [--steps 1] [--dry-run]
```
