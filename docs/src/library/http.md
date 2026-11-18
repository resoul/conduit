# cdt.http

Zero-dependency HTTP/1.1 client, server, and segment router (`http.get`, `http.post`, `http.request`, `:param` and `*wildcard` routes, `keep-alive`, in-memory `dispatch` for testing).

## Import

```conduit
import * as http from "cdt.http"
```

## Types

- `Request { method: String, path: String, body: String, headers: Map<String, String>, query: Map<String, String>, params: Map<String, String> }`
- `Response { status: Int, body: String, headers: Map<String, String> }`
- `BinaryResponse { status: Int, body: [Int], headers: Map<String, String> }`
- `HttpError { kind: String, message: String }`
- `Router`

## Module Functions

| Function | Signature | Description |
| :--- | :--- | :--- |
| `router` | `http.router() -> Router` | Creates a new HTTP router. |
| `build` | `http.build<T>() -> Router` | Instantiates a `Router` from a declarative `router Name { ... }` schema. |
| `serve` | `http.serve<T>(addr: String) -> Void \| HttpError` | Builds and starts a blocking multi-threaded HTTP/1.1 server from a declarative `router Name { ... }` schema. |
| `text` | `http.text(body: String) -> Response` | `200 OK` with `text/plain; charset=utf-8`. |
| `html` | `http.html(body: String) -> Response` | `200 OK` with `text/html; charset=utf-8`. |
| `json` | `http.json(body: String) -> Response` | `200 OK` with `application/json`. |
| `response` | `http.response(status: Int, body: String) -> Response` | Custom status code and body. |
| `response_with` | `http.response_with(status: Int, body: String, headers: Map<String, String>) -> Response` | Custom status code, body, and headers. |
| `get` | `http.get(url: String) -> Response \| HttpError` | Sends an HTTP `GET` request (follows up to 10 redirects; `http://` via `TcpStream`, `https://` via system `curl`). |
| `post` | `http.post(url: String, body: String) -> Response \| HttpError` | Sends an HTTP `POST` request with `body`. |
| `request` | `http.request(method: String, url: String, body: String, headers: Map<String, String>) -> Response \| HttpError` | Sends a custom HTTP request with method, body, and headers. |
| `request_timeout` | `http.request_timeout(method: String, url: String, body: String, headers: Map<String, String>, timeout_ms: Int) -> Response \| HttpError` | Sends a custom HTTP request with an explicit timeout in milliseconds. |
| `get_bytes` | `http.get_bytes(url: String) -> BinaryResponse \| HttpError` | Sends an HTTP `GET` request and returns raw byte list `[Int]` (`0..=255`). |
| `request_bytes` | `http.request_bytes(method: String, url: String, body: [Int], headers: Map<String, String>) -> BinaryResponse \| HttpError` | Sends a binary HTTP request and returns `BinaryResponse`. |

## `Router` Methods

| Method | Signature | Description |
| :--- | :--- | :--- |
| `get`, `post`, `put`, `delete`, `patch`, `options`, `head`, `any` | `r.<method>(pattern: String, handler: (Request) -> Response) -> Void` | Registers a route (`/users/:id`, `/files/*rest`; `any` matches all methods). |
| `before` | `r.before(handler: (Request) -> Response?) -> Void` | Registers a pre-handler hook; returning a `Response` short-circuits routing. |
| `after` | `r.after(handler: (Request, Response) -> Response) -> Void` | Registers a post-handler hook to transform responses (e.g. CORS headers, logging). |
| `dispatch` | `r.dispatch(req: Request) -> Response` | Dispatches a `Request` in-memory (returns `404`/`405` automatically). |
| `serve` | `r.serve(addr: String) -> Void \| HttpError` | Starts a blocking HTTP/1.1 server on `addr` (e.g. `"127.0.0.1:8080"`). |
| `stop` | `r.stop() -> Void` | Signals `r.serve(...)` to stop accepting new connections and shut down gracefully after in-flight requests finish. |

## Declarative `router` DSL Example

```conduit
import * as http from "cdt.http"

func check_auth(req: http.Request) -> http.Response? {
    if req.path.starts_with("/admin") && (req.headers.get("authorization") ?? "") != "Bearer secret" {
        return http.response(401, "unauthorized")
    }
    return nil
}

func add_cors(_req: http.Request, resp: http.Response) -> http.Response {
    hdrs := resp.headers
    hdrs.set("access-control-allow-origin", "*")
    return http.response_with(resp.status, resp.body, hdrs)
}

func hello(req: http.Request) -> http.Response {
    name := req.params.get("name") ?? "world"
    return http.text("Hello, {name}!")
}

router Api "Demo API" {
    before check_auth
    after add_cors
    get "/hello/:name" => hello
}

func main() -> Void {
    switch http.serve<Api>("127.0.0.1:8080") {
        case http.HttpError(e):
            print("Server error: {e.message}")
        case _:
            print("stopped")
    }
}
```

## Imperative Example

```conduit
import * as http from "cdt.http"

func main() -> Void {
    r := http.router()
    r.get("/hello/:name", (req: http.Request) -> http.Response => {
        name := req.params.get("name") ?? "world"
        return http.text("Hello, {name}!")
    })
    switch r.serve("127.0.0.1:8080") {
        case http.HttpError(e):
            print("Server error: {e.message}")
        case _:
            print("stopped")
    }
}
```
