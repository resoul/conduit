# Standard Library Overview

Conduit includes a suite of standard libraries providing high-performance primitives implemented directly on the host engine.

All standard libraries reside in the reserved `cdt.*` namespace.

---

## Design Principles

1. **Host-Native Efficiency**: Standard library operations execute directly in Rust native modules (`src/native/`), avoiding bytecode overhead for JSON, time, and validation operations.
2. **Strict Type Encapsulation**: Host resources (such as monotonic clock measurements or system timestamps) are wrapped in opaque types (`time.Instant`, `time.Timestamp`, `time.Duration`). Users cannot forge or modify raw internal state, preventing undefined behavior.
3. **Explicit Error Modeling**: Standard operations that can fail return typed errors (such as `time.TimeError`, `json.DecodeError`, `validation.ValidationError`) rather than throwing exceptions or crashing the runtime.

---

## Available Modules

| Module | Namespace | Description |
| :--- | :--- | :--- |
| [cdt.json](json.md) | `cdt.json` | Typed JSON decoding (`decode<T>`), deterministic `encode`, and dynamic `json.Value` trees. |
| [cdt.time](time.md) | `cdt.time` | System clock, monotonic timestamps, high-resolution duration calculation, and epoch conversion. |
| [cdt.validation](validation.md) | `cdt.validation` | Schema validation, field rule annotations, issue aggregation, and standard rules (`min`, `max`, `email`). |

