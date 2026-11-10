# Tuples

Tuples are fixed-size, heterogeneous collections of values grouped into a single unit.

---

## Tuple Types & Construction

A tuple is created by enclosing comma-separated expressions in parentheses:

```conduit
coords := (10, 20)
```

The type of `coords` is `(Int, Int)`. Functions can return multiple values cleanly by returning a tuple:

```conduit
func get_dimensions() -> (Int, Int, String) {
    return (1920, 1080, "16:9")
}
```

---

## Destructuring (`:=`)

Tuples can be unpacked into individual variables using destructuring assignment:

```conduit
dims := get_dimensions()
(width, height, aspect) := dims

print(width)  // 1920
print(height) // 1080
print(aspect) // 16:9
```

### Wildcards (`_`)

If you only need a subset of the returned values, use the wildcard underscore `_` to ignore specific elements:

```conduit
(w, _, _) := dims
print(w) // 1920
```

> [!WARNING]
> **Safety Invariant:** A wildcard `_` in a tuple destructuring pattern **cannot** be used to silently discard an element whose type contains an error variant. Error values must be handled or propagated explicitly.

---

## Matching Tuples in `switch`

You can pattern match tuples within `switch` statements:

```conduit
pair := (100, 200)
switch pair {
    case (first, second):
        print("Matched pair:")
        print(first)
        print(second)
}
```

---

## Runnable Example

The following program demonstrates tuple creation, multi-value returns, destructuring with wildcards, and pattern matching:

```conduit
{{#include ../../../examples/docs/language/tuples.cdt}}
```

### Running the Example

```bash
conduit run examples/docs/language/tuples.cdt
```

Output:

```text
10
20
1920
1080
16:9
1920
Matched pair:
100
200
```

