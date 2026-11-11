# Changelog

All notable changes to the **Conduit** language are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.1.11] - 2026-11-11

### Added & Changed

* **JSON Field Tags (`` `json:"..."` ``)**:
  * Added Go-style `` `json:"..."` `` raw-string field tags on `type` record declarations to customize serialized and deserialized JSON key names independently of Conduit field identifiers and `@validation` rules.
  * Added compile-time validation of field tag syntax and duplicate JSON key names within a struct, plus LSP hover rendering of field tags.
  * Formatted non-identifier JSON field names in `DecodeError.path` and `EncodeError.path` using quoted bracket segments (e.g., `["a.b"]`, `items[0]["display name"]`).

* **`cdt.json` Performance & Prepared Execution Plans**:
  * Added a borrowed JSON parser (`BorrowedJsonNode<'a>`) using `Cow<'a, str>` for unescaped strings and object keys, `&'a str` number slices, slice-based string scanning, and lazy error-path construction on failure only.
  * Added arena-compiled `JsonDecodePlan` and `JsonEncodeSchema` caches (keyed by canonical `TypeId` and generic type arguments, with automatic invalidation on interpreter reuse), stack-allocated field slots for structs up to 16 fields, pre-encoded `"\"key\":"` field prefixes, and direct `Float` buffer formatting without temporary `String` allocations.
  * Updated the interpreter to borrow local variable and struct-field arguments in `json.decode` and `json.encode` without deep-cloning the input value, and added a cross-language JSON benchmark suite under `benchmarks/json/`.

* **JetBrains & TextMate Editor Setup**:
  * Added a VS Code-compatible TextMate manifest (`editors/textmate/package.json`) for `.cdt` files so RustRover and other JetBrains IDEs can load the Conduit grammar bundle.
  * Updated JetBrains editor setup documentation (`docs/src/getting-started/editors.md`, `editors/README.md`) to use LSP4IJ for configuring `conduit lsp`.

## [0.1.10] - 2026-11-11

### Added & Changed

* **Type System Soundness**:
  * Disallowed mutating captured outer `var` bindings inside closures (`Cannot mutate captured variable '...' inside a closure`), preventing silent updates lost to environment snapshotting.
  * Aligned function call resolution between `TypeChecker` and `Interpreter` so local bindings shadow top-level functions of the same name.
  * Disallowed multiple instantiations of the same generic record in a single union (`Box<Int> | Box<String>`), both in direct union annotations and after generic type parameter substitution (`Cannot combine different generic instantiations of '...' in the same union`), preventing runtime type-erasure collisions in `switch`.
  * Added contextual typing for empty list literals `[]` (`[T]` inferred from annotations, assignments, return types, function/method arguments, struct fields, `??`, and sibling list elements) while rejecting ambiguous standalone `[]` (`Cannot infer element type for empty list literal '[]'; add an explicit type annotation`).

* **String Interpolation & Syntax Fixes**:
  * Added compile-time verification of `{...}` string interpolation expressions (including dotted field paths like `{u.name}` and `{err.message}`) and support for escaped braces `\{` and `\}`.
  * Raised the precedence of `??` above comparison operators (`==`, `!=`, `<`, `<=`, `>`, `>=`) so `a ?? 0 > 5` parses as `(a ?? 0) > 5`.
  * Allowed bare `return` statements immediately before `case` inside `switch` blocks.
  * Added exact lexer/parser support for the minimum 64-bit signed integer literal `-9223372036854775808` (`i64::MIN`).

* **Runtime Safety, Performance & Hardening**:
  * Replaced unchecked `i64` arithmetic (`+`, `-`, `*`, `/`, `%`, unary `-`) and `Float` rounding conversions (`.floor()`, `.ceil()`, `.round()`, `.trunc()`) with checked operations that raise descriptive `RuntimeError`s on overflow or `NaN`/`Infinity`.
  * Enforced `MAX_CALL_DEPTH = 256` in the interpreter to prevent native stack overflow on unbounded recursion.
  * Updated mutating collection methods (`List.append`, `Map.set`, `Map.remove`) to mutate target bindings and struct fields in place without cloning the collection on each call.
  * Hardened `cdt.json`: fixed UTF-8 slicing in `\uXXXX` escape parsing, added `MAX_JSON_DEPTH = 128`, preserved `.0` on whole-number `Float` encoding (`3.0` -> `"3.0"`), and respected integer vs float syntax when decoding `Int | Float` unions.
  * Removed the `file_name()` fallback in `ReachabilityAnalyzer::paths_match` and made `Display` formatting for structs/errors and multi-module type-checking order deterministic.

## [0.1.9] - 2026-11-11

### Added & Changed

* **Optional Chaining (`?.`) & Flow-Sensitive Type Narrowing**:
  * Added `?.` optional chaining for field access (`user?.profile?.city`) and non-`Void`, non-error-union method calls (`s?.trim()?.upper()`), flattening nested optionals to `U?` without double-wrapping (`U??`) and short-circuiting argument and tail evaluation on `nil`.
  * Added flow-sensitive narrowing of `T?` to `T` via `guard x != nil else { ... }`, `if x != nil { ... } else { ... }`, `if x == nil { ... } else { ... }`, `while x != nil { ... }`, and short-circuit boolean expressions (`x != nil && ...`, `x == nil || ...`), with automatic reset back to `T?` upon assigning `nil` / `T?` / `Any` to mutable `var` bindings.
  * Updated the interpreter to short-circuit `&&` and `||` right-hand operands so guards like `x == nil || x.field == 0` never dereference `nil`.

* **Extended Loops, Destructuring, and Safe Mutation**:
  * Added `while cond { ... }` precondition loops requiring strict `Bool` conditions, supporting `break` and `continue` isolated across closure boundaries.
  * Extended `for` loops with tuple pattern destructuring (`for (idx, item) in items.enumerate()`, wildcard `_` with error-discard checks) and direct insertion-order iteration over `Map<K, V>` (`for (k, v) in map`, `for entry in map`).
  * Enabled `m[key] = value` index assignment on mutable `var` maps, nested struct field assignment (`order.meta.updated = true`), and mutating method calls on fields of `var` structs (`order.items.append(x)`), while enforcing immutability on `:=` bindings (`Cannot mutate immutable variable '...'`).
  * Added list methods `.slice(start, end)`, `.reverse()`, `.enumerate()`, and `[String].join(sep)`, plus Unicode-scalar-indexed string methods `.index_of(sub) -> Int?` and `.slice(start, end) -> String`.

* **Strict String Number Parsing & Prelude `ParseError`**:
  * Added prelude error `ParseError { value: String, message: String }` (shadowable by local user definitions) and methods `String.to_int() -> Int | ParseError` and `String.to_float() -> Float | ParseError` (rejecting whitespace without `.trim()`, `i64` overflow, and non-finite floats such as `"NaN"` / `"Infinity"`).
  * Updated LSP hover/completion entries, TextMate syntax highlighting, and language documentation.

## [0.1.8] - 2026-11-11

### Added & Changed

* **User-Defined Generic Functions & Records**:
  * Added generic record declarations `type Name<T, ...> { ... }` and generic function declarations `func name<T, ...>(...) -> ...` with validation rejecting empty (`<>`) and duplicate (`<T, T>`) type parameter lists.
  * Added explicit generic type arguments on type annotations (`Page<User>`, `alias.Page<Int>`, nested `Box<[Pair<String, Int>]>`) and function calls (`first_or_default<Int>([], 0)`), alongside type argument inference from call arguments with conflict detection (`Conflicting types inferred for type parameter 'T'`).
  * Added `Type::TypeParam` and canonical `Type::Generic { type_id, type_args }` identity so `Box<Int>` and `Box<String>` are strictly distinct, generic `switch` branches bind concrete field types, and `export` privacy recursively inspects generic type arguments.
  * Updated `ReachabilityAnalyzer` and LSP hover/completions to traverse generic type arguments.

* **Ordered Dictionary Type (`Map<K, V>`)**:
  * Added built-in `Map<K, V>` constrained to `String`, `Int`, or `Bool` keys (`"Map key type must be 'String', 'Int', or 'Bool', got '...'"`), with deterministic insertion ordering (`set` overwrites preserve original key position; `remove` then re-`set` appends to end).
  * Added `Map<K, V>()` and `Map.from([(k, v), ...])` constructors, safe `map[key] -> V | KeyError` indexing with prelude `KeyError { key: String, message: String }`, and `.set`, `.get`, `.remove`, `.contains_key`, `.len`, `.keys`, `.values`, and `.entries` methods.
  * Integrated `Map<K, V>` and generic structs with `cdt.validation` (`min`/`max` on map entry counts and recursive `validation.check` producing formatted paths such as `users_by_id["alice"].email` and `items[42].title`).

* **Floating-Point Type (`Float`) & Strict Conversions**:
  * Added `Float` (`f64`, IEEE 754) literals (`0.0`, `3.14`, `-42.5`), arithmetic (`+`, `-`, `*`, `/`), unary `-`, comparisons (`<`, `<=`, `>`, `>=`, `==`, `!=`), and runtime `Division by zero` checks to prevent silent `NaN`/`Infinity` propagation.
  * Preserved strict separation from `Int` (no implicit mixing), with explicit `Int.to_float()` and `Float` methods `.floor()`, `.ceil()`, `.round()`, `.trunc()`, `.to_string()`, `.is_nan()`, `.is_infinite()`, and `.is_finite()`.
  * Extended `cdt.validation` `min` / `max` bounds and custom `[#validator]` literal arguments (including negative floats) to support `Float`, rejecting non-finite `NaN` and `Infinity` values.

* **Standard Library `cdt.json`**:
  * Implemented zero-dependency RFC 8259 JSON parser and serializer in pure Rust `std`, exporting `json.Value`, `json.DecodeError { path, message }`, and `json.EncodeError { path, message }`.
  * Added typed `json.decode<T>(raw)` enforcing strict field presence, `null` rules, unknown field rejection, exact 64-bit integer bounds (rejecting fractions, exponents, and overflows), non-ambiguous union checking, and exact violation paths without implicitly triggering `@`-validators.
  * Added deterministic `json.encode(value)` preserving struct declaration field order and `Map` insertion order, plus unstructured `json.parse`, `json.stringify`, and `.is_null()` / `.as_*()` navigation helpers on `json.Value`.

## [0.1.7] - 2026-11-11

### Added & Changed

* **Anonymous Functions & Lexical Closures**:
  * Added `func(param: Type, ...) -> ReturnType { ... }` expression syntax (omitted return type defaults to `Void`) usable in assignments, direct calls, `map`/`filter`, pipes (`|>`), and higher-order returns.
  * Closures capture enclosing lexical bindings by value snapshot at creation time and preserve their defining module context so cross-module closures continue resolving private helpers and module types accurately.
  * Type checker isolates closure `current_return_type` (`return` and `?` apply to the closure itself) and resets `loop_depth` (`break`/`continue` cannot cross closure boundaries), enforces `block_always_returns` for non-`Void` closures, and rejects unhandled error variables inside closures.
  * Updated `ReachabilityAnalyzer` to scan parameter types, return types, and bodies of closure expressions.

* **String Methods & Explicit `.to_string()`**:
  * Added `String` methods: `.split(sep: String) -> [String]`, `.starts_with(prefix: String) -> Bool`, `.ends_with(suffix: String) -> Bool`, `.replace(from: String, to: String) -> String` (returning the original string unchanged when `from == ""`), and `.char_count() -> Int` (Unicode scalar count).
  * Documented `String.len()` as UTF-8 byte length (`O(1)`) in contrast with `.char_count()` and `cdt.validation` `min`/`max` character-count bounds (Unicode scalar values).
  * Added explicit `.to_string() -> String` on `Int`, `Bool`, and `String` while preserving strict non-coercion for `+`.
  * Updated LSP hover/completion and documentation examples for closures, strings, and `.to_string()`.

## [0.1.6] - 2026-11-11

### Added & Changed

* **Canonical Type Identity Across Compiler & Runtime**:
  * Replaced suffix and short-name matching with canonical `TypeId` (`ModuleId` + type name) distinguishing `Builtin`, `Program`, canonical `Local(PathBuf)`, and `Native(String)` modules.
  * Updated `TypeChecker`, `Value::Struct` / `Value::Error`, `NativeValue`, `switch` pattern matching, native method lookup, and validation schema resolution (`module_struct_fields`) to use canonical `TypeId`.
  * Two types with identical names declared in different modules (e.g. `a.User` vs `b.User`) are now strictly distinct and incompatible across compile-time checks, runtime `switch`, and `cdt.validation.check`.
  * Importing the same module under different aliases consistently resolves to the exact same canonical type.

* **Safe List Indexing & Prelude `IndexError`**:
  * Added list index operator `list[index]` returning `T | IndexError` for `[T]` with `Int` index.
  * Added prelude error `IndexError { index: Int, length: Int, message: String }` (available without imports). Both `index < 0` and `index >= list.len()` produce `IndexError`.
  * Added `list.get(index: Int) -> T?` returning `nil` for out-of-bounds or negative indices without double-wrapping `U?` elements.
  * Updated `ReachabilityAnalyzer` and parser postfix chaining to support indexed expressions (such as `err.issues[0]?` and `matrix[r]?[c]?`).

## [0.1.5] - 2026-11-10

### Added

* **Custom Validators and Field Annotations**:
  * Function marker `[#validator]` (case-insensitive) to declare validator functions directly preceding `func` or `export func`.
  * Strict validator function contract: `func rule(value: T, extra_params...) -> Void | validation.Issue`.
  * Struct field annotations: `@rule` or `@module.rule(args...)` executed in declaration order.
  * Parameterized validator annotations supporting positional literals, including negative integer literals (e.g. `@range(-10, 100)`).
  * Optional field skipping: fields of type `T?` validate `T` when present and automatically skip validation when `nil`.
  * Preserved first-class function semantics: validators can be passed as callbacks or invoked directly.

* **Standard Validation Library (`cdt.validation`)**:
  * Public types:
    * `error Issue { code: String, message: String }`: Single rule violation.
    * `type Violation { path: String, code: String, message: String }`: Attributed field violation with path (e.g. `user.profile.email` or `items[0].code`).
    * `error ValidationError { issues: [Violation] }`: Aggregated errors for `switch` and `?` handling.
  * Standard rules:
    * `min(value, min_val)` and `max(value, max_val)`: bounds checking for `Int`, Unicode scalar count for `String`, and element count for `List`.
    * `email(value: String)`: format validation for email addresses.
    * `check(value: T) -> T | validation.ValidationError`: explicit recursive validation preserving value and types on success without mutation.
  * Reachability analysis integration: struct field annotations are traced as function references, ensuring referenced validators and their transitive dependencies are preserved.

* **Official Documentation on mdBook**:
  * Complete documentation site structure in `docs/` written entirely in English.
  * Verified runnable documentation examples under `examples/docs/` covering getting started, core language, standard library, and practical recipes.
  * Negative example tests (`examples/docs/negative/`) verifying diagnostics for discarded errors, private type leakage, and invalid `?` on optionals.
  * Automated documentation example verification test suite (`tests/doc_examples_test.rs`).
  * Deterministic API markdown documentation generator from `NativeRegistry` (`conduit doc [module]`).
  * Custom syntax highlighting script for mdBook (`docs/theme/conduit-highlight.js`) supporting `conduit` and `cdt` code blocks.
  * Dedicated GitHub Actions workflow (`.github/workflows/docs.yml`) for building, verifying, and publishing documentation to GitHub Pages.

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

