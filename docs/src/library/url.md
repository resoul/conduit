# `cdt.url`

URL parsing and RFC 3986 percent / query-string encoding and decoding.

```conduit
import * as url from "cdt.url"

func inspect(raw: String) -> String | url.UrlError {
    u := url.parse(raw)?
    q := url.decode_query(u.query)?
    return "{u.scheme}://{u.host}:{u.port}{u.path}"
}
```

## Types

* `Url { scheme: String, host: String, port: Int, path: String, query: String, fragment: String }` (`port` is `-1` when omitted)
* `UrlError { kind: String, message: String }` (`"invalid_url"`, `"invalid_port"`, `"invalid_encoding"`, `"invalid_utf8"`)

## API

| Function | Signature | Description |
| :--- | :--- | :--- |
| `parse` | `(raw: String) -> Url \| UrlError` | Parses a URL into structured components |
| `encode` | `(s: String) -> String` | Percent-encodes `s` (RFC 3986 unreserved preserved) |
| `decode` | `(s: String) -> String \| UrlError` | Percent-decodes `s` to UTF-8 |
| `encode_query` | `(params: Map<String, String>) -> String` | Encodes sorted `k=v&...` query string |
| `decode_query` | `(q: String) -> Map<String, String> \| UrlError` | Parses query string (`+` as space, optional `?`) |
