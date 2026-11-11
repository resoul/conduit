# cdt.json

The `cdt.json` standard library provides strict typed JSON decoding (`json.decode<T>`), deterministic encoding (`json.encode`), and dynamic JSON tree navigation (`json.parse`, `json.stringify`, `json.Value`).

```conduit
import cdt.json as json
```

---

## Decoding vs. Validation

Decoding and validation are explicitly separated into two sequential steps:

1. `json.decode<T>(raw)` parses RFC 8259 JSON syntax and verifies structure and types against `T` (missing required fields, `null` in non-optional fields, unknown extra fields, integer fractions/overflow). It does **not** trigger `@`-validators automatically.
2. `validation.check(val)` evaluates declared `@`-validation rules on the already-typed value.

```conduit
user := json.decode<User>(raw)?
checked := validation.check(user)?
```

## JSON Field Names

By default, a Conduit field maps to a JSON key with the same name. Add a Go-style `json` field tag after the field type to rename that key for both decoding and encoding:

```conduit
type Point {
    x: Int `json:"x_coord"`
    y: Int `json:"y_coord"`
}

point := json.decode<Point>("{\"x_coord\":10,\"y_coord\":20}")?
encoded := json.encode(point)?
```

The tag is separate from field validators, so both can be used on one field:

```conduit
name: String `json:"display_name"` @validation.min(2)
```

Only a JSON field-name override is supported; comma-separated tag options such as `omitempty` are not supported.

---

## Runnable Example

```conduit
{{#include ../../../examples/docs/library/json.cdt}}
```

### Running the Example

```bash
conduit run examples/docs/library/json.cdt
```

Output:

```text
{"display_name":"Alice","email":"alice@example.com","score":98.5}
{"items":[10,20]}
```

---

## API Reference

### Error & Dynamic Tree Types

- `json.DecodeError { path: String, message: String }` — Returned when JSON syntax or typed decoding fails. Path syntax follows `validation.Violation.path` (`""`, `"price"`, `"items[1].name"`, `"metadata[\"env\"]"`).
- JSON keys that are not identifiers use a quoted bracket segment in error paths, for example `["a.b"]` or `items[0]["display name"]`.
- `json.EncodeError { path: String, message: String }` — Returned when serializing a non-JSON-compatible value (non-finite `Float`, function/closure, or non-`String`-keyed `Map`).
- `json.Value` — Opaque node representing dynamic JSON (`null`, `bool`, `number`, `string`, `array`, `object`).

### Functions

- `json.decode<T>(raw: String) -> T | json.DecodeError` — Decodes JSON into schema type `T`. Ambiguous object/array/numeric unions (such as `Cat | Dog`) are rejected at compile time.
- `json.encode(value: T) -> String | json.EncodeError` — Serializes Conduit values in deterministic struct declaration order and `Map` insertion order.
- `json.parse(raw: String) -> json.Value | json.DecodeError` — Parses arbitrary JSON into an unstructured tree.
- `json.stringify(val: json.Value) -> String` — Serializes a `json.Value` tree back to JSON.

### `json.Value` Methods

- `.is_null() -> Bool`
- `.as_string() -> String?`
- `.as_int() -> Int?` (only exact JSON integers fitting signed 64-bit `Int`)
- `.as_float() -> Float?`
- `.as_bool() -> Bool?`
- `.as_list() -> [json.Value]?`
- `.as_map() -> Map<String, json.Value>?`

