# Collections & Transforms

Conduit provides native homogeneous lists and built-in transformation utilities.

---

## Lists (`[T]`)

A list is an ordered collection of values of uniform type `T`:

```conduit
nums := [1, 2, 3, 4, 5]
```

The type of `nums` is inferred as `[Int]`. Attempting to construct a list with mismatched element types results in a compile-time type mismatch error.

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

The following program demonstrates list construction, mapping, boolean filtering, and error preservation:

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
[1, 4, 9, 16, 25]
[5, 8]
[InvalidItem(id: -1), 10, InvalidItem(id: -2)]
```

