# Changelog

All notable changes to the **Conduit** language are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.1.4] - 2026-11-10

### Added

* **Explicit Exports (`export func`, `export type`, `export error`)**:
  * Top-level declarations are private to the defining module by default.
  * Explicit `export` keyword exposes functions, types, and errors to importing modules. Case does not determine visibility.
  * Strict private type leakage check: exported functions, structs, or errors cannot expose private types in parameters, return types, fields, or nested in collections, tuples, unions, or function signatures.

* **Module Imports & Canonical Path Resolution**:
  * Standard library imports: `import cdt.time` (default alias `time`) and `import cdt.time as clock`.
  * Local file imports: `import "./users" as users` and `import "../shared/validation" as validation`.
  * Canonical path normalization ensures a file loaded via different relative paths creates a single module identity.
  * Cyclic import detection with clear error chains (e.g., `a.cdt -> b.cdt -> a.cdt`).

* **First-Class Functions and Callbacks Across Modules**:
  * Imported and native functions can be passed as first-class values (`op := users.find; op(42)`).
  * Seamless integration with `map(list, callback)` and `filter(list, callback)`.

* **Unified Native Registry & `cdt.time` Standard Library**:
  * Shared registry architecture (`NativeRegistry`) for native modules, functions, opaque types, and methods.
  * First standard library `cdt.time`:
    * Opaque types: `time.Timestamp`, `time.Instant`, `time.Duration`.
    * Typed error: `time.TimeError { message: String }`.
    * Functions: `time.now()`, `time.monotonic()`, `time.elapsed(start)`, `time.seconds(val)`, `time.milliseconds(val)`.
    * Methods: `Timestamp.unix_seconds()`, `Timestamp.unix_milliseconds()`, `Duration.seconds()`, `Duration.milliseconds()`, `Duration.nanoseconds()`.
    * Negative Unix timestamp flooring towards negative infinity (half a second before epoch produces `-1` Unix second).
    * `elapsed()` validates monotonic clock progression, safely handling non-negative durations and reporting invalid/reversed order as `TimeError`.

* **Reachability Analysis Tool (`conduit reachability`)**:
  * Static dead-code and dependency analyzer tracing functions and native dependencies reachable from `main`.
  * Generates summary reports showing reachable vs unused functions count and active native dependencies.

* **LSP & TextMate Grammar Support**:
  * Added `export`, `import`, and `as` keywords to syntax grammar and LSP completion.
  * Context-aware autocompletion on module dot triggers (`time.`, `<alias>.`), exposing only exported items.
  * LSP hover integrated with `NativeRegistry` documentation and function/method signatures.

* **Examples (`examples/modules/`)**:
  * Added runnable multi-module demo `users.cdt` and `main.cdt`.
  * Added `time_demo.cdt` demonstrating `cdt.time` operations with propagation and switch pattern matching.

---

## [0.1.3] - 2026-11-10

### Added

* **Typed Errors as Return Values (`error Name { ... }`)**:
  * Errors are regular typed values declared via `error Name { fields }` and returned via `return`.
  * Zero exceptions (`throw`/`try`/`catch`) and no implicit inheritance.
  * Static diagnostics prohibiting discarded error expressions at statement level and requiring all local error variables to be handled.
  * Static validation of constructor fields, types, and named arguments.

* **Union Types (`A | B`)**:
  * First-class union types supporting order independence, deduplication, and nested union flattening (`Type::union`).
  * Full static subtyping: `A` is assignable to `A | B`, and union `A | B` is assignable if every source variant satisfies a target variant.
  * Support in variable declarations, parameters, return types, and collections.
  * Direct field access on union types is disallowed without narrowing.

* **Exhaustive Pattern Matching (`switch`)**:
  * Pattern matching statement over typed values, primitives (`Int`, `String`, `Bool`), tuples, and `nil`.
  * Binding syntax: `case User(user):` binds `user` with narrowed type.
  * Static exhaustiveness check requiring all union variants to be covered.
  * Rejection of redundant, shadowed, or impossible branches.
  * Control flow analysis: if all branches of an exhaustive `switch` return, the statement is treated as guaranteed return.

* **Error Propagation Operator (`?`)**:
  * Postfix `?` evaluating expression once, immediately returning any error variant from the enclosing function, and narrowing expression to success variants.
  * Static return type validation ensuring all error variants are assignable to current function's return type.
  * Preserves `nil` when propagating from optional unions `(User?) | InvalidUserName -> User?`.
  * Strictly rejects propagation on pure optionals without error variants.

* **Tuples & Destructuring (`(A, B)`)**:
  * Multi-element tuples (2+ elements) with positional access and static type checking.
  * Tuple destructuring definition `(a, b) := expr` and `(a, _) := expr`.
  * Static check prohibiting wildcard `_` from discarding error-carrying elements.
  * Tuple patterns inside `switch` cases with automatic union type inference for matched positions.

* **Higher-Order Functions & Collection Pipelines**:
  * `map(list, callback)` and `filter(list, callback)` support function values and preserve error elements as values.

* **Language Server Protocol & TextMate Grammar**:
  * LSP hover and autocompletion for `error`, `switch`, and `case`.
  * TextMate syntax grammar updated for `error`, `switch`, `case`, and union operator `|`.

* **Examples (`examples/functions/`)**:
  * [08_errors_and_unions.cdt](examples/functions/08_errors_and_unions.cdt): Demonstrates error declarations, union return types, `?` operator, and exhaustive `switch`.
  * [09_tuples_and_guards.cdt](examples/functions/09_tuples_and_guards.cdt): Demonstrates tuples, destructuring, guard validation, and pattern matching.

---

## [0.1.2] - 2026-11-10

### Added

* **Loop Control Statements (`break` and `continue`)**:
  * Added `break` and `continue` keywords to lexer, AST, and parser.
  * Static control flow analysis in `check.rs` preventing `break` or `continue` outside of `for` loops.
  * Runtime loop flow termination and step progression in `interp.rs`.
  * LSP hover information and autocompletion snippets for loop keywords.

* **Pipeline Operator (`|>`)**:
  * Pipe operator `|>` allowing sequential data processing chains (e.g. `data |> filter() |> double() |> print()`).
  * Elegant compile-time AST desugaring in `parser.rs`: `left |> right_call(args...)` desugars into `right_call(left, args...)`, and `left |> right_ident` desugars into `right_ident(left)`.
  * Zero runtime overhead with full static type safety.

* **Examples (`examples/functions/`)**:
  * [07_loops_and_pipes.cdt](examples/functions/07_loops_and_pipes.cdt): Demonstrates `break`, `continue`, and data processing pipelines using `|>`.

---

## [0.1.1] - 2026-11-10

### Added

* **Guard Statements (`guard <cond> else { ... }`)**:
  * Added `guard` keyword for early exit precondition validation to eliminate deeply nested `if` blocks.
  * Static Control Flow analysis in `check.rs` ensuring the `else` block strictly terminates with a `return` statement (`block_always_returns`).
  * Runtime execution with early return in `interp.rs`.

* **Null-Coalescing Operator (`??`)**:
  * Binary operator `??` for safe unpacking of optional types (`custom_port ?? 8080`).
  * Static type checking verifying default value matches optional inner type (`T? ?? T -> T`).
  * Runtime short-circuit evaluation: if the left operand is not `nil`, the right operand is not evaluated.

* **Examples (`examples/functions/`)**:
  * [06_guard_and_coalesce.cdt](examples/functions/06_guard_and_coalesce.cdt): Demonstrates `guard` statements and `??` operator.

* **Language Server Protocol (`lsp.rs`) & Editor Integration**:
  * Built-in zero-dependency LSP server responding to `conduit lsp`.
  * Real-time live error diagnostics (`textDocument/publishDiagnostics`) running Lexer, Parser, and Type Checker on open and change events.
  * Context-aware hover information (`textDocument/hover`).
  * Autocompletion suggestions and snippet triggers (`textDocument/completion`).
  * TextMate syntax grammar (`editors/textmate/conduit.tmLanguage.json`) for instant syntax highlighting in JetBrains (RustRover) and VS Code.

---

## [0.1.0] - 2026-11-10

Initial release of the Conduit programming language core.

### Added

* **Lexer (`lexer.rs`)**:
  * Pure Rust zero-dependency tokenizer with line and column tracking (`Span`).
  * Literal tokens: integers (`Int`), strings (`String`), booleans (`true`, `false`, `nil`).
  * Operator tokens: `:=`, `->`, `..`, `==`, `!=`, `<=`, `>=`, `&&`, `||`, `+`, `-`, `*`, `/`, `%`.
  * Support for single-line comments (`//`).
  * CLI command: `conduit tokens <file.cdt>`.

* **Parser & AST (`parser.rs`, `ast.rs`)**:
  * Abstract Syntax Tree definitions: `Item`, `Stmt`, `Expr`, `TypeExpr`.
  * Handcrafted Recursive Descent parser with precedence climbing for binary expressions.
  * Declarations: `func name(params) -> ReturnType { body }` and `type Name { fields }`.
  * Statements: `if / else`, `for in`, `return`, `var`, variable definition `:=`, assignment `=`.
  * Method chaining (e.g. `item.name.trim().lower()`) and struct constructor calls with named arguments.
  * CLI command: `conduit ast <file.cdt>`.

* **Type Checker (`check.rs`, `types.rs`)**:
  * Static type analysis before code execution.
  * Two-pass symbol registration allowing forward references for functions and types.
  * Lexical scope stack with `Vec<HashMap<String, Type>>`.
  * Return type validation for `return` statements.
  * Argument type validation for function and method calls.
  * CLI command: `conduit check <file.cdt>`.

* **Runtime & Interpreter (`interp.rs`, `value.rs`)**:
  * Tree-walking AST interpreter.
  * Runtime values (`Value::Int`, `String`, `Bool`, `Nil`, `Void`, `List`, `Struct`).
  * Flow control with early returns (`ControlFlow::Return`).
  * String interpolation: `"{var}"` evaluated from the runtime environment.
  * Built-in functions: `print(...)`.
  * Built-in methods for strings (`trim`, `lower`, `upper`, `len`, `contains`) and lists (`len`, `append`, `contains`).
  * CLI command: `conduit run <file.cdt>` (runs static type-checking prior to execution).

* **Examples (`examples/functions/`)**:
  * [01_math_and_logic.cdt](examples/functions/01_math_and_logic.cdt): Recursion (factorial, fibonacci), math helpers, boolean logic.
  * [02_strings_and_text.cdt](examples/functions/02_strings_and_text.cdt): String concatenation, methods (`.trim()`, `.upper()`, `.contains()`).
  * [03_lists_and_loops.cdt](examples/functions/03_lists_and_loops.cdt): List filtering, loops, ranges (`1..6`), list mutation via `.append()`.
  * [04_structs_and_models.cdt](examples/functions/04_structs_and_models.cdt): Multiple structs, field access, interpolation of nested fields (`{order.user_name}`).
  * [05_options_and_nil.cdt](examples/functions/05_options_and_nil.cdt): Optional types (`Int?`), `nil` handling, safe division.

