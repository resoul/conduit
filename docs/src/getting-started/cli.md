# Command Line Interface

The `conduit` CLI provides tools for checking, compiling, executing, disassembling, and benchmarking Conduit programs.

---

## Command Reference

| Command | Arguments | Description |
| :--- | :--- | :--- |
| `conduit build` | `<file.cdt> [-o <output>]` | Compiles and bundles the program (including relative imports) into a standalone executable. |
| `conduit run` | `<file.cdt>` | Type-checks, compiles to bytecode, and executes `main()` in the VM. |
| `conduit watch` | `<file.cdt> [args...]` | Watches `.cdt` files and automatically recompiles and restarts on changes. |
| `conduit lsp` | None | Starts the stdio Language Server (diagnostics, hover, definition, symbols, rename, formatting, completion) for IDEs. |
| `conduit fmt` | `<file_or_dir> [--check]` | Formats `.cdt` source files (or checks formatting with `--check`). |
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

If checks pass, Conduit prints a summary of compiled types and functions (`OK: N type(s), M function(s)`). If an error exists, Conduit displays a diagnostic highlighting the line, column, and rule violation:

```text
[14:10] Check error: ...
```

### Disassembling Bytecode

To inspect the register bytecode emitted for a program:

```bash
conduit disasm main.cdt
```

### Running Benchmarks

```bash
conduit bench --strict
```

Executes the 34 built-in compute, call, closure, collection, pipeline, and JSON workloads and verifies all target latency gates.

### Building a Standalone Executable

```bash
conduit build main.cdt -o my_app
./my_app
```

Resolves all relative module imports, performs tree-shaking and type-checking ahead of time, and emits a self-contained executable with embedded bytecode (`CDTB0001`). Runtime arguments passed to `./my_app` are forwarded directly to `env.args()` and `cdt.cli`.

### Watching & Auto-Reloading in Development

```bash
conduit watch main.cdt
```

Polls `.cdt` files in the project directory and automatically stops, recompiles, and restarts `conduit run` whenever any file changes.
