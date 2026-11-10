# Conduit

A domain-specific programming language for web services, real-time communication, and data streams. Source files use the `.cdt` extension.

## Overview

Conduit treats networking, routing, data schemas, and event channels as language-level primitives rather than external libraries.

- **Zero External Dependencies**: Implemented purely using the Rust standard library (`std`).
- **Lightweight & Self-Contained**: Binary size is under 1 MB, no external runtime or VM needed.
- **Strongly Typed**: Static type-checker catches type errors, unhandled errors, and non-exhaustive patterns before code runs.
- **Explicit Modules & Privacy**: All declarations are private by default; `export` explicitly exposes items across modules.
- **Typed Errors as Values**: Errors are regular typed values returned via `return`, handled with `switch`, and propagated with `?`. Zero exceptions.
- **Field Annotations & Custom Validators**: Mark functions with `[#validator]` and annotate struct fields with `@rule` or `@module.rule(...)`, validated explicitly with `validation.check()`.
- **Standard Library (`cdt.*`)**: First-class standard libraries built on a unified native registry (`cdt.time` for system and monotonic time, `cdt.validation` for schema rules).

## Installation

Rust and Cargo are **not required** to use Conduit.

### macOS & Linux
```bash
curl -fsSL https://raw.githubusercontent.com/resoul/conduit/main/install.sh | sh
```

### Windows (PowerShell)
```powershell
irm https://raw.githubusercontent.com/resoul/conduit/main/install.ps1 | iex
```

## Usage

```bash
# Type-check a Conduit project or file
conduit check examples/modules/main.cdt

# Run a Conduit program
conduit run examples/modules/main.cdt

# Run dead code & native dependency reachability analysis
conduit reachability examples/modules/main.cdt

# Start the Language Server for editors (RustRover, VS Code, etc.)
conduit lsp
```

## Quick Language Tour

### Modules & Explicit Exports

```conduit
// users.cdt
export type User {
    id: Int
    name: String
}

export error UserNotFound {
    id: Int
}

export func find(id: Int) -> User | UserNotFound {
    guard id > 0 else {
        return UserNotFound(id: id)
    }
    return User(id: id, name: "Alice")
}
```

### Imports, Error Propagation (`?`), and Pattern Matching (`switch`)

```conduit
// main.cdt
import "./users" as users
import cdt.time

func get_user_status(id: Int) -> String | users.UserNotFound {
    // Propagate error with '?' operator
    user := users.find(id)?
    return user.name
}

func main() {
    start := time.monotonic()

    switch get_user_status(42) {
        case String(name):
            print("Found user: " + name)
        case users.UserNotFound(err):
            print("Missing user ID:")
            print(err.id)
    }

    elapsed := time.elapsed(start)
    switch elapsed {
        case time.Duration(d):
            print(d.milliseconds()?)
        case time.TimeError(e):
            print(e.message)
    }
}
```

### Custom Validators & Field Annotations

```conduit
import cdt.validation as validation

[#validator]
export func not_admin(name: String) -> Void | validation.Issue {
    if name == "admin" {
        return validation.Issue(code: "reserved", message: "Username 'admin' is reserved")
    }
    return
}

type NewAccount {
    username: String @validation.min(3) @not_admin
    email: String @validation.email
    age: Int @validation.min(18)
}

func register(acc: NewAccount) -> NewAccount | validation.ValidationError {
    // Explicit validation returns the verified struct or collected violations
    return validation.check(acc)?
}
```

## Examples

Runnable example files are located in `examples/`:
- `examples/docs/`: Verified documentation examples matching the official guide.
- `examples/modules/`: Multi-module projects, explicit exports, and `cdt.time` operations.
- `examples/functions/`: Core language features, pipelines, guards, unions, and tuples.

## Documentation

The official Conduit documentation is built with mdBook and hosted on GitHub Pages:
- **Online Book:** [https://resoul.github.io/conduit/](https://resoul.github.io/conduit/)
- **Documentation Sources:** [docs/src/introduction.md](docs/src/introduction.md)
- **Build Locally:** `mdbook build docs`
- **Local Live Preview:** `mdbook serve docs` (opens at `http://localhost:3000`)

## Editor Support

Conduit provides native editor support via LSP and TextMate bundles for JetBrains (RustRover), VS Code, and others. See [editors/README.md](editors/README.md) for quick 1-minute setup instructions.
