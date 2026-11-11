# Types & Records

Conduit is a statically typed language with a strong, expressive type system. Every expression has a type known at compile time.

---

## Primitive Types

Conduit features built-in primitive types:

| Type | Description | Examples |
| :--- | :--- | :--- |
| `Int` | 64-bit signed integer | `0`, `42`, `-10` |
| `Float` | 64-bit floating-point number | `3.14`, `-0.5` |
| `Bool` | Boolean truth value | `true`, `false` |
| `String` | UTF-8 encoded text string | `"hello"`, `"Conduit"` |
| `Void` | Represents the absence of a meaningful return value | Returned by functions without a return value |

---

### String Methods & Length Semantics

`String` values expose methods for inspection, transformation, and splitting:

| Method | Signature | Behavior |
| :--- | :--- | :--- |
| `.len()` | `() -> Int` | Returns **UTF-8 byte length** (`O(1)`). Use `.char_count()` when counting characters. |
| `.char_count()` | `() -> Int` | Returns the count of **Unicode scalar values** (`"café".char_count() == 4`, whereas `"café".len() == 5`). |
| `.trim()` | `() -> String` | Strips leading and trailing whitespace. |
| `.lower()` / `.upper()` | `() -> String` | Converts ASCII/Unicode text to lowercase or uppercase. |
| `.contains(sub)` | `(String) -> Bool` | Checks whether `sub` occurs in the string. |
| `.starts_with(prefix)` | `(String) -> Bool` | Checks whether the string begins with `prefix`. |
| `.ends_with(suffix)` | `(String) -> Bool` | Checks whether the string ends with `suffix`. |
| `.split(sep)` | `(String) -> [String]` | Splits on `sep` (returns `[s]` if `sep` is not found; `[""]` when `s == ""` and `sep != ""`). |
| `.replace(from, to)` | `(String, String) -> String` | Replaces all occurrences of `from` with `to` (returns the original string unchanged when `from == ""`). |
| `.to_string()` | `() -> String` | Identity conversion on `String`. |

> **Note:** `cdt.validation` constraints `min` and `max` measure string character count in Unicode scalar values (matching `.char_count()`), while `String.len()` returns raw UTF-8 bytes.

### Explicit `.to_string()` Conversion

Conduit never performs implicit string coercion with `+`. Convert `Int` or `Bool` explicitly via `.to_string()`:

```conduit
count := 42
ok := true
msg := "count=" + count.to_string() + ", ok=" + ok.to_string()
```

## Optional Types (`T?`) and `nil`

Any type can be marked as optional by appending a question mark `?`:

```conduit
var maybe_user: String? = nil
```

An optional type represents a value that is either of type `T` or `nil`.

### Null-Coalescing Operator (`??`)

The null-coalescing operator `??` unwraps an optional value, providing a fallback value if the left operand is `nil`:

```conduit
var score: Int? = nil
actual_score := score ?? 0 // Evaluates to 0

score = 42
updated_score := score ?? 0 // Evaluates to 42
```

> [!IMPORTANT]
> **Difference between `T?` and `T | Error`:**
> - `T?` (an optional) indicates mere presence or absence of a value. It does not carry diagnostic error details and cannot be unwrapped with the `?` try operator.
> - `T | Error` (a union with an error type) indicates a fallible operation. It carries detailed error fields and can be propagated using the `?` try operator.

---

## Custom Record Types (`type`)

Custom data records are defined with the `type` keyword:

```conduit
type Point {
    x: Int
    y: Int
}
```

### Instantiation

Records are instantiated using named arguments matching their field names:

```conduit
pt := Point(x: 10, y: 20)
```

### Field Access

Fields are accessed using dot notation:

```conduit
print(pt.x) // 10
print(pt.y) // 20
```

---

## Runnable Example

The following complete program demonstrates primitive types, optional unwrapping with `??`, record definition, and field access:

```conduit
{{#include ../../../examples/docs/language/types.cdt}}
```

### Running the Example

```bash
conduit run examples/docs/language/types.cdt
```

Output:

```text
42
hello
true
100
25
10
20
5:4
2
```

