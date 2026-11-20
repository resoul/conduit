# `cdt.env`

The `cdt.env` module provides typed environment variable loading, CLI argument access, and declarative `@env` struct field annotations.

---

## Importing `cdt.env`

```conduit
import * as env from "cdt.env"
// or named imports:
// import { load, get, require, args, EnvError } from "cdt.env"
```

---

## Declarative Config Loading (`config`, `@env` & `env.load<T>()`)

You can declare a `config` block (which automatically maps all fields to `@env` variables) or annotate `struct` fields with `@env` / `@env("VAR_NAME")`, and load them in one typed call with `env.load<T>() -> T | env.EnvError`:

```conduit
config AppConfig {
    port: Int @validation.min(1024) = 8080
    debug: Bool @env("APP_DEBUG") = false
    api_key: String
    region: String? @env("APP_REGION")
}
```

You can also generate a `.env.example` template directly from the CLI:

```sh
conduit env example app.cdt -o .env.example
```

- `@env` maps a field to the uppercase form of its field name (e.g., `port` -> `PORT`).
- `@env("CUSTOM_NAME")` maps the field to `"CUSTOM_NAME"`.
- Supported field types: `String`, `Int`, `Float`, `Bool`, and their optional equivalents (`String?`, `Int?`, `Float?`, `Bool?`).
- Fields with a default value (e.g., `= 8080`, `= "127.0.0.1"`) use that default if the environment variable is unset or empty.
- Optional fields (`T?`) without a default become `empty` when unset or empty.
- Required non-optional fields without a default return `env.EnvError`.
- `@env` composes naturally with `@validation.*` rules via `validation.check(cfg)`.

```conduit
import * as env from "cdt.env"
import * as validation from "cdt.validation"

struct Config {
    port: Int @env @validation.min(1024) = 8080
    debug: Bool @env("APP_DEBUG") = false
    api_key: String @env
    region: String? @env("APP_REGION")
}

func load_config() -> Config | env.EnvError | validation.ValidationError {
    cfg := env.load<Config>()?
    return validation.check(cfg)?
}

func main() -> Void {
    switch load_config() {
        case Config(cfg): print("Listening on {cfg.port}")
        case env.EnvError(e): print("Env error [{e.name}]: {e.message}")
        case validation.ValidationError(e): print("Validation error: {e.issues.len()} issue(s)")
    }
}
```

### Bool Parsing Rules

For `Bool` and `Bool?` fields, `env.load<T>()` accepts (case-insensitively):
- Truthy: `true`, `1`, `yes`, `on`
- Falsy: `false`, `0`, `no`, `off`

Any other non-empty string returns `env.EnvError`.

---

## Functions & Types

| Symbol | Signature | Description |
| :--- | :--- | :--- |
| `env.load<T>` | `() -> T \| env.EnvError` | Loads environment variables into struct `T` using `@env` annotations and defaults. |
| `env.get` | `(name: String) -> String?` | Returns the value of environment variable `name`, or `empty` if unset. |
| `env.require` | `(name: String) -> String \| env.EnvError` | Returns the value of environment variable `name`, or `EnvError` if unset or empty. |
| `env.args` | `() -> [String]` | Returns the process command-line arguments. |
| `env.EnvError` | `struct EnvError { name: String, message: String }` | Error returned when a required variable is missing or fails type parsing. |
