# Changelog

All notable changes to the **Conduit** language are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.3.2] - 2026-11-19

### Added

* **`cdt.db` Table & Model Schemas (`@db.*`)**:
  * Added `@db.table("name")` struct annotation and field annotations (`@db.pk`, `@db.auto`, `@db.unique`, `@db.index`, `@db.col`, `@db.default`, `@db.fk`, supporting both leading and trailing field placement in `src/frontend/parser.rs`) with `DbModelMeta` bundle serialization (`src/ir/bundle.rs`).
  * Added `db.ddl<T>(dialect)`, `Connection` model methods (`.create_table<T>()`, `.drop_table<T>()`, `.insert<T>(row)`, `.all<T>()`, `.find<T>(id)`, `.delete<T>(id)`), and corresponding `db.*` helpers for `migration` blocks (`db.create_table<T>()`, `db.drop_table<T>()`, `db.insert<T>()`, `db.all<T>()`, `db.find<T>()`, `db.delete<T>()`) (`src/native/db.rs`, `src/vm/exec_db.rs`, `cdt_db_model_schemas_ddl_and_bundle_e2e`), and updated `examples/db_service/main.cdt`.
* **`cdt.cron` Declarative `schedule` & Scheduler**:
  * Added top-level `schedule Name { every "*/5 * * * *" => fn; interval 10s => fn }` syntax (`src/frontend/parser_cli.rs`, `src/frontend/check_sys.rs`, `src/frontend/reachability.rs`), compile-time 5-field UTC cron validation, isolated per-job VM execution with overlap protection (`src/native/cron.rs`, `src/vm/exec_cron.rs`, `EXT_CRON_OP`), `cron.run<T>()`, `cron.start<T>()`, `cron.run_once<T>()`, `cron.validate()`, `Scheduler` (`.stop()`, `.is_running()`), `conduit cron <list|print|run>` CLI commands, `docs/src/library/cron.md`, LSP docs, and TextMate highlighting (`cdt_cron_schedule_interval_and_bundle_e2e`).
* **Typed HTTP Router Contracts (`router` + `struct` + `@validation.*`) & HTTP Ergonomics**:
  * Route handlers in `router Name { ... }` now accept `(http.Request, BodyStruct) -> http.Response` in addition to `(http.Request) -> http.Response` (`src/frontend/check_sys.rs`, `src/frontend/reachability.rs`, `src/ir/lower_native.rs`, `src/vm/exec_http.rs`). The router automatically decodes JSON (`400 Bad Request` on invalid JSON) and runs all `@validation.*` / `[#validator]` rules (`422 Unprocessable Entity` with `issues` JSON) before invoking the handler (`cdt_http_typed_contract_validation_e2e`).
  * Added `group "/prefix" { before ...; after ...; ... }` scoped middleware, `static "/prefix" => "./dir"` static file serving (with path-traversal protection and `Content-Type` detection), and `cors "<origin>"` (preflight `OPTIONS` + `access-control-allow-origin`) inside `router` blocks (`cdt_http_groups_static_cors_helpers_e2e`).
  * Added `http.ok_json(val)`, `http.status_json(status, val)`, `http.redirect(url, status?)`, `http.no_content()`, `req.header(name)` (case-insensitive), `req.param(name)`, `req.query_param(name)`, `http.get_json<T>(url, query?)`, and `http.post_json(url, body)`.
* **Editor Support & JetBrains PhpStorm Dark Theme**:
  * Upgraded `editors/textmate/conduit.tmLanguage.json` to a modular repository-based TextMate grammar (fixing `func` declarations, separating DSL/HTTP/annotation/property/constant scopes, restricting string interpolation highlighting to `{ident.field}`, and supporting backtick field tags) and added `editors/textmate/language-configuration.json`.
  * Added the bundled **Conduit PhpStorm Dark** VS Code / Antigravity color theme (`editors/textmate/themes/conduit-phpstorm-dark.json`) and standalone `.cdt`-only preset (`editors/textmate/phpstorm-token-colors.json`).
* **CLI Integration Test Suite (`tests/cli_e2e.rs`)**:
  * Added end-to-end binary tests covering `conduit check` (`[line:col]` error coordinates & SQL identifier validation), `conduit run` (`cdt.db` + `cdt.crypto`), `conduit cron list|print|run`, and `conduit build` standalone executables.

### Changed

* **Unified Native Module Registry & Compile-Time Safety**:
  * Replaced ~35 per-module `*_aliases`/`*_direct_fns`/`*_direct_types` maps in `CheckedProgram` and duplicate `is_native_module` lists with a single `NATIVE_MODULES` table (`src/frontend/check_native.rs`, `src/frontend/check.rs`, `src/frontend/check_call.rs`, `src/ir/lower_call.rs`).
  * Attached `[line:col]` spans to typechecker statement/expression errors (`src/frontend/check.rs`), enabling exact diagnostic positions in both CLI and `conduit lsp`.
  * Added strict `[a-zA-Z_][a-zA-Z0-9_]*` SQL identifier validation for `@db.table`, `@db.col`, `@db.fk`, and unsafe-token checks for `@db.default`, and removed non-test `.unwrap()` calls in `src/ir/bundle.rs`, `src/native/pg.rs`, and `src/native/mysql.rs`.
* **VM Memory Layout (`Box<VmHostState>`)**:
  * Boxed cold host-resource tables (`fs_readers`, `fs_writers`, `routers`, `db_conns`, `cron_schedulers`) inside `Box<VmHostState>` (`src/vm/mod.rs`) to keep the `Vm` struct compact in cache.
* **Documentation & Release Metadata**:
  * Updated `README.md`, `docs/src/getting-started/{installation,editors}.md`, `docs/src/library/{db,http,cron}.md`, `docs/theme/conduit-highlight.js`, `editors/README.md`, and bumped crate/editor versions to `0.3.2`.

## [0.3.1] - 2026-11-18

### Added

* **`cdt.db` Zero-Dependency PostgreSQL & MySQL Drivers**:
  * Added native PostgreSQL v3.0 wire-protocol driver (`src/native/pg.rs` — `SCRAM-SHA-256`, MD5, cleartext, `?` to `$1..$N` placeholder translation, `Parse/Bind/Describe/Execute/Sync`) and native MySQL v10 wire-protocol driver (`src/native/mysql.rs` — `mysql_native_password`, `caching_sha2_password` fast auth, `COM_QUERY`, prepared statements `COM_STMT_PREPARE`/`EXECUTE`/`CLOSE`).
  * Exposed unified `cdt.db` API (`db.connect`, `null`, `int`, `float`, `bool`, `text`, `Connection` methods `.exec`, `.query<T>`, `.query_rows`, `.begin`, `.commit`, `.rollback`, `.migrate`, `.dialect`, `.last_insert_id`, `.close`, and `DbError { kind, message, code }`) in `src/native/db.rs`, `src/vm/exec_db.rs`, `EXT_DB_OP`, and `docs/src/library/db.md`.
* **Declarative `migration` Syntax & `conduit db` CLI**:
  * Added top-level `migration <version> "<name>" { up { ... } down { ... } }` (and shorthand `up "..."` / `down "..."`) declarations across parser, linker, reachability, typechecker, IR lowering, and binary bundle serialization (`src/ir/bundle.rs`), tracking applied versions in `schema_migrations`.
  * Added `conduit db <migrate|rollback|status> <file.cdt> --url <dsn> [--steps <n>] [--dry-run]` CLI commands (`src/main.rs`, `docs/src/getting-started/cli.md`).
* **Password Hashing in `cdt.crypto`**:
  * Added `crypto.hash_password(password)` (PBKDF2-HMAC-SHA256 with random 16-byte salt, `$pbkdf2-sha256$600000$...`), `crypto.verify_password(password, encoded)`, and `crypto.constant_time_eq(a, b)` (`src/native/crypto.rs`, `src/vm/exec_sys.rs`, `docs/src/library/crypto.md`).
* **Examples Reorganization**:
  * Renamed `examples/http_client` to `examples/cli_http` and added `examples/db_service` with `docker-compose.yml` for PostgreSQL 16 and MariaDB 11.8 (`cdt_db_migrations_bundle_and_crypto_passwords_e2e`).

### Fixed

* **Hoisted Loop Constants Before Loop Entry**:
  * Fixed `FnLowerer` (`src/ir/lower.rs`) by calling `flush_hoisted_constants_if_outer()` at function entry so constants hoisted from a loop are initialized before any code outside the loop reads them.
* **Native Module Resolution in File Loader**:
  * Registered `"cdt.db"` in `is_native_module` (`src/frontend/mod.rs`, `src/frontend/linker.rs`) and `is_error_type` (`src/frontend/check_native.rs`) for `conduit run`/`check` and `?` error propagation.

### Changed

* **Documentation & Editor Support**:
  * Updated `README.md`, `docs/src/SUMMARY.md`, `docs/src/library/index.md`, `docs/src/library/{db,crypto}.md`, `docs/src/getting-started/{cli,installation}.md`, `src/lsp_docs.rs`, `editors/textmate/conduit.tmLanguage.json`, and bumped crate/editor versions to `0.3.1`.

## [0.3.0] - 2026-11-18

### Added

* **Declarative `router Name { ... }` DSL (`http.build<T>()`, `http.serve<T>()`)**:
  * Added top-level `router Name { before fn; after fn; get "/path/:id" => fn; ... }` syntax (`src/frontend/parser.rs`, `src/frontend/parser_cli.rs`, `src/frontend/check_sys.rs`, `src/frontend/reachability.rs`, `src/ir/lower_native.rs`) that validates handler signatures at compile time and builds or serves an `http.Router` via `http.build<T>()` and `http.serve<T>(addr)`.
* **`cdt.cli` Command Handler Binding (`=> handler`) & `cli.dispatch<T>()`**:
  * Added `=> handler` syntax on `cmd` declarations in `cli` blocks (`src/frontend/parser_cli.rs`) and `cli.dispatch<T>()` / `cli.dispatch_args<T>(args)` (`src/frontend/check_cli.rs`, `src/frontend/reachability.rs`, `src/ir/lower_native.rs`) supporting `(App, Cmd) -> R` and `(Cmd) -> R` handlers (`cdt_cli_dispatch_e2e`, `examples/http_client`).
* **`cdt.http` HTTP/1.1 Client, Server & Router**:
  * Added zero-dependency `cdt.http` server/router (`http.router()`, `http.build<T>()`, `http.serve<T>()`, `http.text()`, `http.html()`, `http.json()`, `http.response()`, `http.response_with()`, `Request`, `Response`, `BinaryResponse`, `HttpError`, `Router` methods `.get`, `.post`, `.put`, `.delete`, `.patch`, `.options`, `.head`, `.any`, `.before`, `.after`, `.dispatch`, concurrent worker-pool `.serve` with 30s socket timeouts and `HEAD` handling, and graceful `.stop()`) with `:param` / `*wildcard` segment routing, `keep-alive`, `Content-Length`, and automatic `400`/`404`/`405`/`413`/`500` handling (`src/native/http.rs`, `src/vm/exec_http.rs`, `EXT_HTTP_OP`, `docs/src/library/http.md`, `cdt_http_router_e2e`, `cdt_http_server_graceful_shutdown_e2e`).
  * Added zero-dependency outbound HTTP client (`http.get()`, `http.post()`, `http.request()`, `http.request_timeout()`, `http.get_bytes()`, `http.request_bytes()`) with automatic redirect following (up to 10 hops), configurable timeout, binary `[Int]` bodies, `http://` over `std::net::TcpStream` (`Content-Length`, `Transfer-Encoding: chunked`), and `https://` via system `curl` fallback (`cdt_http_client_loopback_e2e`).
* **Unified `examples/http_client` Showcase**:
  * Renamed `example/` to `examples/` and consolidated the full CLI, HTTP client/server, config/env, JSON pipeline, streaming FS, and Docker `FROM scratch` example inside `examples/http_client/`.

### Changed

* **HTTP Hot-Path & Documentation Updates**:
  * Pre-cached static `content-type` response header `OrderedMap`s via `LazyLock` (`HEADERS_EMPTY`, `HEADERS_TEXT`, `HEADERS_HTML`, `HEADERS_JSON`), added O(1) non-wildcard route length rejection + single-split path matching in `lookup_route`, eliminated per-request hook `Vec` cloning and intermediate response `HashMap` allocations (`src/native/http.rs`, `src/vm/exec_http.rs`).
  * Updated LSP hover/completions (`src/lsp_docs.rs`), TextMate grammar (`editors/textmate/`), `README.md`, `docs/src/library/index.md`, `docs/src/library/cli.md`, `docs/src/library/http.md`, and bumped crate/editor versions to `0.3.0`.

## [0.2.7] - 2026-11-18

### Added

* **Enums (`enum` / `export enum`)**:
  * Added scalar-backed nominal `enum` declarations (`enum Role { Admin, User, Guest }` and `export enum ...`) with `Role.Admin` expressions, `==`/`!=`, `switch` variant matching (`case Role.Admin:`), cross-module linking, TextMate highlighting, and `enum_and_const_interp_cross_module` tests.
* **LSP Rename, Formatting & `const`/`enum` Hover/Completion**:
  * Added `"renameProvider":true` (`textDocument/rename` via `handle_rename`), `"documentFormattingProvider":true` (`textDocument/formatting`), `const` and `enum` hover/completion (`Enum.` variants), and `enum` support in `textDocument/definition` and `textDocument/documentSymbol` (`src/lsp.rs`), plus updated `docs/src/getting-started/editors.md`.
* **`conduit fmt` Formatter**:
  * Added `conduit fmt <file_or_dir> [--check]` (`src/fmt.rs`, `src/main.rs`) for 4-space indentation and `switch`/`case` formatting while preserving comments and strings, plus docs in `docs/src/getting-started/cli.md`.
* **`List` Aggregate Methods & Cross `Obj <-> Scalar` Pipeline `map`**:
  * Added native `.sum() -> Int | Float`, `.min() -> T?`, and `.max() -> T?` on `[Int]` and `[Float]` (`EXT_LIST_AGG`, `list_sum_min_max_methods`), plus docs in `docs/src/language/collections.md` and `enum` docs in `docs/src/language/types.md`.
  * Added `PipelineStage::MapObjToScalar` and `PipelineStage::MapScalarToObj` (`run_mixed_pipeline` in `src/vm/pipeline.rs`, `src/ir/lower_call.rs`, `src/ir/bundle.rs`), enabling pipelines like `[Event] |> filter(...) |> map(e => e.score)` and `[Int] |> map(x => "n={x}")` (`cross_scalar_obj_pipeline_map`).

### Fixed

* **Linker String Interpolation Rewriting**:
  * Added `rewrite_interp_string` in `src/frontend/linker.rs` so colliding `const` / namespace-imported identifiers inside `{...}` string interpolation are safely rewritten across modules.

## [0.2.6] - 2026-11-17

### Added

* **`cdt.url` Standard Library Module**:
  * Added `cdt.url` (`parse`, `encode`, `decode`, `encode_query`, `decode_query`) with `Url { scheme, host, port, path, query, fragment }` and `UrlError { kind, message }` (`src/native/url.rs`, `src/vm/exec_sys.rs`, `EXT_URL_OP`), plus LSP hover/completion docs (`src/lsp_docs.rs`), TextMate highlighting (`editors/textmate/conduit.tmLanguage.json`), library docs (`docs/src/library/url.md`), and end-to-end tests (`cdt_url_e2e`).
* **`cdt.csv` Standard Library Module**:
  * Added RFC 4180 `cdt.csv` (`parse`, `parse_with`, `parse_records`, `stringify`, `stringify_with`) with `CsvError { kind, line, message }` (`src/native/csv.rs`, `src/vm/exec_sys.rs`, `EXT_CSV_OP`), LSP hover/completion docs, TextMate highlighting, library docs (`docs/src/library/csv.md`), and end-to-end tests (`cdt_csv_e2e`).
* **`cdt.math` & `cdt.log` Standard Library Modules**:
  * Added `cdt.math` (`pi`, `e`, `tau`, `sqrt`, `pow`, `hypot`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `exp`, `ln`, `log10`, `floor`, `ceil`, `round`, `trunc`) (`src/native/math.rs`, `EXT_MATH_OP`) and `cdt.log` (`debug`, `info`, `warn`, `error`, `set_level`, `get_level`, `set_json`) (`src/native/log.rs`, `EXT_LOG_OP`), with LSP docs, library docs (`docs/src/library/{math,log}.md`), and tests (`cdt_math_and_log_e2e`).
* **Module-Level Constants (`const` / `export const`)**:
  * Added compile-time evaluated module constants (`const NAME [: Type] = expr` and `export const ...`) for `Int`, `Float`, `Bool`, and `String` (arithmetic/boolean constant folding, cross-module `import`, default parameter/field values, string interpolation `{CONST}`, TextMate highlighting, and docs in `docs/src/language/variables.md`).
* **LSP Go-to-Definition (`textDocument/definition`) & Document Symbols (`textDocument/documentSymbol`)**:
  * Added `"definitionProvider":true` (`handle_definition`) and `"documentSymbolProvider":true` (`handle_document_symbol`) in `src/lsp.rs` supporting jumps to `func`/`struct`/`type`/`const` declarations, imported symbols, relative import paths, and IDE Outline symbols, plus updated `docs/src/getting-started/editors.md`.
* **Starter Example & Benchmarks**:
  * Updated `example/` (`config.cdt`, `main.cdt`, `README.md`) to showcase `const` / `export const`, `cdt.log`, and `cdt.url` (`inspect` subcommand).
  * Added `pipeline_float_1m`, `csv_parse_20k`, and `url_parse_50k` to `scripts/bench/` (`run_all.py` and language benchmark scripts).

### Fixed & Optimized

* **Zero-Copy Slicing, Pipeline Kernels & Buffered `print`**:
  * Added `Storage::Slice` / `VmString::slice_arc` in `src/vm/string.rs` for zero-copy substring views over `Owned` strings, byte-scanning + zero-copy `csv.parse` (`src/native/csv.rs`), zero-copy `url.parse` / `decode_query` (`src/native/url.rs`), direct `BufReader`-bypassing `Reader.read_chunk` (`src/native/fs.rs`), and 64 KiB buffered `print` when `stdout` is not a TTY (`src/vm/exec_map.rs`) with automatic flushing in `Vm::call`, `process.*`, and `log.*`.
  * Extended `src/vm/pipeline.rs` (`try_run_int_reduce_fast` and new `try_run_float_reduce_fast`) to natively execute `filter` comparisons (`==`, `!=`, `<`, `<=`, `>`, `>=` on `Int`, and `==`, `<`, `<=`, `>`, `>=` on `Float`), `reduce` for `min`/`max` alongside `sum`, `Float` affine `map(a * x + b)` (including `Op::FmaFloat`), `filter |> reduce` / `map |> reduce` combinations, `const M` divisor specialization + `overflowing_*` overflow-flag loops for `Int`, and branchless `-0.0` accumulation for `Float` sum while preserving exact overflow and IEEE-754 `NaN`/`-0.0` semantics.

## [0.2.5] - 2026-11-16

### Added

* **`cdt.fs` Standard Library Module (Basic & Streaming File I/O)**:
  * Added basic file and directory operations: `read_text`, `write_text`, `append_text`, `write_atomic` (temp-file + rename), `read_lines`, `exists`, `is_file`, `is_dir`, `size`, `list_dir`, `mkdir_all`, `remove`, `copy`, and `rename`, returning `FsError { kind, path, message }` on I/O failures.
  * Added high-throughput streaming operations backed by 64 KB `BufReader`/`BufWriter` VM handles: `open_read` -> `Reader` (`read_line`, `read_chunk`, `close`), `open_write` / `open_append` -> `Writer` (`write`, `write_line`, `flush`, `close`), and `each_line(path, cb)` for zero-list-allocation line streaming.
  * Added `cdt.fs` type-checking (`src/frontend/check_fs.rs`), IR lowering (`EXT_FS_UNARY`, `EXT_FS_BINARY`), VM execution (`src/native/fs.rs`, `src/vm/exec_fs.rs`), LSP hover/completion docs (`src/lsp_docs.rs`), and end-to-end tests (`cdt_fs_basic_and_streaming`).
* **`cdt.path`, `cdt.process`, and `cdt.crypto` Standard Library Modules**:
  * Added `cdt.path` (`join`, `join_all`, `dirname`, `basename`, `ext`, `stem`, `normalize`, `is_abs`, `with_ext`) for cross-platform lexical path handling (`EXT_PATH_OP`).
  * Added `cdt.process` (`run`, `status`, `cwd`, `set_cwd`, `pid`, `exit`, `Output`, `ProcessError`) for spawning external processes (`EXT_PROCESS_OP`).
  * Added zero-dependency `cdt.crypto` (`sha256`, `hmac_sha256`, `base64_encode`, `base64_decode`, `hex_encode`, `hex_decode`, `uuid_v4`, `random_bytes_hex`, `random_int`, `CryptoError`) (`EXT_CRYPTO_OP`), with LSP hover/completion, TextMate highlighting, and library docs (`docs/src/library/{path,process,crypto}.md`).
* **Cross-OS Benchmarks & Starter Example Updates**:
  * Added `.github/workflows/bench.yml` and `scripts/bench/` (`run_all.py`, `bench.{cdt,go,swift,kt,mjs,php,py}`) for cross-language benchmarking on Linux, macOS, and Windows.
  * Updated `example/` (`events.json`, `notes.txt`, `main.cdt`, `Dockerfile`, `README.md`) to demonstrate `cdt.fs` reading and streaming.

### Fixed & Optimized

* **Pipeline `reduce` Fusion (`filter |> map |> reduce`)**:
  * Added `PipelineStage::ReduceScalar` and `EXT_PIPELINE_REDUCE` (`src/ir/lower_call.rs`, `src/vm/pipeline.rs`) to execute scalar `map`/`filter`/`reduce` chains in a single fused `exec_scalar_leaf` pass with zero intermediate list allocations and no per-element `CallFrame`.
* **Crypto, Path & FS Performance & Safety**:
  * Replaced the insecure non-Unix RNG fallback with Windows `BCryptGenRandom` (`bcrypt.dll`) and `/dev/urandom` behind a 4 KiB thread-local `RNG_BUF` (`src/native/crypto.rs`).
  * Added hardware SHA-256 acceleration for both `aarch64` (`sha2`) and `x86_64` (`sha`, `sse4.1`, `ssse3`) with zero-allocation tail padding, plus zero-dependency SIMD Base64 (`src/native/b64_simd.rs` — ARM64 NEON & x86_64 AVX2) and `from_utf8_unchecked` for `base64_encode`.
  * Eliminated input string copies in `src/vm/exec_sys.rs` (`Arc<VmString>`), added Windows fast-paths in `src/native/path.rs`, combined `Writer.write_line` into a single `BufWriter` write using `MaybeUninit` (no per-call 4 KiB stack zeroing), and enabled `lto = "fat"` + `panic = "abort"` in `Cargo.toml`.
  * Reused `Arc<String>` line buffers across `fs.each_line` iterations, pre-sized `Vec` capacity in `fs.read_lines`, added zero-zeroing `BufRead::fill_buf()` fast-paths with safe UTF-8 boundary handling in `Reader.read_chunk`, and added `sync_frame_ptrs!()` after `Op::ExtOp`.
* **Struct Constructor Loop Fix**:
  * Fixed `lower_struct_ctor` (`src/ir/lower_call.rs`) to emit `Op::NewRecord` instead of `Op::NewRecordMove` so string literals or outer variables in loop-constructed structs are not moved out on the first iteration.

## [0.2.4] - 2026-11-15

### Added & Improved

* **Language Server (`conduit lsp`)**:
  * Reintroduced `conduit lsp` over stdio JSON-RPC (`src/lsp.rs`, `src/lsp_docs.rs`) with `check_file_with_overrides` (`src/frontend/mod.rs`) for real-time diagnostics across unsaved buffers and relative imports.
  * Added `textDocument/hover` showing parameter names/types, return union types, and leading `//` comments for user `func` / `struct` declarations, all 5 `cdt.*` modules, and built-in methods/keywords, plus `\uXXXX` JSON unescaping for JetBrains LSP4IJ (Gson).
  * Added `textDocument/completion` for `cli.`, `env.`, `json.`, `time.`, `validation.`, user symbols, and keywords.
* **Editor & Docs**:
  * Enhanced `editors/textmate/conduit.tmLanguage.json` (stdlib errors/types, `PascalCase` user types, function calls, `reduce`, floats, and `{...}` string interpolation) and added `"publisher": "resoul"` in `editors/textmate/package.json`.
  * Updated [Editor Setup](docs/src/getting-started/editors.md), [editors/README.md](editors/README.md), and [CLI docs](docs/src/getting-started/cli.md) with `conduit lsp` and JetBrains LSP4IJ instructions.

## [0.2.3] - 2026-11-15

### Added

* **`cdt.cli` Command-Line Parser & DSL**:
  * Added the `cdt.cli` standard library module (`cli.parse<T>()`, `cli.parse_args<T>()`, `cli.help<T>()`, `CliError`) with support for flags, options, positional arguments, `rest` arguments, nested subcommands, command groups, and automatic `-h`/`--help` generation.
  * Added **both** the declarative `cli Name { flag, opt, arg, rest, cmd, group }` DSL (desugaring to structs at parse time) and `@cli.flag` / `@cli.opt` / `@cli.arg` / `@cli.rest` / `@cli.cmd` / `@cli.group` struct field annotations on a single shared native `CliPlan` engine (`EXT_CLI_*` opcodes + `.cdtb` bundle serialization).
  * Added [cdt.cli documentation](docs/src/library/cli.md) and updated TextMate syntax highlighting (`editors/textmate/conduit.tmLanguage.json`).
* **`conduit watch` & Docker**:
  * Added `conduit watch <file.cdt> [args...]` for automatic recompilation and restart when `.cdt` files change.
  * Added multi-stage Alpine/musl `Dockerfile`, `.dockerignore`, and `.github/workflows/docker.yml` for multi-arch (`linux/amd64`, `linux/arm64`) publishing to `ghcr.io` and Docker Hub.

## [0.2.2] - 2026-11-15

### Added

* **`conduit build` Standalone Executables**:
  * Added `conduit build <file.cdt> [-o <output>]` to bundle a compiled `Module` (`CDTB0001` trailer) into a self-contained native executable with zero external dependencies, including automatic Mach-O `__LINKEDIT` extension and ad-hoc `codesign` on macOS.
  * Standalone binaries skip frontend parsing/type-checking on startup and forward CLI arguments directly to `env.args()`.
* **Multi-File CLI Resolution**:
  * Added filesystem relative `import` resolution (`import ... from "./..."`) across `conduit build`, `conduit run`, `conduit check`, and `conduit disasm`.

## [0.2.1] - 2026-11-15

### Added

* **Standard Methods & Numeric Helpers**:
  * Added `String` methods: `.is_empty()`, `.contains(sub)`, `.starts_with(prefix)`, `.ends_with(suffix)`, `.split(sep)`, and `.replace(from, to)`.
  * Added `List` (`[T]`) methods: `.is_empty()`, `.first()`, `.last()`, `.sort()` (for `[Int]`, `[Float]`, `[String]`), `.find(pred)`, `.any(pred)`, `.all(pred)`, and global `reduce(list, init, fn)` with `|>` pipeline support.
  * Added `Int` / `Float` numeric methods: `.abs()`, `.min(other)`, `.max(other)`, and `.clamp(lo, hi)`.
  * Added `cdt.env` standard library module (`env.load<T>()`, `env.get()`, `env.require()`, `env.args()`, `EnvError`) with `@env` / `@env("VAR")` field annotations and default-value fallbacks.
* **Language Features**:
  * Added default values (`= expr`) for `struct` fields and `func` parameters.
  * Completed optional chaining (`?.`) type-checking and VM lowering across program, builtin, and native structs without double-wrapping `T?`.
* **CLI & Documentation**:
  * Added `conduit --version` / `-V` and updated `conduit --help` / `-h` usage output.
  * Aligned documentation, TextMate grammar, and Highlight.js syntax with `struct` declarations and verified runnable stdlib examples with `conduit check`.

## [0.2.0] - 2026-11-14

### Added & Changed

* **Register Bytecode VM & Compiler Architecture**:
  * Replaced the tree-walking AST interpreter with a register-based bytecode VM (`Op`, `Module`, `Vm`) using split scalar (`u64`) and object (`Obj`) register files (`size_of::<Obj>() <= 16`, `size_of::<Op>() <= 16`).
  * Added IR lowering (`FnLowerer`) with preallocated local slots, copy-on-write (`Arc`) semantics for collections/records/strings, fused `RunPipeline` execution for `|>` / `map` / `filter` chains, and `Op::ExtOp` compact dispatch.
  * Preserved zero external dependencies (pure Rust `std` only) and kept all source files under 700 lines.

* **Language Features, Generics & Module System**:
  * Added TypeScript-style named and namespace imports (`import { a, b } from "..."`, `import * as ns from "..."`) with reachability tree-shaking of unused functions.
  * Added compile-time monomorphization for generic structs and functions (`struct Box<T>`, `func f<T>`), string interpolation (`"hello {x}"`), and tuple destructuring (`(a, b) := expr`).
  * Added `print(msg)` builtin, `Float.ceil()`, prelude error constructors (`IndexError`, `KeyError`, `ParseError`), `String.to_int()` / `String.to_float()`, and full `Map<K, V>` support (`.set`, `.get`, `.contains_key`, `.remove`, `.keys`, `.values`, `.entries`, `Map.from`, `map[k] -> V | KeyError`, `for (k, v) in map`).

* **Native Standard Libraries & Benchmarks**:
  * Rebuilt native `cdt.json` (`decode`, `encode`, `parse`, `stringify`), `cdt.time` (`now`, `monotonic`, `elapsed`, `seconds`, `milliseconds`, `Timestamp`, `Duration`), and `cdt.validation` (`min`, `max`, `email`, `check`, custom `[#validator]` attributes) on top of the VM.
  * Added built-in `conduit bench [--strict]` suite covering 34 compute, call, closure, collection, pipeline, and JSON workloads.

## [0.1.11] - 2026-11-11

### Added & Changed

* **JSON Field Tags (`` `json:"..."` ``)**:
  * Added Go-style `` `json:"..."` `` raw-string field tags on `type` record declarations to customize serialized and deserialized JSON key names independently of Conduit field identifiers and `@validation` rules.
  * Added compile-time validation of field tag syntax and duplicate JSON key names within a struct, plus LSP hover rendering of field tags.
  * Formatted non-identifier JSON field names in `DecodeError.path` and `EncodeError.path` using quoted bracket segments (e.g., `["a.b"]`, `items[0]["display name"]`).

* **`cdt.json` & Interpreter Performance**:
  * Added a borrowed JSON parser (`BorrowedJsonNode<'a>`) using `Cow<'a, str>` for unescaped strings and object keys, `&'a str` number slices, slice-based string scanning, and lazy error-path construction on failure only.
  * Replaced per-instance `HashMap<String, Value>` storage in `Value::Struct` and `Value::Error` with shared `Arc<StructSchema>` metadata and indexed `Vec<Value>` slots, with `OnceLock` static schemas for builtin/native types, preserved lexicographical `Display` ordering, cursor-guided key matching in `JsonDecodePlan`, and inline cached `"\"key\":"` prefixes for zero-lookup encoding.
  * Updated `Interpreter` to share function AST bodies (`Arc<(Vec<Param>, Block)>`) and closure captured environments (`Arc<HashMap<String, Value>>`), update variables in place via `get_mut`, and borrow place/member chains in `json.decode`, `json.encode`, and zero-arg methods without intermediate clones.
  * Removed legacy DOM decoding (`decode_node_to_conduit*`) and resolver-based encoding wrappers (`encode_conduit_value*` / `JsonEncodeSchema`) in favor of `JsonDecodePlan` and `encode_with_struct_schemas`.
  * Added a cross-language JSON benchmark suite under `benchmarks/json/` (supporting configurable `--records N`) and an allocation/microbenchmark profiler crate under `benchmarks/profile/`.

* **JetBrains & TextMate Editor Setup**:
  * Added a VS Code-compatible TextMate manifest (`editors/textmate/package.json`) for `.cdt` files so RustRover and other JetBrains IDEs can load the Conduit grammar bundle.
  * Updated JetBrains editor setup documentation (`docs/src/getting-started/editors.md`, `editors/README.md`) to use LSP4IJ for configuring `conduit lsp`.

## [0.1.10] - 2026-11-11

### Added & Changed

* **Type System Soundness**:
  * Disallowed mutating captured outer `var` bindings inside closures (`Cannot mutate captured variable '...' inside a closure`), preventing silent updates lost to environment snapshotting.
  * Aligned function call resolution between `TypeChecker` and `Interpreter` so local bindings shadow top-level functions of the same name.
  * Disallowed multiple instantiations of the same generic record in a single union (`Box<Int> | Box<String>`), both in direct union annotations and after generic type parameter substitution (`Cannot combine different generic instantiations of '...' in the same union`), preventing runtime type-erasure collisions in `switch`.
  * Added contextual typing for empty list literals `[]` (`[T]` inferred from annotations, assignments, return types, function/method arguments, struct fields, `??`, and sibling list elements) while rejecting ambiguous standalone `[]` (`Cannot infer element type for empty list literal '[]'; add an explicit type annotation`).

* **String Interpolation & Syntax Fixes**:
  * Added compile-time verification of `{...}` string interpolation expressions (including dotted field paths like `{u.name}` and `{err.message}`) and support for escaped braces `\{` and `\}`.
  * Raised the precedence of `??` above comparison operators (`==`, `!=`, `<`, `<=`, `>`, `>=`) so `a ?? 0 > 5` parses as `(a ?? 0) > 5`.
  * Allowed bare `return` statements immediately before `case` inside `switch` blocks.
  * Added exact lexer/parser support for the minimum 64-bit signed integer literal `-9223372036854775808` (`i64::MIN`).

* **Runtime Safety, Performance & Hardening**:
  * Replaced unchecked `i64` arithmetic (`+`, `-`, `*`, `/`, `%`, unary `-`) and `Float` rounding conversions (`.floor()`, `.ceil()`, `.round()`, `.trunc()`) with checked operations that raise descriptive `RuntimeError`s on overflow or `NaN`/`Infinity`.
  * Enforced `MAX_CALL_DEPTH = 256` in the interpreter to prevent native stack overflow on unbounded recursion.
  * Updated mutating collection methods (`List.append`, `Map.set`, `Map.remove`) to mutate target bindings and struct fields in place without cloning the collection on each call.
  * Hardened `cdt.json`: fixed UTF-8 slicing in `\uXXXX` escape parsing, added `MAX_JSON_DEPTH = 128`, preserved `.0` on whole-number `Float` encoding (`3.0` -> `"3.0"`), and respected integer vs float syntax when decoding `Int | Float` unions.
  * Removed the `file_name()` fallback in `ReachabilityAnalyzer::paths_match` and made `Display` formatting for structs/errors and multi-module type-checking order deterministic.

## [0.1.9] - 2026-11-11

### Added & Changed

* **Optional Chaining (`?.`) & Flow-Sensitive Type Narrowing**:
  * Added `?.` optional chaining for field access (`user?.profile?.city`) and non-`Void`, non-error-union method calls (`s?.trim()?.upper()`), flattening nested optionals to `U?` without double-wrapping (`U??`) and short-circuiting argument and tail evaluation on `nil`.
  * Added flow-sensitive narrowing of `T?` to `T` via `guard x != nil else { ... }`, `if x != nil { ... } else { ... }`, `if x == nil { ... } else { ... }`, `while x != nil { ... }`, and short-circuit boolean expressions (`x != nil && ...`, `x == nil || ...`), with automatic reset back to `T?` upon assigning `nil` / `T?` / `Any` to mutable `var` bindings.
  * Updated the interpreter to short-circuit `&&` and `||` right-hand operands so guards like `x == nil || x.field == 0` never dereference `nil`.

* **Extended Loops, Destructuring, and Safe Mutation**:
  * Added `while cond { ... }` precondition loops requiring strict `Bool` conditions, supporting `break` and `continue` isolated across closure boundaries.
  * Extended `for` loops with tuple pattern destructuring (`for (idx, item) in items.enumerate()`, wildcard `_` with error-discard checks) and direct insertion-order iteration over `Map<K, V>` (`for (k, v) in map`, `for entry in map`).
  * Enabled `m[key] = value` index assignment on mutable `var` maps, nested struct field assignment (`order.meta.updated = true`), and mutating method calls on fields of `var` structs (`order.items.append(x)`), while enforcing immutability on `:=` bindings (`Cannot mutate immutable variable '...'`).
  * Added list methods `.slice(start, end)`, `.reverse()`, `.enumerate()`, and `[String].join(sep)`, plus Unicode-scalar-indexed string methods `.index_of(sub) -> Int?` and `.slice(start, end) -> String`.

* **Strict String Number Parsing & Prelude `ParseError`**:
  * Added prelude error `ParseError { value: String, message: String }` (shadowable by local user definitions) and methods `String.to_int() -> Int | ParseError` and `String.to_float() -> Float | ParseError` (rejecting whitespace without `.trim()`, `i64` overflow, and non-finite floats such as `"NaN"` / `"Infinity"`).
  * Updated LSP hover/completion entries, TextMate syntax highlighting, and language documentation.

## [0.1.8] - 2026-11-11

### Added & Changed

* **User-Defined Generic Functions & Records**:
  * Added generic record declarations `type Name<T, ...> { ... }` and generic function declarations `func name<T, ...>(...) -> ...` with validation rejecting empty (`<>`) and duplicate (`<T, T>`) type parameter lists.
  * Added explicit generic type arguments on type annotations (`Page<User>`, `alias.Page<Int>`, nested `Box<[Pair<String, Int>]>`) and function calls (`first_or_default<Int>([], 0)`), alongside type argument inference from call arguments with conflict detection (`Conflicting types inferred for type parameter 'T'`).
  * Added `Type::TypeParam` and canonical `Type::Generic { type_id, type_args }` identity so `Box<Int>` and `Box<String>` are strictly distinct, generic `switch` branches bind concrete field types, and `export` privacy recursively inspects generic type arguments.
  * Updated `ReachabilityAnalyzer` and LSP hover/completions to traverse generic type arguments.

* **Ordered Dictionary Type (`Map<K, V>`)**:
  * Added built-in `Map<K, V>` constrained to `String`, `Int`, or `Bool` keys (`"Map key type must be 'String', 'Int', or 'Bool', got '...'"`), with deterministic insertion ordering (`set` overwrites preserve original key position; `remove` then re-`set` appends to end).
  * Added `Map<K, V>()` and `Map.from([(k, v), ...])` constructors, safe `map[key] -> V | KeyError` indexing with prelude `KeyError { key: String, message: String }`, and `.set`, `.get`, `.remove`, `.contains_key`, `.len`, `.keys`, `.values`, and `.entries` methods.
  * Integrated `Map<K, V>` and generic structs with `cdt.validation` (`min`/`max` on map entry counts and recursive `validation.check` producing formatted paths such as `users_by_id["alice"].email` and `items[42].title`).

* **Floating-Point Type (`Float`) & Strict Conversions**:
  * Added `Float` (`f64`, IEEE 754) literals (`0.0`, `3.14`, `-42.5`), arithmetic (`+`, `-`, `*`, `/`), unary `-`, comparisons (`<`, `<=`, `>`, `>=`, `==`, `!=`), and runtime `Division by zero` checks to prevent silent `NaN`/`Infinity` propagation.
  * Preserved strict separation from `Int` (no implicit mixing), with explicit `Int.to_float()` and `Float` methods `.floor()`, `.ceil()`, `.round()`, `.trunc()`, `.to_string()`, `.is_nan()`, `.is_infinite()`, and `.is_finite()`.
  * Extended `cdt.validation` `min` / `max` bounds and custom `[#validator]` literal arguments (including negative floats) to support `Float`, rejecting non-finite `NaN` and `Infinity` values.

* **Standard Library `cdt.json`**:
  * Implemented zero-dependency RFC 8259 JSON parser and serializer in pure Rust `std`, exporting `json.Value`, `json.DecodeError { path, message }`, and `json.EncodeError { path, message }`.
  * Added typed `json.decode<T>(raw)` enforcing strict field presence, `null` rules, unknown field rejection, exact 64-bit integer bounds (rejecting fractions, exponents, and overflows), non-ambiguous union checking, and exact violation paths without implicitly triggering `@`-validators.
  * Added deterministic `json.encode(value)` preserving struct declaration field order and `Map` insertion order, plus unstructured `json.parse`, `json.stringify`, and `.is_null()` / `.as_*()` navigation helpers on `json.Value`.

## [0.1.7] - 2026-11-11

### Added & Changed

* **Anonymous Functions & Lexical Closures**:
  * Added `func(param: Type, ...) -> ReturnType { ... }` expression syntax (omitted return type defaults to `Void`) usable in assignments, direct calls, `map`/`filter`, pipes (`|>`), and higher-order returns.
  * Closures capture enclosing lexical bindings by value snapshot at creation time and preserve their defining module context so cross-module closures continue resolving private helpers and module types accurately.
  * Type checker isolates closure `current_return_type` (`return` and `?` apply to the closure itself) and resets `loop_depth` (`break`/`continue` cannot cross closure boundaries), enforces `block_always_returns` for non-`Void` closures, and rejects unhandled error variables inside closures.
  * Updated `ReachabilityAnalyzer` to scan parameter types, return types, and bodies of closure expressions.

* **String Methods & Explicit `.to_string()`**:
  * Added `String` methods: `.split(sep: String) -> [String]`, `.starts_with(prefix: String) -> Bool`, `.ends_with(suffix: String) -> Bool`, `.replace(from: String, to: String) -> String` (returning the original string unchanged when `from == ""`), and `.char_count() -> Int` (Unicode scalar count).
  * Documented `String.len()` as UTF-8 byte length (`O(1)`) in contrast with `.char_count()` and `cdt.validation` `min`/`max` character-count bounds (Unicode scalar values).
  * Added explicit `.to_string() -> String` on `Int`, `Bool`, and `String` while preserving strict non-coercion for `+`.
  * Updated LSP hover/completion and documentation examples for closures, strings, and `.to_string()`.

## [0.1.6] - 2026-11-11

### Added & Changed

* **Canonical Type Identity Across Compiler & Runtime**:
  * Replaced suffix and short-name matching with canonical `TypeId` (`ModuleId` + type name) distinguishing `Builtin`, `Program`, canonical `Local(PathBuf)`, and `Native(String)` modules.
  * Updated `TypeChecker`, `Value::Struct` / `Value::Error`, `NativeValue`, `switch` pattern matching, native method lookup, and validation schema resolution (`module_struct_fields`) to use canonical `TypeId`.
  * Two types with identical names declared in different modules (e.g. `a.User` vs `b.User`) are now strictly distinct and incompatible across compile-time checks, runtime `switch`, and `cdt.validation.check`.
  * Importing the same module under different aliases consistently resolves to the exact same canonical type.

* **Safe List Indexing & Prelude `IndexError`**:
  * Added list index operator `list[index]` returning `T | IndexError` for `[T]` with `Int` index.
  * Added prelude error `IndexError { index: Int, length: Int, message: String }` (available without imports). Both `index < 0` and `index >= list.len()` produce `IndexError`.
  * Added `list.get(index: Int) -> T?` returning `nil` for out-of-bounds or negative indices without double-wrapping `U?` elements.
  * Updated `ReachabilityAnalyzer` and parser postfix chaining to support indexed expressions (such as `err.issues[0]?` and `matrix[r]?[c]?`).

## [0.1.5] - 2026-11-10

### Added

* **Custom Validators and Field Annotations**:
  * Function marker `[#validator]` (case-insensitive) to declare validator functions directly preceding `func` or `export func`.
  * Strict validator function contract: `func rule(value: T, extra_params...) -> Void | validation.Issue`.
  * Struct field annotations: `@rule` or `@module.rule(args...)` executed in declaration order.
  * Parameterized validator annotations supporting positional literals, including negative integer literals (e.g. `@range(-10, 100)`).
  * Optional field skipping: fields of type `T?` validate `T` when present and automatically skip validation when `nil`.
  * Preserved first-class function semantics: validators can be passed as callbacks or invoked directly.

* **Standard Validation Library (`cdt.validation`)**:
  * Public types:
    * `error Issue { code: String, message: String }`: Single rule violation.
    * `type Violation { path: String, code: String, message: String }`: Attributed field violation with path (e.g. `user.profile.email` or `items[0].code`).
    * `error ValidationError { issues: [Violation] }`: Aggregated errors for `switch` and `?` handling.
  * Standard rules:
    * `min(value, min_val)` and `max(value, max_val)`: bounds checking for `Int`, Unicode scalar count for `String`, and element count for `List`.
    * `email(value: String)`: format validation for email addresses.
    * `check(value: T) -> T | validation.ValidationError`: explicit recursive validation preserving value and types on success without mutation.
  * Reachability analysis integration: struct field annotations are traced as function references, ensuring referenced validators and their transitive dependencies are preserved.

* **Official Documentation on mdBook**:
  * Complete documentation site structure in `docs/` written entirely in English.
  * Verified runnable documentation examples under `examples/docs/` covering getting started, core language, standard library, and practical recipes.
  * Negative example tests (`examples/docs/negative/`) verifying diagnostics for discarded errors, private type leakage, and invalid `?` on optionals.
  * Automated documentation example verification test suite (`tests/doc_examples_test.rs`).
  * Deterministic API markdown documentation generator from `NativeRegistry` (`conduit doc [module]`).
  * Custom syntax highlighting script for mdBook (`docs/theme/conduit-highlight.js`) supporting `conduit` and `cdt` code blocks.
  * Dedicated GitHub Actions workflow (`.github/workflows/docs.yml`) for building, verifying, and publishing documentation to GitHub Pages.

## [0.1.4] - 2026-11-10

### Added

* **Explicit Exports (`export func`, `export type`, `export error`)**:
  * Top-level declarations are private to the defining module by default.
  * Explicit `export` keyword exposes functions, types, and errors to importing modules. Case does not determine visibility.
  * Strict private type leakage check: exported functions, structs, or errors cannot expose private types in parameters, return types, fields, or nested in collections, tuples, unions, or function signatures.

* **Module Imports & Canonical Path Resolution**:
  * Standard library imports: `import cdt.time` (default alias `time`) and `import cdt.time as clock`.
  * Local file imports: `import "./users" as users` and `import "../shared/validation" as validation`.
  * Canonical path normalization ensures a file loaded via different relative paths creates a single module identity.
  * Cyclic import detection with clear error chains (e.g., `a.cdt -> b.cdt -> a.cdt`).

* **First-Class Functions and Callbacks Across Modules**:
  * Imported and native functions can be passed as first-class values (`op := users.find; op(42)`).
  * Seamless integration with `map(list, callback)` and `filter(list, callback)`.

* **Unified Native Registry & `cdt.time` Standard Library**:
  * Shared registry architecture (`NativeRegistry`) for native modules, functions, opaque types, and methods.
  * First standard library `cdt.time`:
    * Opaque types: `time.Timestamp`, `time.Instant`, `time.Duration`.
    * Typed error: `time.TimeError { message: String }`.
    * Functions: `time.now()`, `time.monotonic()`, `time.elapsed(start)`, `time.seconds(val)`, `time.milliseconds(val)`.
    * Methods: `Timestamp.unix_seconds()`, `Timestamp.unix_milliseconds()`, `Duration.seconds()`, `Duration.milliseconds()`, `Duration.nanoseconds()`.
    * Negative Unix timestamp flooring towards negative infinity (half a second before epoch produces `-1` Unix second).
    * `elapsed()` validates monotonic clock progression, safely handling non-negative durations and reporting invalid/reversed order as `TimeError`.

* **Reachability Analysis Tool (`conduit reachability`)**:
  * Static dead-code and dependency analyzer tracing functions and native dependencies reachable from `main`.
  * Generates summary reports showing reachable vs unused functions count and active native dependencies.

* **LSP & TextMate Grammar Support**:
  * Added `export`, `import`, and `as` keywords to syntax grammar and LSP completion.
  * Context-aware autocompletion on module dot triggers (`time.`, `<alias>.`), exposing only exported items.
  * LSP hover integrated with `NativeRegistry` documentation and function/method signatures.

* **Examples (`examples/modules/`)**:
  * Added runnable multi-module demo `users.cdt` and `main.cdt`.
  * Added `time_demo.cdt` demonstrating `cdt.time` operations with propagation and switch pattern matching.

---

## [0.1.3] - 2026-11-10

### Added

* **Typed Errors as Return Values (`error Name { ... }`)**:
  * Errors are regular typed values declared via `error Name { fields }` and returned via `return`.
  * Zero exceptions (`throw`/`try`/`catch`) and no implicit inheritance.
  * Static diagnostics prohibiting discarded error expressions at statement level and requiring all local error variables to be handled.
  * Static validation of constructor fields, types, and named arguments.

* **Union Types (`A | B`)**:
  * First-class union types supporting order independence, deduplication, and nested union flattening (`Type::union`).
  * Full static subtyping: `A` is assignable to `A | B`, and union `A | B` is assignable if every source variant satisfies a target variant.
  * Support in variable declarations, parameters, return types, and collections.
  * Direct field access on union types is disallowed without narrowing.

* **Exhaustive Pattern Matching (`switch`)**:
  * Pattern matching statement over typed values, primitives (`Int`, `String`, `Bool`), tuples, and `nil`.
  * Binding syntax: `case User(user):` binds `user` with narrowed type.
  * Static exhaustiveness check requiring all union variants to be covered.
  * Rejection of redundant, shadowed, or impossible branches.
  * Control flow analysis: if all branches of an exhaustive `switch` return, the statement is treated as guaranteed return.

* **Error Propagation Operator (`?`)**:
  * Postfix `?` evaluating expression once, immediately returning any error variant from the enclosing function, and narrowing expression to success variants.
  * Static return type validation ensuring all error variants are assignable to current function's return type.
  * Preserves `nil` when propagating from optional unions `(User?) | InvalidUserName -> User?`.
  * Strictly rejects propagation on pure optionals without error variants.

* **Tuples & Destructuring (`(A, B)`)**:
  * Multi-element tuples (2+ elements) with positional access and static type checking.
  * Tuple destructuring definition `(a, b) := expr` and `(a, _) := expr`.
  * Static check prohibiting wildcard `_` from discarding error-carrying elements.
  * Tuple patterns inside `switch` cases with automatic union type inference for matched positions.

* **Higher-Order Functions & Collection Pipelines**:
  * `map(list, callback)` and `filter(list, callback)` support function values and preserve error elements as values.

* **Language Server Protocol & TextMate Grammar**:
  * LSP hover and autocompletion for `error`, `switch`, and `case`.
  * TextMate syntax grammar updated for `error`, `switch`, `case`, and union operator `|`.

* **Examples (`examples/functions/`)**:
  * [08_errors_and_unions.cdt](examples/functions/08_errors_and_unions.cdt): Demonstrates error declarations, union return types, `?` operator, and exhaustive `switch`.
  * [09_tuples_and_guards.cdt](examples/functions/09_tuples_and_guards.cdt): Demonstrates tuples, destructuring, guard validation, and pattern matching.

---

## [0.1.2] - 2026-11-10

### Added

* **Loop Control Statements (`break` and `continue`)**:
  * Added `break` and `continue` keywords to lexer, AST, and parser.
  * Static control flow analysis in `check.rs` preventing `break` or `continue` outside of `for` loops.
  * Runtime loop flow termination and step progression in `interp.rs`.
  * LSP hover information and autocompletion snippets for loop keywords.

* **Pipeline Operator (`|>`)**:
  * Pipe operator `|>` allowing sequential data processing chains (e.g. `data |> filter() |> double() |> print()`).
  * Elegant compile-time AST desugaring in `parser.rs`: `left |> right_call(args...)` desugars into `right_call(left, args...)`, and `left |> right_ident` desugars into `right_ident(left)`.
  * Zero runtime overhead with full static type safety.

* **Examples (`examples/functions/`)**:
  * [07_loops_and_pipes.cdt](examples/functions/07_loops_and_pipes.cdt): Demonstrates `break`, `continue`, and data processing pipelines using `|>`.

---

## [0.1.1] - 2026-11-10

### Added

* **Guard Statements (`guard <cond> else { ... }`)**:
  * Added `guard` keyword for early exit precondition validation to eliminate deeply nested `if` blocks.
  * Static Control Flow analysis in `check.rs` ensuring the `else` block strictly terminates with a `return` statement (`block_always_returns`).
  * Runtime execution with early return in `interp.rs`.

* **Null-Coalescing Operator (`??`)**:
  * Binary operator `??` for safe unpacking of optional types (`custom_port ?? 8080`).
  * Static type checking verifying default value matches optional inner type (`T? ?? T -> T`).
  * Runtime short-circuit evaluation: if the left operand is not `nil`, the right operand is not evaluated.

* **Examples (`examples/functions/`)**:
  * [06_guard_and_coalesce.cdt](examples/functions/06_guard_and_coalesce.cdt): Demonstrates `guard` statements and `??` operator.

* **Language Server Protocol (`lsp.rs`) & Editor Integration**:
  * Built-in zero-dependency LSP server responding to `conduit lsp`.
  * Real-time live error diagnostics (`textDocument/publishDiagnostics`) running Lexer, Parser, and Type Checker on open and change events.
  * Context-aware hover information (`textDocument/hover`).
  * Autocompletion suggestions and snippet triggers (`textDocument/completion`).
  * TextMate syntax grammar (`editors/textmate/conduit.tmLanguage.json`) for instant syntax highlighting in JetBrains (RustRover) and VS Code.

---

## [0.1.0] - 2026-11-10

Initial release of the Conduit programming language core.

### Added

* **Lexer (`lexer.rs`)**:
  * Pure Rust zero-dependency tokenizer with line and column tracking (`Span`).
  * Literal tokens: integers (`Int`), strings (`String`), booleans (`true`, `false`, `nil`).
  * Operator tokens: `:=`, `->`, `..`, `==`, `!=`, `<=`, `>=`, `&&`, `||`, `+`, `-`, `*`, `/`, `%`.
  * Support for single-line comments (`//`).
  * CLI command: `conduit tokens <file.cdt>`.

* **Parser & AST (`parser.rs`, `ast.rs`)**:
  * Abstract Syntax Tree definitions: `Item`, `Stmt`, `Expr`, `TypeExpr`.
  * Handcrafted Recursive Descent parser with precedence climbing for binary expressions.
  * Declarations: `func name(params) -> ReturnType { body }` and `type Name { fields }`.
  * Statements: `if / else`, `for in`, `return`, `var`, variable definition `:=`, assignment `=`.
  * Method chaining (e.g. `item.name.trim().lower()`) and struct constructor calls with named arguments.
  * CLI command: `conduit ast <file.cdt>`.

* **Type Checker (`check.rs`, `types.rs`)**:
  * Static type analysis before code execution.
  * Two-pass symbol registration allowing forward references for functions and types.
  * Lexical scope stack with `Vec<HashMap<String, Type>>`.
  * Return type validation for `return` statements.
  * Argument type validation for function and method calls.
  * CLI command: `conduit check <file.cdt>`.

* **Runtime & Interpreter (`interp.rs`, `value.rs`)**:
  * Tree-walking AST interpreter.
  * Runtime values (`Value::Int`, `String`, `Bool`, `Nil`, `Void`, `List`, `Struct`).
  * Flow control with early returns (`ControlFlow::Return`).
  * String interpolation: `"{var}"` evaluated from the runtime environment.
  * Built-in functions: `print(...)`.
  * Built-in methods for strings (`trim`, `lower`, `upper`, `len`, `contains`) and lists (`len`, `append`, `contains`).
  * CLI command: `conduit run <file.cdt>` (runs static type-checking prior to execution).

