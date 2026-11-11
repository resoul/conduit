# Control Flow

Conduit provides structured control flow mechanisms designed for clear, linear execution paths and concise transformations.

---

## Conditionals (`if` / `else`)

Conditionals evaluate a boolean condition:

```conduit
if score >= 100 {
    print("Top tier")
} else {
    print("Normal tier")
}
```

The condition must evaluate strictly to `Bool`. Conduit does not perform implicit truthy/falsy conversions on integers or strings.

---

## Loops (`for` / `in` and `while`)

The `for` loop iterates over lists, ranges, or `Map<K, V>` entries (in deterministic insertion order) and supports tuple destructuring:

```conduit
for item in [1, 2, 3, 4, 5] {
    if item == 2 {
        continue // Skip to next iteration
    }
    if item > 4 {
        break    // Terminate loop
    }
    print(item)
}

for (idx, word) in ["a", "b"].enumerate() {
    print("{idx}: {word}")
}

for (k, v) in Map.from([("x", 1), ("y", 2)]) {
    print("{k}={v}")
}
```

Use `while` loops for condition-controlled iteration (`Bool` condition required):

```conduit
var attempt = 0
while attempt < 3 {
    attempt = attempt + 1
}
```

- `break`: Immediately exits the nearest enclosing loop.
- `continue`: Skips the remainder of the current iteration and advances to the next element.

Using `break` or `continue` outside a loop body results in a compile-time error.

---

## Guard Clauses (`guard ... else`)

The `guard` statement enforces preconditions at the beginning of functions, ensuring invalid states are handled immediately before running core logic:

```conduit
func check_positive(n: Int) -> Int {
    guard n > 0 else {
        return 0 // Must terminate or return from the enclosing block
    }
    return n
}
```

> [!IMPORTANT]
> The `else` block of a `guard` statement **must** exit the current function or control flow (e.g., via `return`, `break`, or `continue`). Falling through an `else` block without exiting is diagnosed as a compile-time error.

---

## Pipeline Operator (`|>`)

The pipe operator `|>` passes the result of the left-hand expression as the first argument to the right-hand function:

```conduit
func double(x: Int) -> Int {
    return x * 2
}

func add_ten(x: Int) -> Int {
    return x + 10
}

// 5 |> double (10) |> add_ten (20)
result := 5 |> double |> add_ten
```

Pipelines can be written with or without parentheses on single-argument functions:

```conduit
"Hello, Pipeline!" |> print()
```

---

## Runnable Example

The following program demonstrates `if`/`else`, `for` loops with `break` and `continue`, `guard` clauses, and the pipeline operator `|>`:

```conduit
{{#include ../../../examples/docs/language/control_flow.cdt}}
```

### Running the Example

```bash
conduit run examples/docs/language/control_flow.cdt
```

Output:

```text
Normal
1
3
4
0
7
20
```

