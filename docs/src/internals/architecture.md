# Architecture & Pipeline

Conduit is implemented as a multi-stage compiler and register-based bytecode VM written in pure Rust (`std` only) with zero external dependencies.

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
  [ 4. Monomorphizer ]   ──> Generic & Tagged-Union Specialization (`frontend/monomorph*.rs`)
       │
       ▼
  [ 5. Type Checker ]    ──> Multi-Error Static Semantics & Exhaustiveness (`frontend/check*.rs`)
       │
       ▼
  [ 6. Reachability ]    ──> Dead-Code Tree-Shaking (`frontend/reachability.rs`)
       │
       ▼
  [ 7. IR Lowering ]     ──> Register Bytecode & LIFO `defer` Epilogues (`ir/lower*.rs`, `conduit disasm`)
       │
       ▼
  [ 8. Validation ]      ──> Bytecode Operand and Return Checks (`ir/mod.rs`)
       │
       ▼
  [ 9. Bytecode VM ]     ──> Split Scalar/Object Register Execution (`vm/exec*.rs`)
```

## Bytecode Validation

`Module::validate` checks register indices, constant and function references, and jump targets before the VM executes the bytecode. Every instruction that returns a value must agree with its function's declared scalar or object register bank. This includes early object returns from `?`. The same checks apply when loading the bytecode bundled into a standalone executable.

## Record Maps

The VM can build records directly for a single `map` stage that adds an integer constant to one field and copies two inline object fields. When the function immediately returns the constructed record, the fast path handles the checked addition and field copies together, avoiding per-record bytecode dispatch.

---

## Source Code Organization

The modules in `src/` correspond directly to the compiler, tooling, and VM stages:

- `src/frontend/`: Lexer (`lexer.rs`), recursive-descent parser (`parser.rs`, `parser_expr.rs`, `parser_stmt.rs`, `parser_cli.rs`), module linker (`linker.rs`), generic & data-enum monomorphization (`monomorph*.rs`), multi-error static type checker (`check*.rs`), and reachability tree-shaking (`reachability.rs`).
- `src/ir/`: IR `Module` validator/disassembler (`mod.rs`), binary bundle codec (`bundle.rs`), and register bytecode lowering (`lower*.rs`), including fused `RunPipeline` planning and zero-opcode `defer` epilogues.
- `src/vm/`: Compact 16-byte `Op` and `Obj` definitions (`opcode.rs`, `value.rs`), main VM dispatch loop (`exec.rs`, `exec_cold.rs`, `exec_union.rs`, `exec_map.rs`, `exec_app.rs`, `exec_http.rs`, `exec_db.rs`, `exec_redis.rs`, `exec_jwt.rs`, `exec_cache.rs`, `exec_lock.rs`, `exec_chan.rs`, `exec_cron.rs`, `exec_sys.rs`), and fused pipeline execution (`pipeline.rs`, `pipeline_leaf.rs`).
- `src/native/`: Zero-dependency host implementations for `cdt.json`, `cdt.time`, `cdt.validation`, `cdt.http`, `cdt.db` (PostgreSQL & MySQL wire drivers), `cdt.redis` (RESP2), `cdt.jwt` (HS256/HS512), `cdt.cache` (TTL/LRU/WAL/Snapshot), `cdt.lock`, `cdt.chan`, `cdt.cron`, `cdt.fs`, `cdt.crypto`, `cdt.url`, `cdt.csv`, `cdt.cli`, `cdt.env`, `cdt.path`, `cdt.process`, `cdt.math`, and `cdt.log`.
- `src/diagnostics.rs`, `src/openapi.rs`, `src/test_runner.rs`, `src/fmt.rs`, `src/lsp.rs`, `src/watch.rs`: Snippet error diagnostics, `conduit routes` & OpenAPI 3.0.3 generator (`conduit openapi`), built-in `.cdt` test runner (`conduit test`), formatter (`conduit fmt`), Language Server (`conduit lsp`), and file watcher (`conduit watch`).
- `src/benchmarks/` & `scripts/bench/`: Built-in `conduit bench` suite and cross-language benchmark suite (`run_all.py`, `bench.{cdt,go,swift,kt,mjs,php,py}` covering 25 operations).
