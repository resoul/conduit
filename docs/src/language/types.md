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
| `.len()` | `() -> Int` | Returns UTF-8 byte length (`O(1)`). |
| `.is_empty()` | `() -> Bool` | Returns `true` when the string has length `0`. |
| `.trim()` | `() -> String` | Strips leading and trailing whitespace. |
| `.lower()` / `.upper()` | `() -> String` | Converts text to lowercase or uppercase. |
| `.contains(sub)` | `(String) -> Bool` | Checks whether `sub` occurs in the string. |
| `.starts_with(prefix)` | `(String) -> Bool` | Checks whether the string begins with `prefix`. |
| `.ends_with(suffix)` | `(String) -> Bool` | Checks whether the string ends with `suffix`. |
| `.split(sep)` | `(String) -> [String]` | Splits on `sep`. |
| `.replace(from, to)` | `(String, String) -> String` | Replaces all occurrences of `from` with `to` (returns the original string unchanged when `from == ""`). |
| `.to_int()` | `() -> Int \| ParseError` | Parses a strict decimal integer without surrounding whitespace. |
| `.to_float()` | `() -> Float \| ParseError` | Parses a finite floating-point number without surrounding whitespace. |
| `.to_string()` | `() -> String` | Identity conversion on `String`. |

> **Note:** `cdt.validation` constraints `min` and `max` measure string character count in Unicode scalar values, while `String.len()` returns raw UTF-8 bytes.

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
- Round or format floats via `.floor() -> Float`, `.ceil() -> Float`, `.round() -> Float`, and `.to_string() -> String`.
- Both `Int` and `Float` provide `.abs()`, `.min(other)`, `.max(other)`, and `.clamp(lo, hi)`.

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

### Optional Chaining (`?.`)

Access fields on an optional struct (`T?`) safely with `?.`. If the target is `nil`, the expression evaluates to `nil` without evaluating further chained fields:

```conduit
city := user?.address?.city ?? "Unknown"
```

### Flow-Sensitive Narrowing

Comparing an optional binding against `nil` narrows `T?` to `T` inside `guard x != nil else { ... }`, `if x != nil { ... }`, and `while x != nil { ... }`. Assigning `nil` back to a `var x: T?` resets the narrowed type back to `T?`.

### Strict String Number Parsing (`ParseError`)

Parse strings explicitly into numbers using `.to_int() -> Int | ParseError` and `.to_float() -> Float | ParseError`, where `ParseError { value: String, message: String }` is built into the prelude:

```conduit
n := "42".to_int()?
f := "3.14".to_float()?
```

Both methods reject surrounding whitespace unless `.trim()` is called first, and `.to_float()` rejects non-finite values such as `"NaN"` or `"Infinity"`.

> [!IMPORTANT]
> **Difference between `T?` and `T | Error`:**
> - `T?` (an optional) indicates mere presence or absence of a value. It does not carry diagnostic error details and cannot be unwrapped with the `?` try operator.
> - `T | Error` (a union with an error type) indicates a fallible operation. It carries detailed error fields and can be propagated using the `?` try operator.

---

## Custom Struct Types (`struct`)

Custom data structs are defined with the `struct` keyword:

```conduit
struct Point {
    x: Int
    y: Int
}
```

Struct fields can also specify default values (`= expr`), allowing omitted fields in constructors:

```conduit
struct Config {
    host: String = "localhost"
    port: Int = 8080
}

cfg := Config(port: 9090) // host defaults to "localhost"
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

### Generic Struct Types (`struct Name<T, ...>`)

Structs can declare non-empty, unique type parameters to parameterize their fields across arbitrary concrete types (including nested generic containers):

```conduit
struct Page<T> {
    items: [T]
    total: Int
}

struct Pair<A, B> {
    first: A
    second: B
}

var page: Page<String> = Page(items: ["a", "b"], total: 2)
```

Different type arguments produce distinct, incompatible types (`Page<Int>` cannot be assigned to `Page<String>`). When pattern-matching generic records in a `switch`, match the base type name (`case Page(p):`) and `p` receives its concrete instantiated field types. Public `export` types and functions cannot leak private types inside generic type arguments.

---

## Runnable Example

```conduit
struct Point {
    x: Int
    y: Int
}

---

## Enums (`enum`)

Enums define nominal scalar types with a fixed set of named variants (`export enum` makes them importable from other modules):

```conduit
enum Role {
    Admin,
    User,
    Guest
}

func label(r: Role) -> String {
    switch r {
        case Role.Admin:
            return "admin"
        case Role.User:
            return "user"
        case _:
            return "guest"
    }
}
```

Enums support `==`, `!=`, `switch` matching, lists (`[Role]`), and maps (`Map<Role, String>`) with zero heap allocation.

---

func main() -> Void {
    var score: Int? = nil
    print(score ?? 100)
    pt := Point(x: 10, y: 20)
    print("{pt.x}:{pt.y}")
}
```

### Running the Example

```bash
conduit run types.cdt
```

Output:

```text
100
10:20
```

