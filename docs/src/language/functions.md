# Functions

Functions are fundamental building blocks in Conduit. They are declared with the `func` keyword and can be passed as first-class values.

---

## Function Declarations

A function declaration specifies parameter names, their types, and an optional return type:

```conduit
func add(a: Int, b: Int) -> Int {
    return a + b
}
```

If a function does not return a value, the `-> ReturnType` clause is omitted (or specified as `-> Void`):

```conduit
func greet(name: String) {
    print("Welcome, " + name)
}
```

---

## First-Class Functions & Callbacks

In Conduit, functions are first-class values. They can be stored in variables, passed as arguments to other functions, and returned from higher-order functions.

### Function Type Signatures

Function types are written as `(ParamTypes) -> ReturnType`:

```conduit
func apply_binary(op: (Int, Int) -> Int, x: Int, y: Int) -> Int {
    return op(x, y)
}
```

Here, `op` is any function taking two `Int` parameters and returning an `Int`.

### Passing Functions

You can pass a declared function directly by name without parentheses:

```conduit
res := apply_binary(add, 10, 20)
print(res) // Prints 30
```

## Anonymous Functions & Closures

You can create anonymous functions inline using `func` expressions:

```conduit
double := func(x: Int) -> Int {
    return x * 2
}

// Omitting `-> Type` declares a Void closure
notify := func(msg: String) {
    print(msg)
}
```

Closures capture enclosing local variables from their lexical scope **by value snapshot** at creation time and execute inside the module where they were defined. Reassigning a `var` in the outer scope after creating the closure does not alter the captured value inside the closure.

Control-flow rules inside closures are scoped to the closure itself:
- `return` and error propagation (`?`) apply to the closure's own return type, not an enclosing function.
- `break` and `continue` inside a closure may only target loops declared within that same closure.

---

## Runnable Example

The following program demonstrates function declarations, void procedures, higher-order callback execution, anonymous lambdas, and lexical closures:

```conduit
{{#include ../../../examples/docs/language/functions.cdt}}
```

### Running the Example

```bash
conduit run examples/docs/language/functions.cdt
```

Output:

```text
42
30
Welcome, Explorer
42
15
```

---

## Common Errors & Diagnostics

### Passing Incompatible Function Signature

If a callback signature does not match the expected parameter or return types:

```conduit
func is_positive(n: Int) -> Bool {
    return n > 0
}

res := apply_binary(is_positive, 10, 20) // Error!
```

Diagnostic:

```text
Type error: Type mismatch: expected '(Int, Int) -> Int', found '(Int) -> Bool'
```

