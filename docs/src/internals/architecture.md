# Architecture & Pipeline

Conduit is implemented as a clean, multi-stage compiler and execution pipeline written in pure Rust with zero external dependencies.

---

## Compiler & Runtime Pipeline

When you execute `conduit run main.cdt` or `conduit check main.cdt`, source files pass through the following pipeline:

```text
Source Code (.cdt)
       │
       ▼
  [ 1. Lexer ]           ──> Token Stream (conduit tokens)
       │
       ▼
  [ 2. Parser ]          ──> Abstract Syntax Tree (AST) (conduit ast)
       │
       ▼
[ 3. Module Loader ]     ──> Module Dependency Graph & Cycle Detection
       │
       ▼
[ 4. Type Checker ]      ──> Static Semantic Analysis & Error Verification (conduit check)
       │
       ├─────────────────> [ 5. Reachability Analyzer ] (conduit reachability)
       │
       ▼
[ 6. Interpreter ]       ──> Tree-Walk AST Execution (conduit run)
       │
       ▼
[ 7. Native Registry ]   ──> Standard Library Host Bridge (cdt.time)
```

---

## Detailed Stages

### 1. Lexer (`src/lexer.rs`)
The lexer scans the raw UTF-8 source string character by character, emitting strongly typed tokens (`TokenKind`) along with precise source coordinates (`Span { line, col }`). Comments (`//`) and whitespace are stripped.

### 2. Parser (`src/parser.rs`)
A hand-written recursive descent parser constructs the Abstract Syntax Tree (AST). It handles operator precedence (such as the pipeline operator `|>`), desugaring, pattern structures, and control flow blocks without external parser generators.

### 3. Module Loader (`src/loader.rs`)
The module loader reads relative imports starting from the root file, canonicalizes cross-platform paths, loads dependent `.cdt` files into a module graph, and verifies that no circular dependencies exist.

### 4. Type Checker (`src/check.rs`)
The static type checker traverses the loaded project AST to enforce:
- Type compatibility across assignments, parameters, and returns.
- Exhaustiveness and non-redundancy of `switch` pattern matching.
- Strict error checking: error types cannot be discarded or ignored.
- Safety of the `?` try operator (ensuring it is only used with error-returning functions).
- Module privacy boundaries: unexported symbols cannot be accessed, and private types cannot leak in public signatures.

### 5. Reachability Analyzer (`src/reachability.rs`)
Starting from `func main()` in the root module, the analyzer traces all function call expressions and callback references across the entire loaded project. It identifies dead code and records all accessed host native functions.

### 6. Interpreter (`src/interp.rs`)
A high-performance tree-walk interpreter evaluates AST nodes directly. Scopes and variable environments are managed dynamically without requiring an intermediate bytecode VM or JIT compiler.

### 7. Native Registry (`src/native/`)
The `NativeRegistry` hosts all built-in standard library implementations (such as `cdt.time`). It provides type-safe boundaries, preventing Conduit programs from mutating host state arbitrarily while enabling deterministic documentation generation (`conduit doc`).

---

## Source Code Organization

The core compiler modules in `src/` correspond directly to the pipeline stages:
- `src/lexer.rs`: Token scanner and lexical error diagnostics.
- `src/parser.rs`: Recursive-descent grammar and AST construction.
- `src/loader.rs`: Path canonicalization, module loading, and cycle detection.
- `src/check.rs`: Type system, pattern exhaustiveness, and visibility checks.
- `src/reachability.rs`: Dead code analysis and call graph tracing.
- `src/interp.rs`: Tree-walk execution engine and value representations.
- `src/native/`: Unified registry and host-native implementations (`cdt.time`).


