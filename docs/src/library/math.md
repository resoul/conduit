# cdt.math

Floating-point mathematical functions and constants (`Float`).

## Import

```conduit
import * as math from "cdt.math"
// or
import { pi, sqrt, pow, sin, cos } from "cdt.math"
```

## Constants & Functions

| Function | Signature | Description |
| :--- | :--- | :--- |
| `pi` | `math.pi() -> Float` | Returns `π` (`3.141592653589793`). |
| `e` | `math.e() -> Float` | Returns Euler's number `e`. |
| `tau` | `math.tau() -> Float` | Returns `2π`. |
| `sqrt` | `math.sqrt(x: Float) -> Float` | Square root of `x`. |
| `pow` | `math.pow(base: Float, exp: Float) -> Float` | Raises `base` to `exp`. |
| `hypot` | `math.hypot(a: Float, b: Float) -> Float` | Computes $\sqrt{a^2 + b^2}$ without intermediate overflow. |
| `sin`, `cos`, `tan` | `math.<fn>(x: Float) -> Float` | Trigonometric functions (radians). |
| `asin`, `acos`, `atan` | `math.<fn>(x: Float) -> Float` | Inverse trigonometric functions. |
| `atan2` | `math.atan2(y: Float, x: Float) -> Float` | Four-quadrant arctangent. |
| `exp`, `ln`, `log10` | `math.<fn>(x: Float) -> Float` | Exponential (`e^x`), natural log, and base-10 log. |
| `floor`, `ceil`, `round`, `trunc` | `math.<fn>(x: Float) -> Float` | Rounding functions returning `Float`. |
