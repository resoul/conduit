# Variables & Mutability

Conduit offers concise and type-safe variable declarations. Variables can have their types automatically inferred by the compiler or explicitly annotated.

---

## Variable Definitions

### Inferred Immutable Bindings (`:=`)

The `:=` operator declares a new **immutable** local binding and infers its type from the right-hand expression:

```conduit
name := "Conduit"
version := 1
```

Here, `name` is inferred as `String` and `version` as `Int`. Reassigning a binding declared with `:=` (or a function/closure parameter) is a compile-time error—use `:=` by default unless reassignment is actually needed.

### Mutable Variable Declarations (`var`)

Use the `var` keyword when a variable needs to be reassigned (with an optional type annotation):

```conduit
var count: Int = 10
var score = 0
score = score + 5
score = score * 2
```

### Summary of Reassignment Rules

* Bindings introduced with `:=` cannot be reassigned with `=`.
* Bindings introduced with `var` can be reassigned with `=`.

---

## Runnable Example

```conduit
func main() -> Void {
    name := "Conduit"
    version := 2
    var count: Int = 10
    print(name)
    print(version)
    print(count)
    count = count + 5
    print(count)
}
```

### Running the Example

```bash
conduit run variables.cdt
```

Output:

```text
Conduit
2
10
15
```

---

## Scoping & Shadowing

Variables are scoped to the block `{ ... }` in which they are defined. A nested block can introduce a new variable with the same name as an outer variable, shadowing it within the inner scope:

```conduit
x := 10
if x > 5 {
    x := 99
    print(x) // Prints 99
}
print(x)     // Prints 10
```

---

## Module Constants (`const`)

Top-level constants can be declared at the module level with `const` (and exported across modules with `export const`). Constant initializers are evaluated at compile time and cannot be reassigned:

```conduit
const MAX_RETRIES = 3
export const DEFAULT_PORT: Int = 8080
export const API_PREFIX = "/v1"
```

Supported constant types are `Int`, `Float`, `Bool`, and `String` (including arithmetic, boolean logic, and references to earlier constants).

---

## Common Errors & Diagnostics

### Reassigning an Incompatible Type

Conduit strictly enforces static typing during assignment:

```conduit
var counter: Int = 0
counter = "text" // Error!
```

Diagnostic:

```text
Type error: Cannot assign value of type 'String' to variable 'counter' of type 'Int'
```

