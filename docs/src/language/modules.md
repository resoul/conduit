# Modules & Privacy

Conduit features an explicit, file-based module system designed for encapsulation, predictable encapsulation boundaries, and maintainable project architectures.

---

## File-Based Modules & Privacy by Default

In Conduit, each `.cdt` source file constitutes an independent module.

By default, **all declarations (types, errors, and functions) are private to the file in which they are declared**. They cannot be referenced, called, or constructed by other modules unless explicitly marked for export.

---

## Explicit Exports (`export`)

To expose an item to external modules, prefix its declaration with the `export` keyword:

```conduit
// users.cdt
export type User {
    id: Int
    name: String
}

export error UserNotFound {
    id: Int
}

export func find_user(id: Int) -> User | UserNotFound {
    guard id > 0 else {
        return UserNotFound(id: id)
    }
    return User(id: id, name: "Alice")
}
```

Any item without `export` remains private:

```conduit
// Private helper function: unreachable outside users.cdt
func format_name(n: String) -> String {
    return "User: " + n
}
```

---

## Prevention of Private Type Leakage

To prevent encapsulation leaks, the type checker enforces that **exported functions and types cannot leak private types in their public signatures**:

```conduit
type SecretConfig {
    key: String
}

// Error! Exported function leaks private type SecretConfig:
export func get_config() -> SecretConfig {
    return SecretConfig(key: "secret")
}
```

Diagnostic:

```text
Type error: Exported function 'get_config' exposes private type 'SecretConfig' in return type
```

---

## Importing Modules (`import`)

Modules are imported using the `import` statement at the top of a file.

### Local Relative Imports

Local modules are imported using relative paths (without the `.cdt` extension) and an explicit alias:

```conduit
import "./users" as users
```

Items from the imported module are accessed with qualified syntax:
- `users.User`
- `users.UserNotFound`
- `users.find_user(42)`

### Standard Library Imports

Standard libraries are imported from the `cdt.*` namespace:

```conduit
import cdt.time as time
```

---

## Canonical Type Identity & Import Aliases

Every `type` and `error` in Conduit is uniquely identified by its canonical declaring module path together with its declaration name. Import aliases only affect how a symbol is spelled locally:

- Importing the same module under two aliases (`import "./users" as u1` and `import "./users" as u2`) yields the exact same canonical type, so `u1.User` and `u2.User` are interchangeable across assignments, function calls, and `switch` patterns.
- Same-named declarations in distinct modules (such as `a.User` and `b.User`) remain strictly separate, incompatible types at both compile time and runtime. Struct schemas used by `cdt.validation.check` also follow canonical identity and never collide across modules.

---

## Cycle Detection

Conduit statically analyzes the module dependency graph before type checking. If cyclic dependencies exist (e.g., `A -> B -> C -> A`), the compiler halts immediately with a clear dependency trace:

```text
Loader error: Cyclic import detected: ./a -> ./b -> ./c -> ./a
```

---

## Runnable Example

### Module Definition (`modules_user.cdt`)

```conduit
{{#include ../../../examples/docs/language/modules_user.cdt}}
```

### Module Consumer (`modules_main.cdt`)

```conduit
{{#include ../../../examples/docs/language/modules_main.cdt}}
```

### Running the Multi-Module Project

```bash
conduit run examples/docs/language/modules_main.cdt
```

Output:

```text
Hello, Alice
Successfully caught missing user error
```

