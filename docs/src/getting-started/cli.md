# Command Line Interface

The `conduit` CLI provides tools for checking, compiling, executing, disassembling, and benchmarking Conduit programs.

---

## Command Reference

| Command | Arguments | Description |
| :--- | :--- | :--- |
| `conduit run` | `<file.cdt>` | Type-checks, compiles to bytecode, and executes `main()` in the VM. |
| `conduit check` | `<file.cdt>` | Performs static semantic analysis and type checking without running code. |
| `conduit disasm` | `<file.cdt>` | Compiles the program and prints the disassembled register bytecode. |
| `conduit bench` | `[--filter <pat>] [--strict]` | Runs the built-in VM benchmark suite and checks target performance gates. |
| `conduit --help` | None | Displays the usage instructions. |

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
