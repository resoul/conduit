# Collections & Transforms

Conduit provides native homogeneous lists and built-in transformation utilities.

---

## Lists (`[T]`)

A list is an ordered collection of values of uniform type `T`:

```conduit
nums := [1, 2, 3, 4, 5]
```

The type of `nums` is inferred as `[Int]`. Attempting to construct a list with mismatched element types results in a compile-time type mismatch error.

### Safe Indexing (`list[index]`)

Indexing into `[T]` never panics at runtime and always returns `T | IndexError`:

```conduit
error IndexError {
    index: Int
    length: Int
    message: String
}
```

`IndexError` is part of the prelude and requires no import. Both `index >= list.len()` and negative indices (`index < 0`) produce an `IndexError`—Conduit does not treat negative indices as offsets from the end of the list.

You can propagate out-of-bounds errors with `?` or match explicitly with `switch`:

```conduit
func first_item(items: [String]) -> String | IndexError {
    first := items[0]?
    return first
}

func inspect(nums: [Int]) {
    switch nums[-1] {
        case Int(val):
            print(val)
        case IndexError(err):
            print(err.message)
    }
}
```

### Optional Lookup (`list.get(index)`)

For lookups where an absent element is expected and a default value makes sense, use `list.get(index: Int) -> T?`:

```conduit
nums := [10, 20, 30]
first := nums.get(0) ?? 0     // 10
missing := nums.get(99) ?? 0  // 0
```

If the element type `T` is already optional (`U?`), `.get(index)` keeps the result type as `U?` without double-wrapping. Both negative indices and `index >= list.len()` return `nil`.

Lists also support `.slice(start: Int, end: Int) -> [T]` (with clamped bounds), `.reverse() -> [T]`, `.enumerate() -> [(Int, T)]`, and `[String].join(sep: String) -> String`.

---

## Maps (`Map<K, V>`)

A `Map<K, V>` stores key-value entries with **deterministic insertion order**. Key types `K` are restricted to `String`, `Int`, and `Bool`; invalid key types (such as lists, optionals, or structs) are rejected at compile time.

### Constructing Maps

Create an empty map with `Map<K, V>()` or build one from `(key, value)` pairs with `Map.from`:

```conduit
var scores = Map<String, Int>()
scores.set("alice", 10)
scores.set("bob", 20)

by_id := Map.from([(1, "one"), (2, "two")])
empty := Map.from<String, Int>([])
```

### Indexing (`map[key]`) & `KeyError`

Indexing with `map[key]` returns `V | KeyError` (where `KeyError { key: String, message: String }` is part of the prelude):

```conduit
func find_upper(m: Map<String, String>, key: String) -> String | KeyError {
    val := m[key]?
    return val.upper()
}
```

Use `.get(key) -> V?` when missing keys are expected rather than errors, alongside `.set(key, val)`, `.remove(key) -> V?`, `.contains_key(key) -> Bool`, `.len() -> Int`, `.keys() -> [K]`, `.values() -> [V]`, and `.entries() -> [(K, V)]`.

Updating an existing key via `.set(key, new_val)` replaces the value **in place**, preserving the key's original position in `.keys()`, `.values()`, and `.entries()`. Calling `.remove(key)` followed by `.set(key, val)` appends the re-inserted key at the end.

Mutable `var` maps also support index assignment `m[key] = value`, and mutable `var` structs allow nested field writes (`order.meta.updated = true`) and collection mutations (`order.items.append(x)`). Immutable `:=` bindings forbid mutation of the variable, its fields, and nested collections.

---

## Transforming Collections

Conduit provides two core higher-order functions for lists: `map` and `filter`.

### `map(list, transform)`

The `map` function applies a transform callback `(T) -> U` to every element of `[T]`, producing a new list `[U]`:

```conduit
func square(n: Int) -> Int {
    return n * n
}

squared := map([1, 2, 3], square) // [1, 4, 9]
```

### Error Preservation in `map`

A vital guarantee in Conduit is that **`map` preserves errors as first-class elements**:

If the callback returns a union type containing an error (`T | SomeError`), `map` returns `[T | SomeError]`:

```conduit
error InvalidItem { id: Int }

func validate(n: Int) -> Int | InvalidItem {
    guard n > 0 else {
        return InvalidItem(id: n)
    }
    return n
}

results := map([-1, 10, -2], validate)
// Evaluates to: [InvalidItem(id: -1), 10, InvalidItem(id: -2)]
```

`map` does not abort processing or drop errors silently. The caller receives every element—successful values and errors alike—and can inspect or partition them using `switch`.

### `filter(list, predicate)`

The `filter` function retains only elements for which the predicate callback `(T) -> Bool` evaluates to `true`:

```conduit
func is_positive(n: Int) -> Bool {
    return n > 0
}

positives := filter([-2, 5, -1, 8], is_positive) // [5, 8]
```

> [!WARNING]
> The predicate function passed to `filter` **must** return pure `Bool`. Passing a callback that returns an error type (`Bool | Error`) is strictly rejected by the type checker. Filtering cannot silently swallow errors.

---

## Runnable Example

The following program demonstrates list construction, safe indexing, `.get()`, mapping, boolean filtering, and error preservation:

```conduit
{{#include ../../../examples/docs/language/collections.cdt}}
```

### Running the Example

```bash
conduit run examples/docs/language/collections.cdt
```

Output:

```text
[1, 2, 3, 4, 5]
First: 1
-1
[1, 4, 9, 16, 25]
[5, 8]
[InvalidItem(id: -1), 10, InvalidItem(id: -2)]
[alice, bob]
20
```

