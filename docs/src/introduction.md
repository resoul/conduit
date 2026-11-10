# Introduction

Welcome to the official documentation for **Conduit**, a statically typed, domain-specific programming language tailored for web services, real-time communication, and event-driven data streams. Conduit source files use the `.cdt` extension.

Conduit is designed from first principles with a strong focus on runtime reliability, compile-time safety, simplicity, and zero external dependency footprint.

---

## Core Guarantees & Philosophy

1. **Zero External Dependencies**
   The entire Conduit toolchain—including the lexer, parser, static type checker, interpreter, module loader, LSP server, reachability analyzer, and documentation generator—is implemented exclusively using the standard library (`std`). No third-party crates are required.

2. **Self-Contained & Lightweight**
   The Conduit executable is compiled as a single self-contained binary under 1 MB in size. It does not require a virtual machine (JVM, CLR), heavy runtime environments, or system interpreters to run.

3. **Explicit Modules & Privacy by Default**
   Every declaration (types, errors, functions) within a module is private by default. Items are only exposed to external consumers when explicitly tagged with the `export` keyword. Private types cannot accidentally leak through public API signatures.

4. **Typed Errors as First-Class Values**
   Conduit eliminates hidden exception handling and unchecked runtime failures. Functions declare error types directly in union signatures (`T | Error`). The compiler strictly prevents discarding error values, enforces exhaustive pattern matching with `switch`, and provides ergonomic propagation with the `?` operator.

5. **First-Class Standard Library (`cdt.*`)**
   Standard modules, such as `cdt.time`, are integrated through a unified native registry (`NativeRegistry`). They expose type-safe opaque abstractions (like `time.Instant`, `time.Timestamp`, and `time.Duration`) that prevent accidental manual tampering while providing deterministic time operations.

6. **Static Dead Code & Dependency Analysis**
   Conduit includes built-in reachability analysis (`conduit reachability`), allowing developers and build pipelines to inspect reachable call trees, detect unused functions across projects, and verify native standard library dependencies.

---

## How to Navigate This Book

- **[Getting Started](getting-started/installation.md)**: Walk through installing the binary, writing your first program, exploring the CLI commands, and setting up editor support.
- **[Language Reference](language/variables.md)**: Explore the grammar, type system, functions as first-class values, error handling rules, collection transformations, and module encapsulation.
- **[Standard Library](library/index.md)**: Detailed API references for standard library packages, starting with `cdt.time`.
- **[Practical Guides](guides/working-with-errors.md)**: Real-world patterns for resilient error pipelines, modular application layout, and performance analysis.
- **[Internals](internals/architecture.md)**: An overview of the compiler pipeline from source text to execution.

