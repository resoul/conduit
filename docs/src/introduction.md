# Introduction

Welcome to the official documentation for **Conduit**, a statically typed, domain-specific programming language tailored for web services, data processing pipelines, and modular applications. Conduit source files use the `.cdt` extension.

Conduit is designed from first principles with a strong focus on runtime performance, compile-time safety, simplicity, and zero external dependency footprint.

---

## Core Guarantees & Philosophy

1. **Zero External Dependencies**
   The entire Conduit toolchain—including the lexer, parser, static type checker, monomorphizer, IR compiler, register bytecode VM, and standard libraries—is implemented exclusively using the Rust standard library (`std`). No third-party crates are required.

2. **Fast Register Bytecode VM**
   Conduit compiles `.cdt` programs to compact register-based bytecode with split scalar and object register banks, copy-on-write (`Arc`) collections, and fused `RunPipeline` execution for `|>` / `map` / `filter` chains.

3. **Explicit Modules & Privacy by Default**
   Every declaration (types, errors, functions) within a module is private by default. Items are only exposed to external consumers when explicitly tagged with the `export` keyword. Private types cannot accidentally leak through public API signatures, and unreachable functions are eliminated via reachability tree-shaking.

4. **Typed Errors as First-Class Values**
   Conduit eliminates hidden exception handling and unchecked runtime failures. Functions declare error types directly in union signatures (`T | Error`). The compiler strictly prevents discarding error values, enforces exhaustive pattern matching with `switch`, and provides ergonomic propagation with the `?` operator.

5. **First-Class Standard Library (`cdt.*`) & Built-In Concurrency**
   Standard modules—`cdt.http`, `cdt.db`, `cdt.redis`, `cdt.jwt`, `cdt.cache`, `cdt.lock`, `cdt.chan`, `cdt.cron`, `cdt.process`, `cdt.fs`, `cdt.crypto`, `cdt.json`, `cdt.time`, `cdt.validation`, `cdt.env`, `cdt.cli`, `cdt.path`, `cdt.url`, `cdt.csv`, `cdt.math`, and `cdt.log`—are implemented natively in pure Rust alongside built-in `spawn` background tasks, typed `cdt.chan` (`Chan<T>`) channels, and server lifecycle hooks (`on start` / `on stop`), integrated directly with the type checker and VM.

---

## How to Navigate This Book

- **[Getting Started](getting-started/installation.md)**: Walk through installing the binary, writing your first program, and exploring the CLI commands.
- **[Language Reference](language/variables.md)**: Explore the grammar, type system, functions as first-class values, error handling rules, collection transformations, and module encapsulation.
- **[Standard Library](library/index.md)**: Detailed API references for `cdt.json`, `cdt.time`, `cdt.validation`, `cdt.env`, and `cdt.cli`.
- **[Practical Guides](guides/working-with-errors.md)**: Real-world patterns for resilient error pipelines and modular application layout.
- **[Internals](internals/architecture.md)**: An overview of the compiler pipeline from source text to register bytecode execution.
