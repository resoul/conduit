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
    assert!(stdout.contains("true:CREATE TABLE IF NOT EXISTS accounts (id BIGSERIAL PRIMARY KEY, email TEXT NOT NULL UNIQUE)"));
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
