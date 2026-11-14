# Structuring Modules

As projects grow beyond a single file, organizing code into encapsulated modules is essential for long-term maintainability.

This guide demonstrates how to design clean public interfaces, hide implementation helpers, and verify reachable code across files.

---

## 1. Organizing Project Layout

A typical Conduit project is organized by responsibility:

```text
my_project/
  src/
    math.cdt     # Math utilities, exportable types and errors
    main.cdt     # Application entry point (func main())
```

---

## 2. Designing the Public API

In `math.cdt`, only items marked with `export` are accessible to outside callers. Internal calculation helpers (such as `internal_sum`) remain private:

```conduit
// math.cdt
export type Stats {
    count: Int
    sum: Int
}

export error EmptyData {}

// Private helper: not visible to importers
func internal_sum(items: [Int]) -> Int {
    var total: Int = 0
    for x in items {
        total = total + x
    }
    return total
}

export func compute_stats(items: [Int]) -> Stats | EmptyData {
    ...
}
```

---

## 3. Consuming Modules with Meaningful Aliases

In `main.cdt`, use TypeScript-style relative and standard library imports:

```conduit
import { compute_stats, Stats, EmptyData } from "./math"
import * as clock from "cdt.time"
```

---

## Complete Runnable Recipe

### Module Implementation (`module_recipe_math.cdt`)

```conduit
export type Stats {
    count: Int
    sum: Int
}

export error EmptyData {}

func internal_sum(items: [Int]) -> Int {
    var total: Int = 0
    for x in items {
        total = total + x
    }
    return total
}

export func compute_stats(items: [Int]) -> Stats | EmptyData {
    guard items.len() > 0 else {
        return EmptyData()
    }
    return Stats(count: items.len(), sum: internal_sum(items))
}
```

### Main Entrypoint (`module_recipe_main.cdt`)

```conduit
import { compute_stats, Stats, EmptyData } from "./module_recipe_math"
import { monotonic, elapsed } from "cdt.time"

func main() -> Void {
    start := monotonic()
    switch compute_stats([10, 20, 30, 40]) {
        case Stats(s):
            print("Item count: {s.count}")
            print("Total sum: {s.sum}")
        case EmptyData(_):
            print("Empty input")
    }
    switch elapsed(start) {
        case Duration(_): print("Completed successfully.")
        case TimeError(e): print(e.message)
    }
}
```

### Running the Recipe

```bash
conduit run module_recipe_main.cdt
```

Output:

```text
Item count: 4
Total sum: 100
Completed successfully.
```

---

## 4. Automatic Dead-Code Elimination (Tree-Shaking)

During compilation, Conduit automatically performs reachability analysis starting from `main()` across all imported modules, omitting unused functions from the final compiled bytecode module.

