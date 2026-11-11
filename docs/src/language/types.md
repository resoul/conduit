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

Conduit never performs implicit string coercion with `+`. Convert `Int`, `Float`, or `Bool` explicitly via `.to_string()`:

```conduit
count := 42
ratio := 3.5
ok := true
msg := "count=" + count.to_string() + ", ratio=" + ratio.to_string() + ", ok=" + ok.to_string()
```

### `Float` & Strict Numeric Typing

Conduit enforces strict separation between `Int` and `Float` (`f64`): arithmetic (`+`, `-`, `*`, `/`), unary `-`, and comparisons (`<`, `<=`, `>`, `>=`, `==`, `!=`) require matching operand types. Dividing a `Float` by `0.0` raises a `Runtime error: Division by zero` rather than propagating `NaN` or `Infinity`.

- Convert integers explicitly via `n.to_float() -> Float`.
- Convert or inspect floats via `.floor() -> Int`, `.ceil() -> Int`, `.round() -> Int`, `.trunc() -> Int`, `.to_string() -> String`, `.is_nan() -> Bool`, `.is_infinite() -> Bool`, and `.is_finite() -> Bool`.

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

### Optional Chaining (`?.`) and Flow-Sensitive Narrowing

Use `?.` to safely traverse optional fields and call non-`Void`, non-error-returning methods without nested `switch` blocks. Chained accesses flatten cleanly to `U?` and short-circuit immediately on `nil`:

```conduit
city_upper := user?.profile?.city?.upper() ?? "UNKNOWN"
```

Comparing an optional binding against `nil` automatically narrows `T?` to `T` inside `guard x != nil else { ... }`, `if x != nil { ... }`, `while x != nil { ... }`, and short-circuit boolean conditions (`x != nil && x.active`, `x == nil || x.active`). Assigning `nil` back to a `var x: T?` resets the narrowed type back to `T?`.

### Strict String Number Parsing (`ParseError`)

Parse strings explicitly into numbers using `.to_int() -> Int | ParseError` and `.to_float() -> Float | ParseError`, where `ParseError { value: String, message: String }` is built into the prelude:

```conduit
n := "42".to_int()?
f := "3.14".to_float()?
```

Both methods reject surrounding whitespace unless `.trim()` is called first, and `.to_float()` rejects non-finite values such as `"NaN"` or `"Infinity"`. Strings also provide Unicode-scalar-indexed `.index_of(sub: String) -> Int?` and `.slice(start: Int, end: Int) -> String`.

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

### Generic Record Types (`type Name<T, ...>`)

Records can declare non-empty, unique type parameters to parameterize their fields across arbitrary concrete types (including nested generic containers):

```conduit
type Page<T> {
    items: [T]
    total: Int
}

type Pair<A, B> {
    first: A
    second: B
}

var page: Page<String> = Page(items: ["a", "b"], total: 2)
```

Different type arguments produce distinct, incompatible types (`Page<Int>` cannot be assigned to `Page<String>`). When pattern-matching generic records in a `switch`, match the base type name (`case Page(p):`) and `p` receives its concrete instantiated field types. Public `export` types and functions cannot leak private types inside generic type arguments.

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

