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
