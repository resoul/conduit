# cdt.validation

The `cdt.validation` standard library provides schema validation primitives, standard validation rules (`min`, `max`, `email`), and an explicit evaluation engine via `validation.check()`.

```conduit
import cdt.validation as validation
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
type User {
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

The following program demonstrates custom validators, standard rules, optional field skipping, and violation handling:

```conduit
{{#include ../../../examples/docs/library/validation.cdt}}
```

### Running the Example

```bash
conduit run examples/docs/library/validation.cdt
```

Output:

```text
Testing validation library...
Valid user successfully registered: alice
Validation caught violations:
  [username] reserved: Username 'admin' is reserved
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
- Validates that a numeric value (`Int` or `Float`), string character count (**Unicode scalar values**, matching `.char_count()` rather than `.len()` UTF-8 bytes), list element count, or map entry count meets a minimum bound (inclusive). `Float` values must use a `Float` bound and reject non-finite values (`NaN`/`Infinity`).

#### `max(value, max_val) -> Void | validation.Issue`
- Validates that a numeric value (`Int` or `Float`), string character count (**Unicode scalar values**, matching `.char_count()` rather than `.len()` UTF-8 bytes), list element count, or map entry count does not exceed a maximum bound (inclusive). `Float` values must use a `Float` bound and reject non-finite values (`NaN`/`Infinity`).

#### `email(value: String) -> Void | validation.Issue`
- Validates that a string conforms to basic email format (non-empty local part, single `@`, and valid domain with dot).

#### `check(value: T) -> T | validation.ValidationError`
- Performs explicit recursive validation of a value against all declared field annotations.

