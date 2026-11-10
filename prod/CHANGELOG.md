# Changelog

All notable changes to the **Conduit** language are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

