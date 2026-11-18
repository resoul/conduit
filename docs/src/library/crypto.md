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

* `CryptoError { kind: String, message: String }` (`"invalid_base64"`, `"invalid_hex"`, `"invalid_utf8"`, `"invalid_input"`, `"rng_failed"`)

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
| `hash_password` | `(password: String) -> String \| CryptoError` | PBKDF2-HMAC-SHA256 salted password hash (`$pbkdf2-sha256$...`) |
| `verify_password` | `(password: String, encoded: String) -> Bool` | Constant-time password verification against `$pbkdf2-sha256$...` |
| `constant_time_eq` | `(a: String, b: String) -> Bool` | Constant-time string comparison |
