# Command Line Interface

The `conduit` CLI provides tools for checking, executing, debugging, and inspecting Conduit source files.

---

## Command Reference

| Command | Arguments | Description |
| :--- | :--- | :--- |
| `conduit run` | `<file.cdt>` | Type-checks all modules in the project graph and executes the program. |
| `conduit check` | `<file.cdt>` | Performs static semantic analysis and type checking without running code. |
| `conduit reachability` | `<file.cdt>` | Analyzes reachable call graphs, identifies dead code, and reports native dependencies. |
| `conduit doc` | `[module]` | Generates deterministic Markdown API documentation from the native registry. |
| `conduit lsp` | None | Starts the JSON-RPC Language Server Protocol (LSP) daemon over standard I/O. |
| `conduit ast` | `<file.cdt>` | Parses the source file and outputs the formatted Abstract Syntax Tree (AST). |
| `conduit tokens` | `<file.cdt>` | Tokenizes the source file and outputs the sequence of lexical tokens with line and column spans. |
| `conduit --version` | None | Prints the version string (e.g., `Conduit v0.1.4`). |
| `conduit --help` | None | Displays the usage instructions. |

---

## Examples of Usage

### Checking a Project

```bash
conduit check src/main.cdt
```

If an error exists, Conduit displays a diagnostic highlighting the line, column, and exact rule violation:

```text
[14:10] Type error: Expression result of type 'Int | NotFound' contains an error and must not be discarded
```

### Analyzing Reachability

The reachability analyzer traces all execution paths starting from `main()` across all imported modules and standard libraries:

```bash
conduit reachability src/main.cdt
```

Example report:

```text
=== Reachability Analysis Report ===
Root module: /workspace/src/main.cdt
Reachable functions: 4 / 6 total (2 unused)
Reachable native functions (2):
  - cdt.time.monotonic()
  - cdt.time.elapsed()
```

### Inspecting Abstract Syntax Tree (AST)

To understand how Conduit parses complex expressions or operator precedence:

```bash
conduit ast hello.cdt
```

Output:

```text
Program {
    declarations: [
        Function(
            FunctionDecl {
                is_exported: false,
                name: "main",
                params: [],
                return_type: None,
                body: Block { ... },
                span: Span { line: 1, col: 1 },
            },
        ),
    ],
}
```

### Inspecting Native Library Documentation

```bash
conduit doc cdt.time
```

Prints the complete Markdown documentation for `cdt.time` directly generated from the engine's internal `NativeRegistry`.

