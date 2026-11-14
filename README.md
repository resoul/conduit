# Conduit

A statically typed domain-specific programming language for data processing, validation pipelines, and modular services, powered by a register-based bytecode VM. Source files use the `.cdt` extension.

## Overview

Conduit is designed with explicit control flow, strict types, and composable standard libraries—without hidden runtime magic or external dependencies.

- **Zero External Dependencies**: Implemented purely using the Rust standard library (`std`) with no external crates.
- **Register Bytecode VM**: Compiles `.cdt` programs to compact register bytecode (`Op`) with split scalar/object registers, copy-on-write (`Arc`) collections, and fused `|>` pipelines.
- **Static Type System & Canonical Identity**: Catches type mismatches, unhandled error unions, and non-exhaustive `switch` cases at compile time, preserving distinct module-scoped type identities across imports.
- **Generics, Collections & Closures**: Supports user-defined generic structs and functions (`struct Box<T>`, `func find<T>`), safe indexing on `[T]` (`IndexError`), insertion-ordered `Map<K, V>` (`KeyError`), string interpolation (`"hello {x}"`), anonymous lexical closures, and `|>` pipelines.
- **TS-Style Modules & Tree-Shaking**: Supports `import { a, b } from "./mod"` and `import * as ns from "./mod"` with explicit `export` privacy and dead-code elimination of unreachable functions.
- **Errors as Values**: Functions return explicit union types (`T | Error`), inspected via exhaustive `switch` or propagated via `?`—zero exceptions.
- **Strict Numbers (`Int` & `Float`)**: 64-bit integers (`Int`) and IEEE 754 floats (`Float`) never mix implicitly; conversions (`.to_float()`, `.floor()`, `.ceil()`, `.round()`, `.to_string()`) are always explicit.
- **Explicit Schema Validation**: Mark validators with `[#validator]`, attach `@rule` / `@module.rule(...)` annotations to fields, and execute recursive checks explicitly via `validation.check()`.
- **Standard Library (`cdt.*`)**: Built-in host-native modules for JSON (`cdt.json`), schema validation (`cdt.validation`), and time (`cdt.time`).

## Standard Library

| Module | Purpose | Docs |
| :--- | :--- | :--- |
| `cdt.json` | Strict RFC 8259 typed decoding (`json.decode<T>`), deterministic encoding (`json.encode`), and unstructured `json.Value` trees (`json.parse`, `json.stringify`) | [cdt.json](docs/src/library/json.md) |
| `cdt.validation` | Field annotations, custom `[#validator]` functions, built-in `min`/`max`/`email` rules, and recursive `validation.check()` | [cdt.validation](docs/src/library/validation.md) |
| `cdt.time` | System clock (`Timestamp`), monotonic time (`Instant`), arithmetic (`Duration`), and safe `TimeError` handling | [cdt.time](docs/src/library/time.md) |

## Installation

Rust and Cargo are **not required** to use Conduit.

### macOS & Linux
```bash
curl -fsSL https://raw.githubusercontent.com/resoul/conduit/main/install.sh | sh
```

### Windows (PowerShell)
```powershell
irm https://raw.githubusercontent.com/resoul/conduit/main/install.ps1 | iex
```

## Usage

```bash
# Type-check a Conduit file
conduit check main.cdt

# Run a Conduit program
conduit run main.cdt

# Disassemble compiled VM bytecode
conduit disasm main.cdt

# Run built-in VM performance benchmarks
conduit bench --strict
```

## Documentation

For syntax guides, language rules, and standard library references, see the documentation:

- **Online Book:** [https://resoul.github.io/conduit/](https://resoul.github.io/conduit/)
- **Documentation Sources:** [docs/src/introduction.md](docs/src/introduction.md)
  - [Types & Records (`Float`, Generics, Optionals)](docs/src/language/types.md)
  - [Functions, Closures & Generics](docs/src/language/functions.md)
  - [Collections (`[T]` & `Map<K, V>`)](docs/src/language/collections.md)
  - [Errors & Unions](docs/src/language/errors.md)
  - [Modules & Privacy](docs/src/language/modules.md)
- **Build Locally:** `mdbook build docs`
- **Local Live Preview:** `mdbook serve docs` (opens at `http://localhost:3000`)

## Editor Support

Conduit provides a TextMate syntax grammar for JetBrains (RustRover), VS Code, and compatible editors. See [editors/README.md](editors/README.md) for setup instructions.

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for release notes and version history.
