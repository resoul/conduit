# `cdt.jwt`

Zero-dependency JSON Web Token (JWT) signing (`HS256`, `HS512`) and verification (`HS256`, `HS512`, `RS256`, `RS384`, `RS512`, `PS256`, `ES256`, `ES384`) with typed payload decoding and optional `iss`/`aud` claim enforcement.

```conduit
import * as jwt from "cdt.jwt"

struct Claims {
    sub: String
    role: String
    exp: Int? = nil
}

func main() -> Void | jwt.JwtError {
    // Secret must be at least 32 bytes (e.g. crypto.random_bytes_hex(32) for 256+ bits)
    secret := "replace-with-crypto-random_bytes_hex-32-value"
    token := jwt.sign(Claims(sub: "user_42", role: "admin"), secret, 3600)?
    verified := jwt.verify<Claims>(token, secret)?
    print("Subject: {verified.sub}, role: {verified.role}")
}
```

## Types

* `JwtError { kind: String, message: String }` — Error kinds:
  * `"weak_secret"` — HMAC secret is shorter than 32 bytes (checked on HMAC `sign*` and `verify*`).
  * `"invalid_key"` — PEM public key / certificate cannot be parsed, RSA modulus is outside `2048..=8192` bits (or `e < 65537`), or key type mismatches the expected asymmetric algorithm.
  * `"too_large"` — Token exceeds the 16 KiB size limit.
  * `"unsupported_alg"` — Algorithm is unsupported (including `"none"` rejection) or mismatches the expected algorithm.
  * `"malformed"` — Token does not have 3 valid Base64URL UTF-8 segments, contains duplicate top-level JSON keys, or claims JSON is invalid.
  * `"invalid_signature"` — Cryptographic signature verification failed (HMAC, RSA PKCS#1 v1.5 / PSS, or ECDSA P-256 / P-384).
  * `"expired"` — `exp` claim is in the past (beyond optional `leeway_secs`).
  * `"missing_exp"` — Token has no `exp` claim when `require_exp = true`.
  * `"not_before"` — `nbf` claim is in the future (beyond optional `leeway_secs`).
  * `"invalid_issuer"` — `iss` claim is missing, non-string, or does not equal `expected_iss`.
  * `"invalid_audience"` — `aud` claim does not match `expected_aud` (either a matching string or an array of strings containing `expected_aud`).
  * `"invalid_ttl"` / `"invalid_input"` — Negative TTL or leeway argument.
  * `"encode"` / `"decode"` — JSON encoding or typed struct decoding failure.

## Functions

| Function | Signature | Description |
| :--- | :--- | :--- |
| `sign` | `(claims: T, secret: String, ttl_secs?: Int) -> String \| JwtError` | Signs `claims` (`struct` or `Map<String, ...>`) with `HS256` (secret `>= 32` bytes). When `ttl_secs` is provided, populates missing/null `iat` and `exp`. |
| `sign_alg` | `(alg: String, claims: T, secret: String, ttl_secs?: Int) -> String \| JwtError` | Signs `claims` with `"HS256"` or `"HS512"` (secret `>= 32` bytes). |
| `verify<T>` | `(token: String, secret: String, leeway_secs?: Int, require_exp?: Bool, expected_iss?: String, expected_aud?: String) -> T \| JwtError` | Verifies an `HS256` token (secret `>= 32` bytes, signature, `exp`, `nbf`, duplicate-key check, optional non-empty `expected_iss`/`expected_aud`) and decodes payload into `T`. Set `require_exp: true` to reject tokens without `exp` (`"missing_exp"`). |
| `verify_alg<T>` | `(alg: String, token: String, key: String, leeway_secs?: Int, require_exp?: Bool, expected_iss?: String, expected_aud?: String) -> T \| JwtError` | Verifies using `"HS256"` / `"HS512"` (with HMAC secret) or `"RS256"` / `"RS384"` / `"RS512"` / `"PS256"` / `"ES256"` / `"ES384"` (with PEM-encoded `PUBLIC KEY`, `RSA PUBLIC KEY`, or `CERTIFICATE`), enforces optional non-empty `expected_iss`/`expected_aud`, and decodes into `T`. |
| `decode_unverified<T>` | `(token: String) -> T \| JwtError` | Decodes JWT payload into `T` without verifying signature or timestamps. |

## Security Notes

* Symmetric (`HS256`, `HS512`) signatures are compared in constant time via HMAC-SHA256 / HMAC-SHA512 and reject secrets shorter than **32 bytes** (`"weak_secret"`). Note that this enforces minimum byte length rather than randomness/entropy (e.g., `"a" * 32` passes the length check) — always generate production keys with a CSPRNG such as `crypto.random_bytes_hex(32)`.
* Asymmetric verification accepts PEM `PUBLIC KEY` (SubjectPublicKeyInfo), `RSA PUBLIC KEY` (PKCS#1), and X.509 `CERTIFICATE` blocks: `RS256`/`RS384`/`RS512` (RSA PKCS#1 v1.5, 2048–8192 bit), `PS256` (RSASSA-PSS with SHA-256 and 32-byte salt), `ES256` (ECDSA P-256), and `ES384` (ECDSA P-384). Pass `""` for `expected_iss` to skip the issuer check when only setting `expected_aud`.
* Duplicate top-level JSON keys in the JWT header or payload are rejected (`"malformed"`), and tokens larger than 16 KiB are rejected (`"too_large"`).
* Tokens with `"alg": "none"` or mismatched algorithms are always rejected.
* `ttl_secs = 0` emits `iat` without forcing an `exp` claim; positive `ttl_secs` sets `exp = now + ttl_secs` when `exp` is absent or `nil`. Use `require_exp = true` in `verify` / `verify_alg` when tokens must expire.
