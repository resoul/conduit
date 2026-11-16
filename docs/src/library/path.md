# `cdt.path`

Platform-aware lexical path manipulation without filesystem access.

```conduit
import * as path from "cdt.path"

func main() -> Void {
    p := path.join("/var/log", "app/../service/server.log")
    clean := path.normalize(p)
    print(path.dirname(clean))   // /var/log/service
    print(path.basename(clean))  // server.log
    print(path.stem(clean))      // server
    print(path.ext(clean))       // log
    print(path.with_ext(clean, "json")) // /var/log/service/server.json
}
```

## API

| Function | Signature | Description |
| :--- | :--- | :--- |
| `join` | `(a: String, b: String) -> String` | Joins two path segments |
| `join_all` | `(parts: [String]) -> String` | Joins a list of path segments |
| `dirname` | `(p: String) -> String` | Parent directory (or `""` if none) |
| `basename` | `(p: String) -> String` | Final file or directory name |
| `ext` | `(p: String) -> String` | Extension without leading `.` (or `""`) |
| `stem` | `(p: String) -> String` | File name without extension |
| `normalize` | `(p: String) -> String` | Lexically resolves `.` and `..` |
| `is_abs` | `(p: String) -> Bool` | `true` if `p` is an absolute path |
| `with_ext` | `(p: String, ext: String) -> String` | Replaces or appends file extension |
