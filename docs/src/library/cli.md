# `cdt.cli`

The `cdt.cli` module provides typed command-line argument parsing, subcommand/group routing, and automatic `--help` generation.

It supports **two interchangeable syntaxes** — a concise declarative `cli` DSL and `@cli.*` struct annotations — both compiling to the exact same `CliPlan` and native Rust parser with zero runtime difference.

---

## Importing `cdt.cli`

```conduit
import cli from "cdt.cli"
import { CliError } from "cdt.cli"
// or named imports:
// import { parse, parse_args, help, CliError } from "cdt.cli"
```

---

## Syntax 1: Declarative `cli` DSL

Use `cli Name "description"? { ... }` to define your CLI schema directly. At parse time, it desugars into standard Conduit `struct`s (e.g., `App_db`, `App_db_migrate`, `App_run`) with `@cli.*` annotations.

```conduit
import cli from "cdt.cli"
import { CliError } from "cdt.cli"

cli App "Project management CLI" {
    flag verbose "v" "Enable verbose logging"
    opt port: Int "p" "Server port" = 8080

    group db "Database operations" {
        cmd migrate "Run pending migrations" {
            arg target: String "Target environment"
            flag dry_run "Preview SQL without applying"
        }
        cmd status "Show migration status"
    }

    cmd serve "Start HTTP server" {
        arg entry: String "Entry script"
        rest extra "Additional arguments passed to the script"
    }
}

func main() -> Void {
    switch cli.parse<App>() {
        case CliError(e):
            print(e.message)
        case App(app):
            switch app.db {
                case App_db(db):
                    switch db.migrate {
                        case App_db_migrate(m):
                            print("Migrating {m.target} (dry_run={m.dry_run})")
                    }
                    switch db.status {
                        case App_db_status(_):
                            print("Checking DB status")
                    }
            }
            switch app.serve {
                case App_serve(s):
                    print("Serving {s.entry} on port {app.port} (+{s.extra.len()} args)")
            }
    }
}
```

### DSL Statements

| Statement | Desugared Field Type | Description |
| :--- | :--- | :--- |
| `flag name "s"? "help"? (= bool)?` | `Bool` (default `false`) | Boolean flag (`--name`, `-s`). |
| `opt name: T "s"? "help"? (= val)?` | `T` (`String`, `Int`, `Float`, `Bool`, or `T?`) | Named option (`--name val`, `--name=val`, `-s val`). |
| `arg name: T "help"? (= val)?` | `T` (`String`, `Int`, `Float`, `Bool`, or `T?`) | Positional argument in declaration order. |
| `rest name (: [String])? "help"?` | `[String]` (default `[]`) | Remaining positional arguments (including after `--`). |
| `cmd name "help"? { ... } (=> handler)?` | `Parent_name?` | Subcommand; creates struct `Parent_name` and optionally binds a handler (`cli.dispatch<T>()`). |
| `group name "help"? { ... }` | `Parent_name?` | Command group containing nested `cmd` / `group` items. |

You can also define a standalone reusable group with `cli group Name "help"? { ... }`.

---

## Syntax 2: `@cli.*` Struct Annotations

If you prefer explicit structs (for example, to share types across modules or combine `@cli.*` with `@validation.*` or `@json(...)`), annotate fields directly on `struct` declarations:

```conduit
import cli from "cdt.cli"
import { CliError } from "cdt.cli"

struct ServeCmd {
    entry: String @cli.arg("Entry script")
    extra: [String] @cli.rest("Additional arguments") = []
}

struct DbMigrateCmd {
    target: String @cli.arg("Target environment")
    dry_run: Bool @cli.flag("Preview SQL without applying") = false
}

struct DbGroup {
    migrate: DbMigrateCmd? @cli.cmd("Run pending migrations")
}

struct App {
    verbose: Bool @cli.flag("v", "Enable verbose logging") = false
    port: Int @cli.opt("p", "Server port") = 8080
    db: DbGroup? @cli.group("Database operations")
    serve: ServeCmd? @cli.cmd("Start HTTP server")
}

func run_with(args: [String]) -> String {
    switch cli.parse_args<App>(args) {
        case CliError(e):
            return e.kind + ": " + e.message
        case App(app):
            switch app.db {
                case DbGroup(db):
                    switch db.migrate {
                        case DbMigrateCmd(m):
                            return "migrate:" + m.target
                    }
            }
            switch app.serve {
                case ServeCmd(s):
                    return "serve:" + s.entry + ":" + app.port.to_string()
            }
            return "root:" + app.port.to_string()
    }
    return ""
}
```

### Annotation Reference

- `@cli.flag` / `@cli.flag("help")` / `@cli.flag("s", "help")`: Boolean flag (`Bool` or `Bool?`).
- `@cli.opt` / `@cli.opt("help")` / `@cli.opt("s", "help")`: Named option (`String`, `Int`, `Float`, `Bool`, or optional).
- `@cli.arg` / `@cli.arg("help")`: Positional argument.
- `@cli.rest` / `@cli.rest("help")`: Trailing positional arguments (`[String]`).
- `@cli.cmd` / `@cli.cmd("help")` / `@cli.cmd("custom-name", "help")`: Nested subcommand (`SubStruct?`).
- `@cli.group` / `@cli.group("help")` / `@cli.group("custom-name", "help")`: Nested command group (`GroupStruct?`).

> **Note:** Struct field annotations always come **before** `= default` (e.g. `port: Int @cli.opt("p", "Port") = 8080`).

---

## API Reference

- `cli.parse<T>() -> T | CliError` — Parses process arguments (`std::env::args().skip(1)`).
- `cli.parse_args<T>(args: [String]) -> T | CliError` — Parses an explicit list of argument strings (useful for testing).
- `cli.dispatch<T>() -> Void | CliError | ...` — Parses process arguments and calls the `=> handler` bound to the matched command (supports `(T, Cmd) -> R` and `(Cmd) -> R`).
- `cli.dispatch_args<T>(args: [String]) -> Void | CliError | ...` — Parses an explicit `[String]` list and calls the matched command's `=> handler`.
- `cli.help<T>() -> String` — Returns the formatted `--help` text for `T`.
- `CliError { kind: String, message: String }` — Error or help signal returned by `parse` / `parse_args`.
  - `kind == "help"` when `-h` or `--help` is passed (`message` contains the formatted help text for the matched command or group).
  - `kind == "unknown"` for unrecognized flags or unexpected positional arguments.
  - `kind == "missing"` when a required argument or option value is omitted.
  - `kind == "invalid"` when an `Int`, `Float`, or `Bool` value fails to parse.

---

## Parsing Rules

- **Long flags & options:** `--verbose`, `--port 8080`, `--port=8080`. Underscores in field names map to hyphens on the CLI (e.g. `dry_run` matches `--dry-run`).
- **Short flags & options:** `-v`, `-p 8080`, `-p=8080`, and bundled short flags (`-vd`).
- **End of options:** `--` stops flag/option parsing so remaining tokens are treated as positional / `rest` arguments.
- **Help flag:** `-h` and `--help` are automatically handled at any level (root, group, or subcommand) and return `CliError` with `kind: "help"`.
