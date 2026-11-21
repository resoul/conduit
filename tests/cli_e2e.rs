use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_cdt(name: &str, content: &str) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("conduit_cli_e2e_{}_{}", std::process::id(), n));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, content).unwrap();
    path
}

fn conduit_bin() -> &'static str {
    env!("CARGO_BIN_EXE_conduit")
}

#[test]
fn cli_check_reports_line_col_and_rejects_bad_sql_ident() {
    let bad_type = temp_cdt(
        "bad_type.cdt",
        "export func main() -> Int {\n    x := \"hello\" + 1\n    return 0\n}\n",
    );
    let out = Command::new(conduit_bin())
        .args(["check", bad_type.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[2:"),
        "expected [line:col] in check error, got: {stderr}"
    );

    let bad_sql = temp_cdt(
        "bad_sql.cdt",
        "import * as db from \"cdt.db\"\n@db.table(\"users; DROP TABLE x\")\nstruct Bad { id: Int }\nexport func main() -> Void {}\n",
    );
    let out2 = Command::new(conduit_bin())
        .args(["check", bad_sql.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!out2.status.success());
    let stderr2 = String::from_utf8_lossy(&out2.stderr);
    assert!(
        stderr2.contains("Invalid SQL identifier"),
        "expected SQL identifier rejection, got: {stderr2}"
    );
}

#[test]
fn cli_run_db_ddl_and_crypto_passwords() {
    let src = r#"
import * as db from "cdt.db"
import * as crypto from "cdt.crypto"

@db.table("accounts")
struct Account {
    id: Int @db.pk @db.auto
    email: String @db.unique
}

export func main() -> Void | crypto.CryptoError {
    ddl := db.ddl<Account>("postgres")
    h := crypto.hash_password("s3cr3t")?
    ok := crypto.verify_password("s3cr3t", h)
    print("{ok}:{ddl}")
}
"#;
    let file = temp_cdt("db_ddl.cdt", src);
    let out = Command::new(conduit_bin())
        .args(["run", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(r#"true:CREATE TABLE IF NOT EXISTS "accounts" ("id" BIGSERIAL PRIMARY KEY, "email" TEXT NOT NULL UNIQUE)"#));
}

#[test]
fn cli_cron_list_print_and_run() {
    let src = r#"
import * as cron from "cdt.cron"

schedule Jobs {
    every "*/5 * * * *" => ping
    interval 1s => beat
}

func ping() -> Void {
    print("pong-from-cron")
}

func beat() -> Void {
    return
}

export func main() -> Void {
    return
}
"#;
    let file = temp_cdt("jobs.cdt", src);

    let list_out = Command::new(conduit_bin())
        .args(["cron", "list", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(list_out.status.success());
    let list_str = String::from_utf8_lossy(&list_out.stdout);
    assert!(list_str.contains("schedule Jobs:"));
    assert!(list_str.contains("ping -> every \"*/5 * * * *\""));
    assert!(list_str.contains("beat -> interval 1000ms"));

    let print_out = Command::new(conduit_bin())
        .args(["cron", "print", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(print_out.status.success());
    let print_str = String::from_utf8_lossy(&print_out.stdout);
    assert!(print_str.contains("*/5 * * * * conduit cron run"));
    assert!(print_str.contains(" ping"));

    let run_out = Command::new(conduit_bin())
        .args(["cron", "run", file.to_str().unwrap(), "ping"])
        .output()
        .unwrap();
    assert!(run_out.status.success());
    let run_str = String::from_utf8_lossy(&run_out.stdout);
    assert!(run_str.contains("pong-from-cron"));
}

#[test]
fn cli_build_and_exec_standalone_binary() {
    let src = "export func main() -> Void {\n    print(\"standalone-ok\")\n}\n";
    let file = temp_cdt("standalone.cdt", src);
    let bin_path = file.with_file_name(format!("standalone_bin{}", std::env::consts::EXE_SUFFIX));

    let build_out = Command::new(conduit_bin())
        .args([
            "build",
            file.to_str().unwrap(),
            "-o",
            bin_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        build_out.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build_out.stderr)
    );

    let run_out = Command::new(&bin_path).output().unwrap();
    assert!(
        run_out.status.success(),
        "standalone failed: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&run_out.stdout);
    assert!(stdout.contains("standalone-ok"));
}

#[test]
fn cli_test_runner_pass_fail_filter_and_dir_discovery() {
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("conduit_test_suite_{}_{}", std::process::id(), n));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let helper_path = dir.join("helper.cdt");
    fs::write(
        &helper_path,
        r#"
export func triple(x: Int) -> Int {
    return x * 3
}
test "helper passes" {
    assert_eq(triple(4), 12)
}
"#,
    )
    .unwrap();

    let app_test_path = dir.join("app_test.cdt");
    fs::write(
        &app_test_path,
        r#"
import { triple } from "./helper"

test "alpha math" {
    assert_eq(triple(5), 15)
}

test "beta fail" {
    assert_eq(1 + 1, 3)
}

test "gamma after fail" {
    assert(true)
}
"#,
    )
    .unwrap();

    // 1. Single file with --filter passes when failing test is filtered out.
    let filter_out = Command::new(conduit_bin())
        .args(["test", app_test_path.to_str().unwrap(), "--filter", "alpha"])
        .output()
        .unwrap();
    assert!(
        filter_out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&filter_out.stdout),
        String::from_utf8_lossy(&filter_out.stderr)
    );
    let filter_stdout = String::from_utf8_lossy(&filter_out.stdout);
    assert!(filter_stdout.contains("PASS  alpha math"), "{filter_stdout}");
    assert!(filter_stdout.contains("1 passed; 0 failed; 2 filtered out"), "{filter_stdout}");

    // 2. Single failing file without filter exits 1 and runs all tests.
    let fail_out = Command::new(conduit_bin())
        .args(["test", app_test_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!fail_out.status.success());
    let fail_stdout = String::from_utf8_lossy(&fail_out.stdout);
    assert!(fail_stdout.contains("FAIL  beta fail"), "{fail_stdout}");
    assert!(fail_stdout.contains("actual:   2"), "{fail_stdout}");
    assert!(fail_stdout.contains("expected: 3"), "{fail_stdout}");
    assert!(fail_stdout.contains("PASS  gamma after fail"), "{fail_stdout}");
    assert!(fail_stdout.contains("2 passed; 1 failed"), "{fail_stdout}");

    // 3. --fail-fast stops after first failure before running gamma.
    let ff_out = Command::new(conduit_bin())
        .args(["test", app_test_path.to_str().unwrap(), "--fail-fast"])
        .output()
        .unwrap();
    assert!(!ff_out.status.success());
    let ff_stdout = String::from_utf8_lossy(&ff_out.stdout);
    assert!(ff_stdout.contains("FAIL  beta fail"), "{ff_stdout}");
    assert!(!ff_stdout.contains("gamma after fail"), "{ff_stdout}");
    assert!(ff_stdout.contains("1 passed; 1 failed"), "{ff_stdout}");

    // 4. Directory discovery finds both files cleanly when passing.
    fs::write(
        &app_test_path,
        "import { triple } from \"./helper\"\ntest \"app ok\" { assert_eq(triple(5), 15) }\n",
    )
    .unwrap();
    let dir_out = Command::new(conduit_bin())
        .args(["test", dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        dir_out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&dir_out.stdout),
        String::from_utf8_lossy(&dir_out.stderr)
    );
    let dir_stdout = String::from_utf8_lossy(&dir_out.stdout);
    assert!(dir_stdout.contains("PASS  app ok"), "{dir_stdout}");
    assert!(dir_stdout.contains("PASS  helper passes"), "{dir_stdout}");
    assert!(dir_stdout.contains("2 passed; 0 failed; 0 filtered out"), "{dir_stdout}");

    // 5. Importing ./helper from a single test file does NOT execute helper's inline test.
    let solo_out = Command::new(conduit_bin())
        .args(["test", app_test_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(solo_out.status.success());
    let solo_stdout = String::from_utf8_lossy(&solo_out.stdout);
    assert!(!solo_stdout.contains("helper passes"), "{solo_stdout}");
    assert!(solo_stdout.contains("1 passed; 0 failed"), "{solo_stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_check_pretty_multi_error_and_imported_file_diagnostics() {
    let multi_err = temp_cdt(
        "multi_err.cdt",
        "func a() -> Int {\n    x := \"hello\" + 1\n    return x\n}\nexport func main() -> Void {\n    y := missing_ident\n}\n",
    );
    let out = Command::new(conduit_bin())
        .args(["check", multi_err.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("error[2:"), "{stderr}");
    assert!(stderr.contains("error[6:"), "{stderr}");
    assert!(stderr.contains("-->"), "{stderr}");
    assert!(stderr.contains(" | "), "{stderr}");
    assert!(stderr.contains("^"), "{stderr}");
    assert!(stderr.contains("aborting due to 2 previous errors"), "{stderr}");
    let _ = fs::remove_dir_all(multi_err.parent().unwrap());

    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("conduit_cli_diag_mod_{}_{}", std::process::id(), n));
    fs::create_dir_all(&dir).unwrap();
    let mod_file = dir.join("bad_mod.cdt");
    fs::write(
        &mod_file,
        "export func broken() -> Int {\n    bad := \"abc\" + 2\n    return bad\n}\n",
    )
    .unwrap();
    let main_file = dir.join("main.cdt");
    fs::write(
        &main_file,
        "import { broken } from \"./bad_mod\"\nexport func main() -> Void {\n    broken()\n}\n",
    )
    .unwrap();
    let imp_out = Command::new(conduit_bin())
        .args(["check", main_file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!imp_out.status.success());
    let imp_stderr = String::from_utf8_lossy(&imp_out.stderr);
    assert!(imp_stderr.contains("bad_mod.cdt"), "{imp_stderr}");
    assert!(imp_stderr.contains("bad := \"abc\" + 2"), "{imp_stderr}");
    assert!(imp_stderr.contains("^"), "{imp_stderr}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_routes_and_openapi_commands() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let auth_main = manifest_dir.join("examples/auth_api/main.cdt");
    let cli_http_main = manifest_dir.join("examples/cli_http/main.cdt");

    let routes_auth = Command::new(conduit_bin())
        .args(["routes", auth_main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        routes_auth.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&routes_auth.stderr)
    );
    let routes_auth_out = String::from_utf8_lossy(&routes_auth.stdout);
    assert!(routes_auth_out.contains("router AuthApi"), "{routes_auth_out}");
    assert!(routes_auth_out.contains("POST"), "{routes_auth_out}");
    assert!(routes_auth_out.contains("/auth/login"), "{routes_auth_out}");
    assert!(routes_auth_out.contains("LoginRequest"), "{routes_auth_out}");
    assert!(routes_auth_out.contains("/api/me"), "{routes_auth_out}");
    assert!(routes_auth_out.contains("guard: require_jwt"), "{routes_auth_out}");

    let routes_cli = Command::new(conduit_bin())
        .args(["routes", cli_http_main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        routes_cli.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&routes_cli.stderr)
    );
    let routes_cli_out = String::from_utf8_lossy(&routes_cli.stdout);
    assert!(routes_cli_out.contains("router DemoApi"), "{routes_cli_out}");
    assert!(routes_cli_out.contains("cors: *"), "{routes_cli_out}");
    assert!(routes_cli_out.contains("/admin/stats"), "{routes_cli_out}");
    assert!(routes_cli_out.contains("before: check_admin_auth"), "{routes_cli_out}");

    let openapi_out = Command::new(conduit_bin())
        .args([
            "openapi",
            auth_main.to_str().unwrap(),
            "--server",
            "http://127.0.0.1:8080",
        ])
        .output()
        .unwrap();
    assert!(
        openapi_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&openapi_out.stderr)
    );
    let spec = String::from_utf8_lossy(&openapi_out.stdout);
    assert!(spec.contains(r#""openapi": "3.0.3""#), "{spec}");
    assert!(spec.contains(r#""http://127.0.0.1:8080""#), "{spec}");
    assert!(spec.contains(r#""/auth/login""#), "{spec}");
    assert!(spec.contains(r#""/api/me""#), "{spec}");
    assert!(spec.contains("LoginRequest"), "{spec}");
    assert!(spec.contains(r#""format": "email""#), "{spec}");
    assert!(spec.contains(r#""minLength": 6"#), "{spec}");
    assert!(spec.contains(r#""enum""#), "{spec}");
    assert!(spec.contains("TokenReply"), "{spec}");
    assert!(spec.contains("ValidationErrorResponse"), "{spec}");

    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let out_dir = std::env::temp_dir().join(format!("conduit_openapi_out_{}_{}", std::process::id(), n));
    fs::create_dir_all(&out_dir).unwrap();
    let spec_file = out_dir.join("openapi.json");
    let write_out = Command::new(conduit_bin())
        .args([
            "openapi",
            auth_main.to_str().unwrap(),
            "-o",
            spec_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(write_out.status.success());
    let written = fs::read_to_string(&spec_file).unwrap();
    assert!(written.contains(r#""openapi": "3.0.3""#));
    let _ = fs::remove_dir_all(&out_dir);

    let bad_router = Command::new(conduit_bin())
        .args(["routes", auth_main.to_str().unwrap(), "--router", "Unknown"])
        .output()
        .unwrap();
    assert!(!bad_router.status.success());
}

#[test]
fn cli_env_example_and_config_block() {
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir_path = std::env::temp_dir().join(format!("conduit_env_ex_{}_{}", std::process::id(), n));
    fs::create_dir_all(&dir_path).unwrap();
    let main_cdt = dir_path.join("app.cdt");
    fs::write(
        &main_cdt,
        r#"
import * as env from "cdt.env"

config AppConfig {
    port: Int = 8080
    debug: Bool @env("APP_DEBUG") = false
    api_key: String
    region: String? @env("APP_REGION")
}

func main() -> Void {
    switch env.load<AppConfig>() {
        case AppConfig(c): print("{c.port}:{c.debug}:{c.api_key}")
        case env.EnvError(e): print("err:{e.name}")
    }
}
"#,
    )
    .unwrap();

    let ex_out = Command::new(conduit_bin())
        .args(["env", "example", main_cdt.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        ex_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&ex_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&ex_out.stdout);
    assert!(stdout.contains("# AppConfig"), "{stdout}");
    assert!(stdout.contains("PORT=8080"), "{stdout}");
    assert!(stdout.contains("APP_DEBUG=false"), "{stdout}");
    assert!(stdout.contains("API_KEY= # required (String)"), "{stdout}");
    assert!(stdout.contains("# APP_REGION= # optional (String?)"), "{stdout}");

    let run_out = Command::new(conduit_bin())
        .args(["run", main_cdt.to_str().unwrap()])
        .env("API_KEY", "secret123")
        .env("APP_DEBUG", "true")
        .output()
        .unwrap();
    assert!(
        run_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run_out.stdout).trim(), "8080:true:secret123");
    let _ = fs::remove_dir_all(&dir_path);
}

#[test]
fn cli_top_level_job_blocks() {
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir_path = std::env::temp_dir().join(format!("conduit_job_e2e_{}_{}", std::process::id(), n));
    fs::create_dir_all(&dir_path).unwrap();
    let main_cdt = dir_path.join("jobs.cdt");
    fs::write(
        &main_cdt,
        r#"
job CleanupSessions every "*/5 * * * *" {
    print("cleanup_ok")
}

job Heartbeat interval 10s -> Void {
    print("beat_ok")
}

func main() -> Void {}
"#,
    )
    .unwrap();

    let list_out = Command::new(conduit_bin())
        .args(["cron", "list", main_cdt.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(list_out.status.success(), "stderr: {}", String::from_utf8_lossy(&list_out.stderr));
    let list_str = String::from_utf8_lossy(&list_out.stdout);
    assert!(list_str.contains("CleanupSessions -> every \"*/5 * * * *\""), "{list_str}");
    assert!(list_str.contains("Heartbeat -> interval 10000ms"), "{list_str}");

    let run_out = Command::new(conduit_bin())
        .args(["cron", "run", main_cdt.to_str().unwrap(), "CleanupSessions"])
        .output()
        .unwrap();
    assert!(run_out.status.success(), "stderr: {}", String::from_utf8_lossy(&run_out.stderr));
    assert_eq!(String::from_utf8_lossy(&run_out.stdout).trim(), "cleanup_ok");
    let _ = fs::remove_dir_all(&dir_path);
}

#[test]
fn cli_router_guard_typed_context() {
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir_path = std::env::temp_dir().join(format!("conduit_guard_e2e_{}_{}", std::process::id(), n));
    fs::create_dir_all(&dir_path).unwrap();
    let main_cdt = dir_path.join("api.cdt");
    fs::write(
        &main_cdt,
        r#"
import * as http from "cdt.http"

struct AuthUser {
    name: String
    role: String
}

func require_user(req: http.Request) -> AuthUser | http.Response {
    tok := req.header("authorization")
    if tok == "Bearer secret" {
        return AuthUser(name: "alice", role: "admin")
    }
    return http.status_json(401, AuthUser(name: "anon", role: "none"))
}

func get_me(req: http.Request, user: AuthUser) -> http.Response {
    return http.text("{user.name}:{user.role}")
}

func health(req: http.Request) -> http.Response {
    return http.text("ok")
}

router Api {
    guard require_user {
        get "/me" => get_me
    }
    get "/health" => health
}

func main() -> Void {
    r := http.build<Api>()
    empty_m := Map<String, String>()
    auth_m := Map.from([("authorization", "Bearer secret")])
    r1 := r.dispatch(http.Request(method: "GET", path: "/me", body: "", headers: empty_m, query: empty_m, params: empty_m))
    r2 := r.dispatch(http.Request(method: "GET", path: "/me", body: "", headers: auth_m, query: empty_m, params: empty_m))
    r3 := r.dispatch(http.Request(method: "GET", path: "/health", body: "", headers: empty_m, query: empty_m, params: empty_m))
    print("{r1.status}:{r2.status}:{r2.body}:{r3.status}:{r3.body}")
}
"#,
    )
    .unwrap();

    let run_out = Command::new(conduit_bin())
        .args(["run", main_cdt.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(run_out.status.success(), "stderr: {}", String::from_utf8_lossy(&run_out.stderr));
    assert_eq!(
        String::from_utf8_lossy(&run_out.stdout).trim(),
        "401:200:alice:admin:200:ok"
    );

    // Mismatched handler signature inside guard must fail check
    let bad_cdt = dir_path.join("bad_guard.cdt");
    fs::write(
        &bad_cdt,
        r#"
import * as http from "cdt.http"
struct AuthUser { name: String }
func require_user(req: http.Request) -> AuthUser | http.Response { return http.text(401, "no") }
func bad_handler(req: http.Request) -> http.Response { return http.text(200, "ok") }
router Api {
    guard require_user {
        get "/me" => bad_handler
    }
}
func main() -> Void { _ := http.build<Api>() }
"#,
    )
    .unwrap();
    let bad_out = Command::new(conduit_bin())
        .args(["check", bad_cdt.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!bad_out.status.success());
    let _ = fs::remove_dir_all(&dir_path);
}

#[test]
fn cli_http_typed_query_and_auto_responses() {
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir_path = std::env::temp_dir().join(format!("conduit_http_q_{}_{}", std::process::id(), n));
    fs::create_dir_all(&dir_path).unwrap();
    let main_cdt = dir_path.join("app.cdt");
    fs::write(
        &main_cdt,
        r#"
import * as http from "cdt.http"
import * as validation from "cdt.validation"

@http.status(404)
error NotFoundError {
    message: String,
}

@http.query
struct UserQuery {
    id: Int,
    page: Int = 1,
    @validation.range(1, 50)
    limit: Int = 10,
    q: String?,
}

struct UpdateBody {
    @validation.min_len(2)
    name: String,
}

struct UserOut {
    id: Int,
    page: Int,
    limit: Int,
    name: String,
}

func get_user(_req: http.Request, q: UserQuery) -> UserOut | NotFoundError {
    if q.id == 999 {
        return NotFoundError(message: "user not found")
    }
    label := q.q ?? "none"
    return UserOut(id: q.id, page: q.page, limit: q.limit, name: label)
}

func update_user(_req: http.Request, q: UserQuery, body: UpdateBody) -> UserOut | NotFoundError {
    if q.id == 999 {
        return NotFoundError(message: "user not found")
    }
    return UserOut(id: q.id, page: q.page, limit: q.limit, name: body.name)
}

func delete_user(req: http.Request) -> Void | NotFoundError {
    if (req.param("id") ?? "") == "999" {
        return NotFoundError(message: "user not found")
    }
    return
}

router Api {
    get "/users/:id" => get_user
    post "/users/:id" => update_user
    delete "/users/:id" => delete_user
}

func main() -> Void {
    r := http.build<Api>()
    empty_m := Map<String, String>()

    // 1. Defaults + path param -> 200 JSON
    r1 := r.dispatch(http.Request(method: "GET", path: "/users/42", body: "", headers: empty_m, query: empty_m, params: empty_m))
    // 2. Explicit query params -> 200 JSON
    q2 := Map.from([("page", "3"), ("limit", "25"), ("q", "alice")])
    r2 := r.dispatch(http.Request(method: "GET", path: "/users/42", body: "", headers: empty_m, query: q2, params: empty_m))
    // 3. Domain error -> 404 JSON
    r3 := r.dispatch(http.Request(method: "GET", path: "/users/999", body: "", headers: empty_m, query: empty_m, params: empty_m))
    // 4. Validation failure on query -> 422
    q4 := Map.from([("limit", "99")])
    r4 := r.dispatch(http.Request(method: "GET", path: "/users/42", body: "", headers: empty_m, query: q4, params: empty_m))
    // 5. Invalid int in path param -> 400
    r5 := r.dispatch(http.Request(method: "GET", path: "/users/abc", body: "", headers: empty_m, query: empty_m, params: empty_m))
    // 6. Query + Body together -> 200
    q6 := Map.from([("limit", "5")])
    r6 := r.dispatch(http.Request(method: "POST", path: "/users/7", body: "{\"name\":\"bob\"}", headers: empty_m, query: q6, params: empty_m))
    // 7. Void | Error -> 204
    r7 := r.dispatch(http.Request(method: "DELETE", path: "/users/7", body: "", headers: empty_m, query: empty_m, params: empty_m))

    print("{r1.status}|{r1.body}|{r2.status}|{r2.body}|{r3.status}|{r3.body}|{r4.status}|{r5.status}|{r6.status}|{r6.body}|{r7.status}")
}
"#,
    )
    .unwrap();

    let run_out = Command::new(conduit_bin())
        .args(["run", main_cdt.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        run_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&run_out.stdout);
    let out = stdout.trim();
    assert_eq!(
        out,
        r#"200|{"id":42,"page":1,"limit":10,"name":"none"}|200|{"id":42,"page":3,"limit":25,"name":"alice"}|404|{"message":"user not found"}|422|400|200|{"id":7,"page":1,"limit":5,"name":"bob"}|204"#
    );

    let openapi_out = Command::new(conduit_bin())
        .args(["openapi", main_cdt.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        openapi_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&openapi_out.stderr)
    );
    let spec = String::from_utf8_lossy(&openapi_out.stdout);
    assert!(spec.contains(r#""in": "query""#), "{spec}");
    assert!(spec.contains(r#""in": "path""#), "{spec}");
    assert!(spec.contains(r#""type": "integer""#), "{spec}");
    assert!(spec.contains(r#""404""#), "{spec}");
    assert!(spec.contains(r#""204""#), "{spec}");
    let _ = fs::remove_dir_all(&dir_path);
}

#[test]
fn cli_http_client_pool_builder_e2e() {
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir_path = std::env::temp_dir().join(format!("conduit_http_cli_{}_{}", std::process::id(), n));
    fs::create_dir_all(&dir_path).unwrap();
    let main_cdt = dir_path.join("client.cdt");
    fs::write(
        &main_cdt,
        r#"
import * as http from "cdt.http"

func main() -> Void {
    c := http.client("http://127.0.0.1:1")
        .bearer("tok_123")
        .header("X-App", "conduit")
        .timeout_ms(200)
        .base_url("invalid://bad")

    stats := c.pool_stats()
    idle_before := stats.get("idle") ?? -1
    res := c.get("/health")
    var err_kind = ""
    switch res {
        case http.HttpError(e): err_kind = e.kind
        case http.Response(_): err_kind = "ok"
    }
    c.close()
    print("{idle_before}:{err_kind}")
}
"#,
    )
    .unwrap();

    let run_out = Command::new(conduit_bin())
        .args(["run", main_cdt.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        run_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run_out.stdout).trim(), "0:invalid_url");
    let _ = fs::remove_dir_all(&dir_path);
}

#[cfg(unix)]
#[test]
fn cli_http_signal_graceful_shutdown_and_lifecycle_e2e() {
    use std::net::{TcpListener, TcpStream};
    use std::io::{Read, Write};
    use std::process::Stdio;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir_path = std::env::temp_dir().join(format!("conduit_sig_srv_{}_{}", std::process::id(), n));
    fs::create_dir_all(&dir_path).unwrap();
    let main_cdt = dir_path.join("srv.cdt");
    fs::write(
        &main_cdt,
        format!(
            r#"
import * as http from "cdt.http"
import * as time from "cdt.time"

on start {{
    print("top_start")
}}

on stop {{
    print("top_stop")
}}

func r_start() -> Void {{
    print("router_start")
}}

func r_stop() -> Void {{
    print("router_stop")
}}

func ping(_req: http.Request) -> http.Response {{
    spawn {{
        _s := time.sleep(30)
        print("spawn_done")
    }}
    return http.text("pong")
}}

router Api {{
    on start => r_start
    on stop => r_stop
    get "/ping" => ping
}}

func main() -> Void {{
    _srv := http.serve<Api>("127.0.0.1:{port}")
}}
"#
        ),
    )
    .unwrap();

    let child = Command::new(conduit_bin())
        .args(["run", main_cdt.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let addr = format!("127.0.0.1:{port}");
    let mut got_pong = false;
    for _ in 0..50 {
        if let Ok(mut stream) = TcpStream::connect(&addr) {
            let _ = stream.write_all(b"GET /ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
            let mut resp = String::new();
            let _ = stream.read_to_string(&mut resp);
            if resp.contains("pong") {
                got_pong = true;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(got_pong, "server did not respond with pong on {addr}");

    let kill_status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(kill_status.success());

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec![
            "top_start",
            "router_start",
            "spawn_done",
            "router_stop",
            "top_stop",
        ],
        "stdout was:\n{stdout}"
    );
    let _ = fs::remove_dir_all(&dir_path);
}
