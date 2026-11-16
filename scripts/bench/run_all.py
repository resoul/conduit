from __future__ import annotations

import os
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile

METRICS = [
    ("fs_write_text", "fs.write_text (5.53 MiB)"),
    ("fs_write_lines", "Writer.write_line (100k)"),
    ("fs_read_text", "fs.read_text (5.53 MiB)"),
    ("fs_read_lines", "fs.read_lines (100k)"),
    ("fs_each_line", "fs.each_line (100k)"),
    ("fs_read_chunk64k", "Reader.read_chunk (64K)"),
    ("crypto_sha256_50m", "crypto.sha256 (50x1 MiB)"),
    ("crypto_b64_enc_50m", "crypto.base64_encode (50x1 MiB)"),
    ("crypto_b64_dec_50m", "crypto.base64_decode (50x1 MiB)"),
    ("crypto_uuid_10k", "crypto.uuid_v4 (10k)"),
    ("path_join_norm_100k", "path.join + normalize (100k)"),
    ("process_run_50x", "process.run (50x)"),
    ("json_encode_10k", "json.encode (10k structs)"),
    ("json_decode_10k", "json.decode (10k structs)"),
    ("pipeline_1m", "pipeline filter/map/reduce (1M)"),
    ("pipeline_float_1m", "pipeline float filter/map/reduce (1M)"),
    ("csv_parse_20k", "csv.parse (20k rows)"),
    ("url_parse_50k", "url.parse + query (50k)"),
]


def parse_output(text: str) -> tuple[dict[str, float], str | None]:
    res: dict[str, float] = {}
    check: str | None = None
    for line in text.splitlines():
        if line.startswith("BENCH|"):
            parts = line.split("|")
            if len(parts) == 3:
                try:
                    res[parts[1]] = float(parts[2])
                except ValueError:
                    pass
        elif line.startswith("CHECK|"):
            check = line.split("|", 1)[1].strip()
    return res, check


def run_cmd(cmd: list[str], cwd: str) -> tuple[dict[str, float], str | None]:
    proc = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
    if proc.returncode != 0:
        print(f"Command failed ({' '.join(cmd)}):\n{proc.stderr}", file=sys.stderr)
        return {}, None
    return parse_output(proc.stdout)


def main() -> None:
    repo_root = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
    bench_dir = os.path.dirname(os.path.abspath(__file__))
    exe = "conduit.exe" if os.name == "nt" else "conduit"
    conduit_bin = os.environ.get(
        "CONDUIT_BIN", os.path.join(repo_root, "target", "release", exe)
    )

    runners: list[tuple[str, list[str]]] = []
    temp_artifacts: list[str] = []
    if os.path.exists(conduit_bin):
        runners.append(("Conduit", [conduit_bin, "run", os.path.join(bench_dir, "bench.cdt")]))
    go_bin = shutil.which("go")
    if go_bin:
        runners.append(("Go", [go_bin, "run", os.path.join(bench_dir, "bench.go")]))
    swiftc_bin = shutil.which("swiftc")
    if swiftc_bin:
        swift_bin = os.path.join(tempfile.gettempdir(), "conduit_bench_swift" + (".exe" if os.name == "nt" else ""))
        swift_cache = os.path.join(tempfile.gettempdir(), "swift_mod_cache")
        comp = subprocess.run([swiftc_bin, "-O", "-module-cache-path", swift_cache, os.path.join(bench_dir, "bench.swift"), "-o", swift_bin], capture_output=True, text=True)
        if comp.returncode == 0:
            runners.append(("Swift", [swift_bin]))
            temp_artifacts.append(swift_bin)
        else:
            print(f"Swift compile skipped: {comp.stderr}", file=sys.stderr)
    kotlinc_bin = shutil.which("kotlinc")
    java_bin = shutil.which("java")
    if kotlinc_bin and java_bin:
        kt_jar = os.path.join(tempfile.gettempdir(), "conduit_bench_kt.jar")
        kt_cmd = [kotlinc_bin, os.path.join(bench_dir, "bench.kt"), "-include-runtime", "-d", kt_jar]
        if os.name == "nt" and kotlinc_bin.lower().endswith((".bat", ".cmd")):
            kt_cmd = ["cmd", "/c", *kt_cmd]
        comp = subprocess.run(kt_cmd, capture_output=True, text=True)
        if comp.returncode == 0:
            runners.append(("Kotlin", [java_bin, "-Xmx1g", "-jar", kt_jar]))
            temp_artifacts.append(kt_jar)
        else:
            print(f"Kotlin compile skipped: {comp.stderr}", file=sys.stderr)
    bun_bin = shutil.which("bun")
    if bun_bin:
        runners.append(("Bun", [bun_bin, os.path.join(bench_dir, "bench.mjs")]))
    node_bin = shutil.which("node")
    if node_bin:
        runners.append(("Node.js", [node_bin, os.path.join(bench_dir, "bench.mjs")]))
    php_bin = shutil.which("php")
    if php_bin:
        runners.append(("PHP", [php_bin, os.path.join(bench_dir, "bench.php")]))
    py = sys.executable or shutil.which("python3") or shutil.which("python")
    if py:
        runners.append(("Python 3", [py, os.path.join(bench_dir, "bench.py")]))

    runs = int(os.environ.get("BENCH_RUNS", "5"))
    results: dict[str, dict[str, float]] = {}
    checks: dict[str, str] = {}
    for name, cmd in runners:
        print(f"Running {name} ({runs}x median)...", flush=True)
        samples: dict[str, list[float]] = {}
        for _ in range(runs):
            out, chk = run_cmd(cmd, bench_dir)
            for k, v in out.items():
                samples.setdefault(k, []).append(v)
            if chk is not None:
                if name in checks and checks[name] != chk:
                    print(f"ERROR: {name} CHECK mismatch across runs: {checks[name]} vs {chk}", file=sys.stderr)
                    sys.exit(1)
                checks[name] = chk
        results[name] = {k: statistics.median(vs) for k, vs in samples.items()}
    for art in temp_artifacts:
        try:
            os.remove(art)
        except OSError:
            pass

    distinct_checks = set(checks.values())
    if len(distinct_checks) > 1:
        print(f"ERROR: CHECK mismatch across languages: {checks}", file=sys.stderr)
        sys.exit(1)

    names = [name for name, _ in runners]
    lines = [
        f"### Benchmark Results ({platform.system()} / {platform.machine()})",
        "",
        "| Operation | " + " | ".join(names) + " |",
        "| :--- | " + " | ".join(["---:"] * len(names)) + " |",
    ]
    for key, label in METRICS:
        row = [label]
        for name in names:
            val = results.get(name, {}).get(key)
            row.append(f"{val:.2f} ms" if val is not None else "n/a")
        lines.append("| " + " | ".join(row) + " |")

    report = "\n".join(lines) + "\n"
    print("\n" + report)

    summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary_path:
        with open(summary_path, "a", encoding="utf-8") as f:
            f.write(report + "\n")


if __name__ == "__main__":
    main()
