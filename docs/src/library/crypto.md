# `cdt.crypto`

Zero-dependency hashing, HMAC, Base64/Hex encoding, and OS-backed CSPRNG utilities.

```conduit
import * as crypto from "cdt.crypto"

func sign_payload(secret: String, body: String) -> String | crypto.CryptoError {
    digest := crypto.sha256(body)
    sig := crypto.hmac_sha256(secret, digest)
    token := crypto.base64_encode(sig)
    decoded := crypto.base64_decode(token)?
    return decoded
}
```

## Types

* `CryptoError { kind: String, message: String }` (`"invalid_base64"`, `"invalid_hex"`, `"invalid_utf8"`, `"invalid_input"`, `"rng_failed"`, `"hash_failed"`, `"overloaded"`)

## Password Hashing Concurrency

`hash_password` and `verify_password` share a bounded Argon2id semaphore with a bounded wait queue and timeout so concurrent password operations cannot exhaust server CPU or starve unrelated HTTP routes:

- `CONDUIT_PASSWORD_HASH_CONCURRENCY` (`1..=64`; default `clamp(cpus / 2, 2, 4)`)
- `CONDUIT_PASSWORD_HASH_MAX_QUEUE` (`0..=1024`; default `max(concurrency, 2)`)
- `CONDUIT_PASSWORD_HASH_QUEUE_TIMEOUT_MS` (`10..=60000`; default `2000`)

When both active slots and the wait queue are full (or waiting times out), `hash_password` returns `CryptoError { kind: "overloaded", ... }` and `verify_password` sheds load so HTTP servers automatically respond with `503 Service Unavailable` and `Retry-After`.

## API

| Function | Signature | Description |
| :--- | :--- | :--- |
| `sha256` | `(data: String) -> String` | Lowercase hex SHA-256 digest |
| `hmac_sha256` | `(key: String, data: String) -> String` | Lowercase hex HMAC-SHA256 tag |
| `base64_encode` | `(data: String) -> String` | Standard Base64 encoding |
| `base64_decode` | `(b64: String) -> String \| CryptoError` | Standard Base64 decoding to UTF-8 |
| `hex_encode` | `(data: String) -> String` | Lowercase hexadecimal encoding |
| `hex_decode` | `(hex: String) -> String \| CryptoError` | Hexadecimal decoding to UTF-8 |
| `uuid_v4` | `() -> String` | Random RFC 4122 UUID v4 |
| `random_bytes_hex` | `(len: Int) -> String \| CryptoError` | `len` random bytes as lowercase hex |
| `random_int` | `(min: Int, max: Int) -> Int \| CryptoError` | Random integer in `[min, max)` |
| `hash_password` | `(password: String) -> String \| CryptoError` | Argon2id (RFC 9106) salted password hash (`$argon2id$v=19$m=19456,t=2,p=1$...`) |
| `verify_password` | `(password: String, encoded: String) -> Bool` | Constant-time password verification against `$argon2id$...` and legacy `pbkdf2-sha256$...` hashes |
| `password_needs_rehash` | `(encoded: String) -> Bool` | Returns `true` if a stored password hash uses legacy PBKDF2, weak/short Argon2id parameters, or is malformed |
| `constant_time_eq` | `(a: String, b: String) -> Bool` | Constant-time string comparison |
