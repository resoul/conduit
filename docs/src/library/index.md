# Standard Library Overview

Conduit includes a suite of standard libraries providing high-performance primitives implemented directly on the host engine.

All standard libraries reside in the reserved `cdt.*` namespace.

---

## Design Principles

1. **Host-Native Efficiency**: Standard library operations are executed directly by native Rust implementations via a unified host registry (`NativeRegistry`), avoiding interpretive overhead for time, system, and I/O operations.
2. **Strict Type Encapsulation**: Host resources (such as monotonic clock measurements or system timestamps) are wrapped in opaque types (`time.Instant`, `time.Timestamp`, `time.Duration`). Users cannot forge or modify raw internal state, preventing undefined behavior.
3. **Explicit Error Modeling**: Standard operations that can fail return typed errors (such as `time.TimeError`) rather than throwing exceptions or crashing the runtime.
4. **Deterministic Documentation**: The API specifications for all standard libraries are generated deterministically directly from the `NativeRegistry`.

---

## Available Modules

| Module | Namespace | Description |
| :--- | :--- | :--- |
| [cdt.time](time.md) | `cdt.time` | System clock, monotonic timestamps, high-resolution duration calculation, and epoch conversion. |
| [cdt.validation](validation.md) | `cdt.validation` | Schema validation, field rule annotations, issue aggregation, and standard rules (`min`, `max`, `email`). |

---

## Inspecting Documentation via CLI

You can inspect the generated API reference for any registered standard library directly using the `conduit doc` command:

```bash
conduit doc cdt.time
```

