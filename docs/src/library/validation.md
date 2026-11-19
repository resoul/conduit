# cdt.validation

The `cdt.validation` standard library provides schema validation primitives, standard validation rules (`min`, `max`, `email`), and an explicit evaluation engine via `validation.check()`.

```conduit
import * as validation from "cdt.validation"
```

---

## Conceptual Overview

### Validator Functions

A validator is a function marked with `[#validator]` that adheres to the contract:

```conduit
[#validator]
export func rule_name(value: T, extra_params...) -> Void | validation.Issue
```

- **Marker**: `[#validator]` (case-insensitive) placed immediately before `func` or `export func`.
- **First Parameter**: Receives the field value being validated.
- **Return Contract**: Must return `Void | validation.Issue`. Returning `Void` signals success; returning `validation.Issue(code: "...", message: "...")` signals a violation.
- **First-Class Functions**: Validators are ordinary Conduit functions and can be called directly, passed to higher-order functions, or used in pipelines.

### Field Annotations

Annotations apply rules to struct fields:

```conduit
struct User {
    name: String @validation.min(3) @custom_rule
    email: String? @validation.email
}
```

- **Order of Execution**: When multiple annotations are applied to a field, they execute in declared order.
- **Optional Fields (`T?`)**: If a field is optional, annotations are automatically skipped when the value is `nil`. When non-nil, the inner value `T` is validated.
- **Parameterized Validators**: Accept literal arguments (e.g. `@validation.min(18)` or negative literals like `@range(-5, 10)`).

### Explicit Validation via `validation.check`

```conduit
validation.check(value: T) -> T | validation.ValidationError
```

- Normal struct constructors do not run validators automatically.
- Calling `validation.check(val)` traverses the struct, generic struct fields (such as `Page<User>`), nested structs, lists, maps (`Map<K, V>`), and tuples recursively, producing paths such as `users["alice"].email` and `items[42].title`.
- On success, it returns the original value unmodified (`T`).
- On failure, all violations across all fields are collected into an aggregated `validation.ValidationError`.

---

## Runnable Example

```conduit
import * as validation from "cdt.validation"

struct User {
    username: String @validation.min(3)
    email: String? @validation.email
    age: Int @validation.min(18)
}

func main() -> Void {
    bad := User(username: "al", email: "bad", age: 15)
    switch validation.check(bad) {
        case User(u): print("Valid: {u.username}")
        case validation.ValidationError(err):
            for v in err.issues {
                print("[{v.path}] {v.code}: {v.message}")
            }
    }
}
```

### Running the Example

```bash
conduit run validation_demo.cdt
```

Output:

```text
[username] min: Value length 2 is less than minimum 3
[email] email: Invalid email address format
[age] min: Value 15 is less than minimum 18
```

---

## API Reference

### Types

#### `validation.Issue`
- **Category:** Error Type
- **Fields:**
  - `code: String` - Machine-readable issue identifier.
  - `message: String` - Human-readable explanation.

#### `validation.Violation`
- **Category:** Struct Type
- **Fields:**
  - `path: String` - Dot-separated path to the violating field (e.g. `profile.email` or `items[0].code`).
  - `code: String` - Machine-readable error code.
  - `message: String` - Human-readable message.

#### `validation.ValidationError`
- **Category:** Error Type
- **Fields:**
  - `issues: [validation.Violation]` - List of all violations discovered during validation.

---

### Functions

#### `min(value, min_val) -> Void | validation.Issue`
- Validates that a numeric value (`Int` or `Float`), string character count (**Unicode scalar values**), list element count, or map entry count meets a minimum bound (inclusive). `Float` values must use a `Float` bound and reject non-finite values (`NaN`/`Infinity`).

#### `max(value, max_val) -> Void | validation.Issue`
- Validates that a numeric value (`Int` or `Float`), string character count (**Unicode scalar values**), list element count, or map entry count does not exceed a maximum bound (inclusive). `Float` values must use a `Float` bound and reject non-finite values (`NaN`/`Infinity`).

#### `email(value: String) -> Void | validation.Issue`
- Validates that a string conforms to basic email format (non-empty local part, single `@`, and valid domain with dot).

#### `not_empty(value) -> Void | validation.Issue`
- Validates that a `String` has non-whitespace characters (`trim().is_empty() == false`) or that a `List` / `Map` has at least one element. Usable as `@validation.not_empty`.

#### `min_len(value, n: Int) -> Void | validation.Issue` / `max_len(value, n: Int) -> Void | validation.Issue`
- Validates minimum / maximum character length (`String`) or element count (`List` / `Map`). Usable as `@validation.min_len(n)` / `@validation.max_len(n)`.

#### `range(value, min, max) -> Void | validation.Issue`
- Inclusive range check for `Int` or finite `Float`. Supports negative literals in annotations (`@validation.range(-10, 10)`).

#### `one_of(value: String, allowed: [String]) -> Void | validation.Issue`
- Validates that a `String` equals one of the listed choices. In annotations, pass variadic literals: `@validation.one_of("admin", "user", "guest")`.

#### `url(value: String) -> Void | validation.Issue`
- Validates that a string is a valid HTTP/HTTPS URL with non-empty authority host. Usable as `@validation.url`.

#### `uuid(value: String) -> Void | validation.Issue`
- Validates canonical 36-character UUID format (`8-4-4-4-12` ASCII hex). Usable as `@validation.uuid`.

#### `check(value: T) -> T | validation.ValidationError`
- Performs explicit recursive validation of a value against all declared field annotations.

