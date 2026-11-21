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
| PostgreSQL | `postgres://user:pass@host:5432/dbname` / `postgresql://...` | Native v3.0 wire protocol (SCRAM-SHA-256, LRU prepared statement cache with `?prepare=off` for PgBouncer); translates `?` to `$1, $2, ...` |
| MySQL | `mysql://user:pass@host:3306/dbname` | Native Protocol v10 (`mysql_native_password`, `caching_sha2_password` fast auth); LRU prepared statement cache (`?prepare=off` to disable) |

## Types

* `Connection` — Active database connection handle
* `Pool` — Thread-safe database connection pool (`acquire`, `with`, `stats`, `close`)
* `Param` — Typed SQL bind parameter (`db.null()`, `db.int(v)`, `db.float(v)`, `db.bool(v)`, `db.text(v)`)
* `DbError { kind: String, message: String, code: String }` (`"connect_error"`, `"invalid_url"`, `"query_error"`, `"protocol"`, `"decode"`, `"tx"`, `"migration"`, `"closed"`)

## Functions

| Function | Signature | Description |
| :--- | :--- | :--- |
| `connect` | `(url: String) -> Connection \| DbError` | Opens a PostgreSQL or MySQL connection |
| `pool` | `(url: String, max_conns?: Int, idle_timeout_ms?: Int) -> Pool \| DbError` | Opens a thread-safe connection pool (defaults: `max_conns = 16`, `idle_timeout_ms = 300000`) |
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
| `insert_all<T>` | `(rows: [T]) -> Int \| DbError` | Batch-inserts multiple structs `T` in chunked multi-row `INSERT` statements |
| `update<T>` | `(row: T) -> Int \| DbError` | Updates non-PK columns of `T` matching `@db.pk` (or `id`), returns affected count |
| `count<T>` | `() -> Int \| DbError` | Returns total row count (`SELECT COUNT(*)`) for model `T`'s table |
| `all<T>` | `() -> [T] \| DbError` | Selects all rows from model `T`'s table |
| `find<T>` | `(id: Any) -> T? \| DbError` | Finds a row by `@db.pk` (or `id`) |
| `find_by<T>` | `(column: String, value: Any) -> [T] \| DbError` | Queries rows where validated model column matches `value` |
| `delete<T>` | `(id: Any) -> Int \| DbError` | Deletes a row by `@db.pk` (or `id`) |
| `from<T>` | `() -> Query<T>` | Starts a fluent typed query builder for `@db.table` model `T` |

## Connection Pool (`db.Pool`)

For concurrent HTTP services and workers, use `db.pool()` to reuse persistent PostgreSQL or MySQL connections safely across threads:

```conduit
pool := db.pool("postgres://conduit:secret@127.0.0.1:5432/app", 16, 300000)?
defer pool.close()

// Automatic acquire + release via callback
total := pool.with(conn => conn.count<User>())?

// Or manual acquire + defer close (returns connection to pool)
conn := pool.acquire()?
defer conn.close()
```

| Method | Signature | Description |
| :--- | :--- | :--- |
| `acquire` | `() -> Connection \| DbError` | Acquires a connection (calling `conn.close()` returns it to the pool) |
| `with` | `(cb: (Connection) -> R) -> R \| DbError` | Runs `cb(conn)` and always returns `conn` to the pool |
| `stats` | `() -> Map<String, Int>` | Returns `{"idle": ..., "in_use": ..., "max": ...}` |
| `close` | `() -> Void` | Closes all idle connections and marks the pool closed |

## Fluent Query Builder (`conn.from<T>()`)

For dynamic queries (e.g. conditional HTTP search filters), `conn.from<T>()` returns a chainable `Query<T>` that validates all column names against model `T` and binds all values as safe SQL parameters:

```conduit
func search_users(conn: db.Connection, role: String?, page: Int) -> [User] | db.DbError {
    q := conn.from<User>()
        .where_eq("active", true)
        .where("age", ">=", 18)
        .order_by("id", "desc")
        .paginate(page, 20)

    if role != nil {
        q = q.where_eq("role", role)
    }
    return q.all()
}
```

Available `Query<T>` methods:
* **Projection:** `.select([cols])`
* **Filters:** `.where_eq(col, val)`, `.or_where_eq(col, val)`, `.where(col, op, val)` (`=`, `!=`, `>`, `>=`, `<`, `<=`, `LIKE`, `ILIKE`), `.or_where(col, op, val)`, `.where_in(col, [vals])`, `.or_where_in(col, [vals])`, `.where_null(col)`, `.where_not_null(col)`, `.or_where_null(col)`, `.or_where_not_null(col)`, `.and_query(sub)`, `.or_query(sub)`
* **Ordering & Pagination:** `.order_by(col, dir?)` (`"asc"` | `"desc"`), `.limit(n)`, `.offset(n)`, `.paginate(page, per_page)`
* **Execution:** `.all() -> [T] | DbError`, `.first() -> T? | DbError`, `.count() -> Int | DbError`, `.exists() -> Bool | DbError`, `.delete() -> Int | DbError`, `.rows() -> [Map<String, Any>] | DbError`, `.to_sql(mode?) -> String`

## Declarative `query` Blocks

For static queries with full **compile-time** field and type verification, define top-level `query` blocks:

```conduit
query FindAdultUsers(conn: db.Connection, min_age: Int, max_rows: Int = 20) -> [User] {
    from User
    where active == true && (age >= min_age || role in ["admin", "owner"]) && bio != nil
    order_by age desc, id asc
    limit max_rows
    offset 0
}

query CountActiveUsers(conn: db.Connection) -> Int {
    from User
    where active == true
    count
}
```

Supported clauses: `from <Model>`, `select <fields...>`, `where <pred>` (`==`, `!=`, `>`, `>=`, `<`, `<=`, `in`, `like`, `ilike`, `&&`, `||`, parentheses), `order_by <field> [asc|desc]`, `limit <expr>`, `offset <expr>`, and optional mode `all` (default), `first` (`T?`), `count` (`Int`), `exists` (`Bool`), or `delete` (`Int`). Call them like normal functions: `users := FindAdultUsers(conn, 18)?`.

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

## TLS & Security

Conduit includes a built-in zero-dependency TLS 1.2 / 1.3 client used by PostgreSQL, MySQL, Redis (`rediss://`), and HTTPS (`http.get` / `http.request`).

New TLS 1.3 connections can resume a previous session using a server-issued ticket cached in memory for the same host, port, ALPN settings, and trust configuration. Resumed sessions use a fresh ephemeral key exchange. The client sends database credentials and commands after the handshake completes.

- **PostgreSQL URL parameters:** `sslmode=disable|allow|prefer|require|verify-ca|verify-full` (current default: `disable`), `sslrootcert=/path/to/ca.pem`. Note: `allow` is simplified to try TLS first (like `prefer`) and fall back to plaintext if the server refuses SSL (`'N'`). When TLS is active and the server advertises `SCRAM-SHA-256-PLUS`, `tls-server-end-point` channel binding is automatically negotiated.
  With `sslmode=disable`, `allow`, `prefer`, or `require` without `sslrootcert`, Conduit emits a one-time warning per host on non-loopback addresses (including RFC 1918 private networks and Docker hostnames such as `db`); **Conduit 0.4.0 will default non-loopback connections to `verify-full`**. Use `sslmode=verify-full` for verified TLS with SAN checks, `sslmode=verify-ca&sslrootcert=/path/to/ca.pem` for internal certificates without a hostname SAN, or explicit `sslmode=disable` only on a trusted network.
- Prepared PostgreSQL queries request binary results for integers, booleans, double-precision floats, and UUIDs
  once the statement cache has the result column types. The connection preserves the usual Conduit result types.
- **MySQL URL parameters:** `ssl-mode=disabled|preferred|required|verify_ca|verify_identity` (current default: `disabled`), `sslrootcert=/path/to/ca.pem` (or `ssl-ca`). Enables `caching_sha2_password` full authentication over TLS. Non-loopback connections with `disabled`, `preferred`, or `required` emit a migration warning toward `verify_identity` / `verify_ca` + `sslrootcert`.
- **Redis URL parameters:** Use `rediss://...` with optional `tls_verify=full|ca|none` (default: `full`) and `ca_file=/path/to/ca.pem`. Non-loopback `redis://` plaintext or `tls_verify=none` connections emit a one-time warning per host.
- **Supported algorithms & limits:** TLS 1.3 (`AES-128/256-GCM`, `ChaCha20-Poly1305`, `X25519`, `P-256`) and TLS 1.2 (`ECDHE` + `AEAD` with mandatory `extended_master_secret`). Legacy SSLv3 / TLS 1.0 / TLS 1.1, CBC, RC4, 3DES, static RSA/DHE, renegotiation, compression, and CRL/OCSP revocation checks are not supported.
