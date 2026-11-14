# Architecture & Pipeline

Conduit 0.2.0 is implemented as a multi-stage compiler and register-based bytecode VM written in pure Rust (`std` only) with zero external dependencies.

---

## Compiler & Runtime Pipeline

When you execute `conduit run main.cdt`, source files pass through the following pipeline:

```text
Source Code (.cdt)
       │
       ▼
  [ 1. Lexer ]           ──> Token Stream (`frontend/lexer.rs`)
       │
       ▼
  [ 2. Parser ]          ──> AST (`frontend/parser*.rs`)
       │
       ▼
  [ 3. Linker ]          ──> TS-style Module Graph (`frontend/linker.rs`)
       │
       ▼
  [ 4. Monomorphizer ]   ──> Generic Specialization (`frontend/monomorph*.rs`)
       │
       ▼
  [ 5. Type Checker ]    ──> Static Semantics & Exhaustiveness (`frontend/check*.rs`)
       │
       ▼
  [ 6. Reachability ]    ──> Dead-Code Tree-Shaking (`frontend/reachability.rs`)
       │
       ▼
  [ 7. IR Lowering ]     ──> Register Bytecode (`ir/lower*.rs`, `conduit disasm`)
       │
       ▼
  [ 8. Bytecode VM ]     ──> Split Scalar/Object Register Execution (`vm/exec*.rs`)
```

---

## Source Code Organization

The modules in `src/` correspond directly to the compiler and VM stages:

- `src/frontend/`: Lexer (`lexer.rs`), recursive-descent parser (`parser.rs`, `parser_expr.rs`, `parser_stmt.rs`), module linker (`linker.rs`), generic monomorphization (`monomorph*.rs`), static type checker (`check*.rs`), and reachability tree-shaking (`reachability.rs`).
- `src/ir/`: IR `Module` validator/disassembler (`mod.rs`) and register bytecode lowering (`lower*.rs`), including fused `RunPipeline` planning.
- `src/vm/`: Compact 16-byte `Op` and `Obj` definitions (`opcode.rs`, `value.rs`), main VM dispatch loop (`exec.rs`, `exec_cold.rs`, `exec_union.rs`, `exec_map.rs`), and fused pipeline execution (`pipeline.rs`, `pipeline_leaf.rs`).
- `src/native/`: Zero-dependency host implementations for `cdt.json`, `cdt.time`, and `cdt.validation`.
- `src/benchmarks/`: Built-in `conduit bench` suite covering 34 VM workloads.
