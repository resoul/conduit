# Working with Errors

In Conduit, error handling is not an afterthought handled with out-of-band exceptions. Errors are first-class domain values that compose cleanly with the type system.

This guide outlines recommended architectural patterns for designing and handling errors in production Conduit applications.

---

## 1. Model Domain Errors Explicitly

Avoid using generic string error codes or untyped error payloads. Instead, declare distinct `error` records for each failure domain:

```conduit
export error ValidationError {
    field: String
    detail: String
}

export error ParseError {
    input: String
}
```

By declaring concrete fields (such as `field` and `detail`), callers can inspect error causes programmatically rather than attempting to parse brittle error message strings.

---

## 2. Decide When to Propagate vs. When to Handle

Conduit gives you two complementary tools for handling errors:
- **Use `?` (Try Operator)**: When the current function cannot reasonably resolve the failure and must forward it to its caller.
- **Use `switch` (Pattern Matching)**: At boundary layers (such as HTTP handlers, CLI commands, or UI controllers) where you must resolve failures and convert them into user-facing output or fallback behavior.

```conduit
func process_input(s: String) -> Int | ValidationError | ParseError {
    // Propagate early to the caller if parsing fails:
    val := parse_positive_int(s)?
    return val * 2
}
```

---

## 3. Handling Fallible Operations in Collections

Because `map` preserves errors as elements in the resulting list, processing a collection of inputs produces a list of union values: `[Int | ValidationError | ParseError]`.

You can iterate through the list with a `for` loop and dispatch results with `switch`:

```conduit
for item in results {
    switch item {
        case Int(val):
            print("Successfully processed value")
        case ValidationError(ve):
            print("Validation error: " + ve.detail)
        case ParseError(pe):
            print("Parse error: " + pe.input)
    }
}
```

---

## Complete Runnable Recipe

The following standalone recipe demonstrates input validation, error propagation through helper functions, and exhaustive resolution in `main()`:

```conduit
{{#include ../../../examples/docs/guides/error_recipe.cdt}}
```

### Running the Recipe

```bash
conduit run examples/docs/guides/error_recipe.cdt
```

Output:

```text
Processed integer:
84
Validation failed on input: empty string
Could not parse: unknown
```

