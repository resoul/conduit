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
- `Client` — Reusable HTTP/1.1 client with base URL, default headers, configurable timeout, and a thread-safe Keep-Alive TCP/TLS connection pool.

## Module Functions

| Function | Signature | Description |
| :--- | :--- | :--- |
| `router` | `http.router() -> Router` | Creates a new HTTP router. |
| `client` | `http.client(base_url?: String) -> Client` | Creates a pooled keep-alive `http.Client` with an optional `base_url` (default timeout `30000` ms). |
| `build` | `http.build<T>() -> Router` | Instantiates a `Router` from a declarative `router Name { ... }` schema. |
| `serve` | `http.serve<T>(addr: String) -> Void \| HttpError` | Builds and starts a blocking multi-threaded HTTP/1.1 server from a declarative `router Name { ... }` schema. |
| `text` | `http.text(body: String) -> Response` | `200 OK` with `text/plain; charset=utf-8`. |
| `html` | `http.html(body: String) -> Response` | `200 OK` with `text/html; charset=utf-8`. |
| `json` | `http.json(body: String) -> Response` | `200 OK` with `application/json`. |
| `ok_json` | `http.ok_json(val: T) -> Response` | Encodes `val` as JSON and returns `200 OK` (`application/json`). |
| `status_json` | `http.status_json(status: Int, val: T) -> Response` | Encodes `val` as JSON with custom HTTP `status`. |
| `redirect` | `http.redirect(url: String, status?: Int) -> Response` | Redirect response (`302` by default) with `location` header. |
| `no_content` | `http.no_content() -> Response` | `204 No Content` empty response. |
| `response` | `http.response(status: Int, body: String) -> Response` | Custom status code and body. |
| `response_with` | `http.response_with(status: Int, body: String, headers: Map<String, String>) -> Response` | Custom status code, body, and headers. |
| `get` | `http.get(url: String) -> Response \| HttpError` | Sends an HTTP/HTTPS `GET` request (follows up to 10 redirects; `http://` and `https://` via built-in TCP/TLS client). |
| `post` | `http.post(url: String, body: String) -> Response \| HttpError` | Sends an HTTP `POST` request with `body`. |
| `get_json` | `http.get_json<T>(url: String, query?: Map<String, String>) -> T \| HttpError` | Sends `GET` (`Accept: application/json`), appends optional URL-encoded `query`, and decodes JSON into `T`. |
| `post_json` | `http.post_json(url: String, body: T) -> Response \| HttpError` | Encodes `body` as JSON and sends `POST` with `Content-Type: application/json`. |
| `request` | `http.request(method: String, url: String, body: String, headers: Map<String, String>) -> Response \| HttpError` | Sends a custom HTTP request with method, body, and headers. |
| `request_timeout` | `http.request_timeout(method: String, url: String, body: String, headers: Map<String, String>, timeout_ms: Int) -> Response \| HttpError` | Sends a custom HTTP request with an explicit timeout in milliseconds. |
| `get_bytes` | `http.get_bytes(url: String) -> BinaryResponse \| HttpError` | Sends an HTTP `GET` request and returns raw byte list `[Int]` (`0..=255`). |
| `request_bytes` | `http.request_bytes(method: String, url: String, body: [Int], headers: Map<String, String>) -> BinaryResponse \| HttpError` | Sends a binary HTTP request and returns `BinaryResponse`. |

## `Client` Methods (Keep-Alive Pool)

`http.Client` reuses idle HTTP/1.1 TCP and TLS connections per `(host, port, tls)` across requests (up to 8 idle connections per host, 60s idle TTL) and transparently retries once on a fresh socket if a pooled connection was closed by the server.

| Method | Signature | Description |
| :--- | :--- | :--- |
| `header` | `c.header(name: String, value: String) -> Client` | Returns a client copy (sharing the same pool) with default header `name: value`. |
| `bearer` | `c.bearer(token: String) -> Client` | Sets `Authorization: Bearer <token>` on the client. |
| `timeout_ms` | `c.timeout_ms(ms: Int) -> Client` | Sets request timeout in milliseconds. |
| `base_url` | `c.base_url(url: String) -> Client` | Updates the client's `base_url`. |
| `get` | `c.get(path: String) -> Response \| HttpError` | Pooled `GET` against `base_url + path` (or full URL). |
| `delete` | `c.delete(path: String) -> Response \| HttpError` | Pooled `DELETE` request. |
| `post` | `c.post(path: String, body: String) -> Response \| HttpError` | Pooled `POST` request with string `body`. |
| `put` | `c.put(path: String, body: String) -> Response \| HttpError` | Pooled `PUT` request with string `body`. |
| `patch` | `c.patch(path: String, body: String) -> Response \| HttpError` | Pooled `PATCH` request with string `body`. |
| `get_json` | `c.get_json<T>(path: String, query?: Map<String, String>) -> T \| HttpError` | Pooled `GET` (`Accept: application/json`) decoding JSON into `T`. |
| `post_json` | `c.post_json(path: String, body: T) -> Response \| HttpError` | Pooled `POST` encoding `body` as JSON (`Content-Type: application/json`). |
| `request` | `c.request(method: String, path: String, body?: String, headers?: Map<String, String>) -> Response \| HttpError` | Pooled custom request with optional body and per-request headers. |
| `pool_stats` | `c.pool_stats() -> Map<String, Int>` | Returns pool metrics (`{"idle": <count>}`). |
| `close` | `c.close() -> Void` | Closes and drains all idle connections in the client's pool. |

```conduit
api := http.client("https://api.example.com")
    .bearer(token)
    .timeout_ms(5000)

user := api.get_json<User>("/v1/me")?
api.close()
```

## `Router` Methods

| Method | Signature | Description |
| :--- | :--- | :--- |
| `get`, `post`, `put`, `delete`, `patch`, `options`, `head`, `any` | `r.<method>(pattern: String, handler: (Request) -> Response) -> Void` | Registers a route (`/users/:id`, `/files/*rest`; `any` matches all methods). |
| `before` | `r.before(handler: (Request) -> Response?) -> Void` | Registers a pre-handler hook; returning a `Response` short-circuits routing. |
| `after` | `r.after(handler: (Request, Response) -> Response) -> Void` | Registers a post-handler hook to transform responses (e.g. CORS headers, logging). |
| `dispatch` | `r.dispatch(req: Request) -> Response` | Dispatches a `Request` in-memory (returns `404`/`405` automatically). |
| `serve` | `r.serve(addr: String) -> Void \| HttpError` | Starts a blocking HTTP/1.1 server on `addr` (e.g. `"127.0.0.1:8080"`). |
| `stop` | `r.stop() -> Void` | Signals `r.serve(...)` to stop accepting new connections and shut down gracefully after in-flight requests finish. |

## `Request` Helper Methods

| Method | Signature | Description |
| :--- | :--- | :--- |
| `header` | `req.header(name: String) -> String?` | Case-insensitive header lookup. |
| `param` | `req.param(name: String) -> String?` | Route `:param` / `*wildcard` lookup. |
| `query_param` | `req.query_param(name: String) -> String?` | Query-string parameter lookup. |

## Declarative `router` DSL Features

Inside `router Name { ... }`:
- `group "/api/v1" { before auth; get "/users" => list_users }` — Route prefix grouping with local `before`/`after` middleware scoped to that prefix.
- `guard auth_user { get "/me" => me }` — Typed route guard block: `auth_user(req: http.Request) -> Ctx | http.Response` short-circuits on `http.Response` or passes `Ctx` as the last typed parameter to handlers inside the block.
- `@http.query` struct binding — Annotate a struct with `@http.query` to automatically parse and validate `:param` path parameters and `?key=val` query parameters (with defaults and `@validation.*` -> `400 invalid_query` / `422 validation_failed`). Handlers can accept `(http.Request, QueryStruct)`, `(http.Request, QueryStruct, BodyStruct)`, and/or `GuardCtx`.
- `@http.status(code)` & typed return/error unions — Handlers can return any JSON-serializable type `T`, `Void` (`204`), `T?` (`200`/`404`), `http.Response`, or a union `T | AppError` directly; annotate `error` or `struct` types with `@http.status(404)` (unannotated `error` defaults to `500`, `struct` defaults to `200`). Both `@http.query` parameters and typed return/error responses are reflected in `conduit openapi`.
- `static "/assets" => "./public"` — Serves static files with `Content-Type` detection and path-traversal protection.
- `cors "*"` — Automatically handles `OPTIONS` preflight (`204`) and injects `access-control-allow-origin` on responses.

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

## Typed JSON & Validation Contracts

A route handler in a declarative `router` block may take a second parameter `(req: http.Request, body: MyStruct) -> http.Response`:

```conduit
import * as http from "cdt.http"
import * as validation from "cdt.validation"

struct CreateUser {
    email: String @validation.email
    name: String @validation.min(2)
}

func create_user(req: http.Request, body: CreateUser) -> http.Response {
    return http.response(201, "Created {body.name}")
}

router Api {
    post "/users" => create_user
}
```

When a request matches `/users`, the router automatically:
1. Decodes `req.body` as `CreateUser` (returns `400 Bad Request` JSON `{"error":"invalid_json","message":...}` on parse/type error).
2. Runs all `@validation.*` and custom `[#validator]` rules on `CreateUser` (returns `422 Unprocessable Entity` JSON `{"error":"validation_failed","issues":[...]}` on failure).
3. Invokes `create_user(req, body)` with the validated struct.

The typed body parameter can also be a tagged-union data `enum` (decoded via its `"type"` discriminator field).

## Route Inspection & OpenAPI 3.0.3 (`conduit routes` & `conduit openapi`)

Declarative `router` blocks can be inspected or exported to an OpenAPI 3.0.3 specification directly from the CLI without starting the server:

```bash
# Print the route table (METHOD, PATH, HANDLER, BODY, middleware)
conduit routes main.cdt

# Generate OpenAPI 3.0.3 JSON specification
conduit openapi main.cdt --title "Users API" --version "1.0.0" --server http://localhost:8080 -o openapi.json
```

`conduit openapi` automatically extracts path parameters (`:id`, `*rest`), request body schemas (including `@validation.*` constraints and data `enum` `oneOf` + `discriminator` schemas), and inferred response status codes (`http.ok_json`, `http.status_json`, `http.no_content`, `http.redirect`, plus `400`/`422` error contracts).

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
