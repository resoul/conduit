# Errors & Unions

Conduit eliminates unchecked runtime exceptions in favor of **typed errors as first-class values**.

---

## Defining Error Types (`error`)

Errors are declared using the `error` keyword:

```conduit
error NotFound {
    id: Int
}

error PermissionDenied {
    user: String
}
```

Error types are concrete records with typed fields, carrying relevant debugging and operational metadata.

---

## Returning Errors & Union Types (`T | Error`)

Fallible functions express their possible outcomes explicitly in their return type using union syntax (`|`):

```conduit
func fetch_resource(id: Int) -> String | NotFound {
    guard id > 0 else {
        return NotFound(id: id)
    }
    return "Resource data"
}
```

### Prohibition Against Discarding Errors

In many languages, forgetting to check an error code leads to silent failure. In Conduit, **the compiler forbids discarding expressions that evaluate to error types**:

```conduit
// Error! Cannot discard fallible result:
fetch_resource(42)
```

Diagnostic:

```text
Type error: Expression result of type 'String | NotFound' contains an error and must not be discarded
```

---

## The Try Operator (`?`)

The `?` operator provides ergonomic error propagation. When applied to an expression whose type is a union containing an error variant:
- If the value is a success variant, it unwraps the inner value.
- If the value is an error variant, it immediately returns the error from the enclosing function.

```conduit
func authorize_and_fetch(id: Int, user: String) -> String | NotFound | PermissionDenied {
    guard user == "admin" else {
        return PermissionDenied(user: user)
    }
    // Automatically propagates NotFound if fetch_resource fails
    data := fetch_resource(id)?
    return data
}
```

> [!IMPORTANT]
> The `?` operator is **only** permitted on types containing error variants, and only inside functions whose return type includes those error types.
> Using `?` on a pure optional (`Int?`) is rejected by the compiler. Use null-coalescing (`??`) for optionals.

---

## Pattern Matching (`switch`)

The `switch` statement performs exhaustive pattern matching over union types:

```conduit
res := authorize_and_fetch(42, "admin")
switch res {
    case String(data):
        print("Success: " + data)
    case NotFound(e):
        print("Missing item")
    case PermissionDenied(e):
        print("Access denied for " + e.user)
}
```

### Compiler Guarantees for `switch`:
1. **Exhaustiveness**: Every possible variant of the union must be handled. Leaving out a variant causes a compile-time error.
2. **No Redundant Cases**: Specifying multiple cases for the same variant is diagnosed as an error.
3. **Payload Binding**: Payload variables (like `e` in `case NotFound(e):`) are bound with their exact concrete type within the case block.

---

## Runnable Example

The following program demonstrates error declarations, propagation with `?`, and exhaustive `switch` pattern matching:

```conduit
{{#include ../../../examples/docs/language/errors.cdt}}
```

### Running the Example

```bash
conduit run examples/docs/language/errors.cdt
```

Output:

```text
Success: Resource data
Missing item id:
-1
Access denied for guest
```

