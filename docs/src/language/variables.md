# Variables & Mutability

Conduit offers concise and type-safe variable declarations. Variables can have their types automatically inferred by the compiler or explicitly annotated.

---

## Variable Definitions

### Inferred Type Definitions (`:=`)

The `:=` operator declares a new variable and infers its type from the right-hand expression:

```conduit
name := "Conduit"
version := 1
```

Here, `name` is inferred as `String` and `version` as `Int`.

### Explicit Type Declarations (`var`)

To explicitly annotate the type of a variable, use the `var` keyword:

```conduit
var count: Int = 10
```

### Mutability & Reassignment

Variables declared with `var` can be reassigned with the assignment operator `=`:

```conduit
var score: Int = 0
score = score + 5
score = score * 2
```

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

