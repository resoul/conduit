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

In `main.cdt`, use relative imports and clear alias names:

```conduit
import "./math" as math
import cdt.time as clock
```

---

## Complete Runnable Recipe

### Module Implementation (`module_recipe_math.cdt`)

```conduit
{{#include ../../../examples/docs/guides/module_recipe_math.cdt}}
```

### Main Entrypoint (`module_recipe_main.cdt`)

```conduit
{{#include ../../../examples/docs/guides/module_recipe_main.cdt}}
```

### Running the Recipe

```bash
conduit run examples/docs/guides/module_recipe_main.cdt
```

Output:

```text
Item count:
4
Total sum:
100
Benchmark completed successfully.
```

---

## 4. Running Reachability Analysis

Conduit provides a built-in reachability analyzer to detect dead code or verify that private helper functions are actually utilized by the entry point:

```bash
conduit reachability examples/docs/guides/module_recipe_main.cdt
```

Output:

```text
=== Reachability Analysis Report ===
Root module: /workspace/examples/docs/guides/module_recipe_main.cdt
Reachable functions: 4 / 4 total (0 unused)
Reachable native functions (2):
  - cdt.time.monotonic()
  - cdt.time.elapsed()
```

If an unused private function is added to `module_recipe_math.cdt`, `conduit reachability` will flag it as unused, keeping your codebase clean.

