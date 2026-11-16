# cdt.log

Lightweight leveled logger writing timestamped text or JSON lines to `stderr`.

## Import

```conduit
import * as log from "cdt.log"
// or
import { info, warn, error, set_level, set_json } from "cdt.log"
```

## Functions

| Function | Signature | Description |
| :--- | :--- | :--- |
| `debug` | `log.debug(msg: String) -> Void` | Logs at `debug` level. |
| `info` | `log.info(msg: String) -> Void` | Logs at `info` level (default minimum). |
| `warn` | `log.warn(msg: String) -> Void` | Logs at `warn` level. |
| `error` | `log.error(msg: String) -> Void` | Logs at `error` level. |
| `set_level` | `log.set_level(level: String) -> Void` | Sets minimum level (`"debug"`, `"info"`, `"warn"`, `"error"`). |
| `get_level` | `log.get_level() -> String` | Returns the current minimum level. |
| `set_json` | `log.set_json(enabled: Bool) -> Void` | Switches between human-readable text and JSON lines (`{"ts","level","msg"}`). |
