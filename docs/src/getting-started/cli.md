# Command Line Interface

The `conduit` CLI provides tools for checking, compiling, executing, disassembling, and benchmarking Conduit programs.

---

## Command Reference

| Command | Arguments | Description |
| :--- | :--- | :--- |
| `conduit build` | `<file.cdt> [-o <output>]` | Compiles and bundles the program (including relative imports) into a standalone executable. |
| `conduit run` | `<file.cdt>` | Type-checks, compiles to bytecode, and executes `main()` in the VM. |
| `conduit test` | `[file_or_dir] [--filter <pat>] [--fail-fast]` | Discovers and runs `test "<name>" { ... }` blocks in `.cdt` files. |
| `conduit watch` | `<file.cdt> [args...]` | Watches `.cdt` files and automatically recompiles and restarts on changes. |
| `conduit lsp` | None | Starts the stdio Language Server (diagnostics, hover, definition, symbols, rename, formatting, completion) for IDEs. |
| `conduit fmt` | `<file_or_dir> [--check]` | Formats `.cdt` source files (or checks formatting with `--check`). |
| `conduit routes` | `<file.cdt> [--router <name>]` | Prints the declarative HTTP router route table (`METHOD`, `PATH`, `HANDLER`, `BODY`, middleware). |
| `conduit openapi` | `<file.cdt> [--router <name>] [--title <str>] [--version <str>] [--server <url>] [-o <file>]` | Generates an OpenAPI 3.0.3 JSON specification from declarative `router` and `@validation.*` schemas. |
| `conduit env example` | `<file.cdt> [-o <file>]` | Generates a `.env.example` template from `config` and `@env` fields. |
| `conduit check` | `<file.cdt>` | Performs static semantic analysis and type checking without running code. |
| `conduit disasm` | `<file.cdt>` | Compiles the program and prints the disassembled register bytecode. |
| `conduit bench` | `[--filter <pat>] [--strict]` | Runs the built-in VM benchmark suite and checks target performance gates. |
| `conduit --version` (`-V`) | None | Prints the installed Conduit version. |
| `conduit --help` (`-h`) | None | Displays the usage instructions. |

---

## Examples of Usage

### Checking a Project

```bash
conduit check main.cdt
```

If checks pass, Conduit prints a summary of compiled types and functions (`OK: N type(s), M function(s)`). If errors exist across functions or imported files, Conduit displays snippet-based caret diagnostics pointing to each file, line, and column:

```text
error[2:10]: Operator '+' cannot be applied to String and Int
 --> main.cdt:2:10
   |
 2 |     x := "hello" + 1
   |          ^
```

### Disassembling Bytecode

To inspect the register bytecode emitted for a program:

```bash
conduit disasm main.cdt
```

### Running Benchmarks

```bash
conduit bench --strict
conduit bench --strict --filter collections.map_records
```

Use a release build for benchmark timings. The suite reports the median and median absolute deviation in microseconds per operation. `--filter` selects workloads by name substring, and `--strict` exits with status 1 if a selected workload exceeds its target. For `collections.map_records`, one operation maps 4,096 records and the target is 140 µs.

### Building a Standalone Executable

```bash
conduit build main.cdt -o my_app
./my_app
```

Resolves all relative module imports, performs tree-shaking and type-checking ahead of time, and emits a self-contained executable with SHA-256-verified embedded bytecode (`CDTB0002`). Runtime arguments passed to `./my_app` are forwarded directly to `env.args()` and `cdt.cli`.

### Watching & Auto-Reloading in Development

```bash
conduit watch main.cdt
```

Polls `.cdt` files in the project directory and automatically stops, recompiles, and restarts `conduit run` whenever any file changes.

### Managing Database Migrations (`conduit db`)

```bash
conduit db status main.cdt --url postgres://conduit:secret@127.0.0.1:5432/app
conduit db migrate main.cdt --url postgres://conduit:secret@127.0.0.1:5432/app [--dry-run]
conduit db rollback main.cdt --url postgres://conduit:secret@127.0.0.1:5432/app [--steps 1] [--dry-run]
```

Inspects, applies, or rolls back declarative `migration <version> "<name>"` blocks against PostgreSQL or MySQL, tracking applied versions in `schema_migrations`.

### Inspecting & Running Cron Jobs (`conduit cron`)

```bash
conduit cron list main.cdt
conduit cron run main.cdt cleanup_sessions
conduit cron print main.cdt
```

Lists declared `schedule` jobs, runs a single job immediately, or prints ready-to-paste `crontab` lines to `stdout` without modifying the host system.

### Running Tests (`conduit test`)

```bash
conduit test
conduit test math_test.cdt
conduit test tests/ --filter auth --fail-fast
```

Executes top-level `test "<name>" { ... }` blocks in isolated VM instances using built-in `assert`, `assert_eq`, `assert_ne`, and `fail` assertions, and returns a non-zero exit code if any test fails or fails to compile.

### Inspecting Routes & Generating OpenAPI (`conduit routes`, `conduit openapi`)

```bash
conduit routes main.cdt
conduit openapi main.cdt --title "My Service" --server http://127.0.0.1:8080 -o openapi.json
```

Inspects declarative `router` schemas, group/route middleware, CORS/static directives, typed JSON request contracts (`@validation.*`), and inferred handler JSON/text responses to display a formatted route table or emit an OpenAPI 3.0.3 specification.
