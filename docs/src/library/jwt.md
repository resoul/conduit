# `cdt.jwt`

Zero-dependency JSON Web Token (JWT) signing and verification supporting `HS256` and `HS512` with typed payload decoding.

```conduit
import * as jwt from "cdt.jwt"

struct Claims {
    sub: String
    role: String
    exp: Int? = nil
}

func main() -> Void | jwt.JwtError {
    secret := "super-secret-key"
    token := jwt.sign(Claims(sub: "user_42", role: "admin"), secret, 3600)?
    verified := jwt.verify<Claims>(token, secret)?
    print("Subject: {verified.sub}, role: {verified.role}")
}
```

## Types

* `JwtError { kind: String, message: String }` — Error kinds:
  * `"unsupported_alg"` — Algorithm is not `HS256`/`HS512` (including `"none"` rejection) or mismatches the expected algorithm.
  * `"malformed"` — Token does not have 3 valid Base64URL UTF-8 segments or claims JSON is invalid.
  * `"invalid_signature"` — Constant-time HMAC verification failed.
  * `"expired"` — `exp` claim is in the past (beyond optional `leeway_secs`).
  * `"not_before"` — `nbf` claim is in the future (beyond optional `leeway_secs`).
  * `"invalid_ttl"` / `"invalid_input"` — Negative TTL or leeway argument.
  * `"encode"` / `"decode"` — JSON encoding or typed struct decoding failure.

## Functions

| Function | Signature | Description |
| :--- | :--- | :--- |
| `sign` | `(claims: T, secret: String, ttl_secs?: Int) -> String \| JwtError` | Signs `claims` (`struct` or `Map<String, ...>`) with `HS256`. When `ttl_secs` is provided, populates missing/null `iat` and `exp`. |
| `sign_alg` | `(alg: String, claims: T, secret: String, ttl_secs?: Int) -> String \| JwtError` | Signs `claims` with `"HS256"` or `"HS512"`. |
| `verify<T>` | `(token: String, secret: String, leeway_secs?: Int, require_exp?: Bool) -> T \| JwtError` | Verifies an `HS256` token (signature, `exp`, `nbf`) and decodes payload into `T`. Set `require_exp: true` to reject tokens without `exp` (`"missing_exp"`). |
| `verify_alg<T>` | `(alg: String, token: String, secret: String, leeway_secs?: Int, require_exp?: Bool) -> T \| JwtError` | Verifies using `"HS256"` or `"HS512"` explicitly and decodes into `T`. |
| `decode_unverified<T>` | `(token: String) -> T \| JwtError` | Decodes JWT payload into `T` without verifying signature or timestamps. |

## Security Notes

* Signatures are compared in constant time via HMAC-SHA256 / HMAC-SHA512 (`warn` once per process if secret `< 32` bytes).
* Tokens with `"alg": "none"` or mismatched algorithms are always rejected.
* `ttl_secs = 0` emits `iat` without forcing an `exp` claim; positive `ttl_secs` sets `exp = now + ttl_secs` when `exp` is absent or `nil`. Use `require_exp = true` in `verify` / `verify_alg` when tokens must expire.
