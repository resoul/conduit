import base64, hashlib, json, os, shutil, subprocess, tempfile, time, uuid

# 1. FS I/O
base = os.path.join(tempfile.gettempdir(), f"_py_bench_{os.getpid()}")
os.makedirs(base, exist_ok=True)
p = os.path.join(base, "data.txt")
p_bulk = os.path.join(base, "bulk.txt")
line = "line payload number 0123456789 abcdefghijklmnopqrstuvwxyz\n"

t0 = time.perf_counter()
with open(p, "w", buffering=65536, encoding="utf-8") as f:
    for _ in range(100000):
        f.write(line)
ms_write_stream = (time.perf_counter() - t0) * 1000.0

with open(p, "r", encoding="utf-8") as f:
    data = f.read()
with open(p, "r", encoding="utf-8") as f:
    _ = f.read()

t0 = time.perf_counter()
with open(p_bulk, "w", encoding="utf-8") as f:
    f.write(data)
ms_write_text = (time.perf_counter() - t0) * 1000.0

t0 = time.perf_counter()
with open(p, "r", encoding="utf-8") as f:
    _ = f.read()
ms_read_text = (time.perf_counter() - t0) * 1000.0

t0 = time.perf_counter()
with open(p, "r", encoding="utf-8") as f:
    _ = f.read().splitlines()
ms_read_lines = (time.perf_counter() - t0) * 1000.0

t0 = time.perf_counter()
with open(p, "r", buffering=65536, encoding="utf-8") as f:
    total = 0
    for ln in f:
        total += len(ln)
ms_each_line = (time.perf_counter() - t0) * 1000.0

t0 = time.perf_counter()
with open(p, "r", buffering=65536, encoding="utf-8") as f:
    while f.read(65536):
        pass
ms_read_chunk = (time.perf_counter() - t0) * 1000.0
shutil.rmtree(base, ignore_errors=True)

# 2. Crypto, Path, Process
big = (b"0123456789abcdef" * 4) * (2 ** 14)
hashlib.sha256(big).hexdigest()
base64.b64encode(big)

t0 = time.perf_counter()
for _ in range(50):
    hashlib.sha256(big).hexdigest()
ms_sha = (time.perf_counter() - t0) * 1000.0

t0 = time.perf_counter()
for _ in range(50):
    b64 = base64.b64encode(big).decode("ascii")
ms_b64e = (time.perf_counter() - t0) * 1000.0

t0 = time.perf_counter()
for _ in range(50):
    base64.b64decode(b64).decode("utf-8")
ms_b64d = (time.perf_counter() - t0) * 1000.0

t0 = time.perf_counter()
for _ in range(10000):
    str(uuid.uuid4())
ms_uuid = (time.perf_counter() - t0) * 1000.0

t0 = time.perf_counter()
for _ in range(100000):
    os.path.normpath(os.path.join("/var/log", "app/../service/output.log"))
ms_path = (time.perf_counter() - t0) * 1000.0

cmd = ["cmd", "/c", "exit", "0"] if os.name == "nt" else ["true"]
t0 = time.perf_counter()
for _ in range(50):
    subprocess.run(cmd, capture_output=True)
ms_proc = (time.perf_counter() - t0) * 1000.0

# 3. JSON + Pipeline
items = [{"id": i, "user": "alice", "score": i % 100} for i in range(10000)]

t0 = time.perf_counter()
raw_json = json.dumps(items)
ms_json_enc = (time.perf_counter() - t0) * 1000.0

t0 = time.perf_counter()
decoded = json.loads(raw_json)
ms_json_dec = (time.perf_counter() - t0) * 1000.0

nums = list(range(1000000))
t0 = time.perf_counter()
s = sum(x * 3 + 1 for x in nums if x % 2 == 0)
ms_pipe = (time.perf_counter() - t0) * 1000.0

print(f"BENCH|fs_write_text|{ms_write_text:.3f}")
print(f"BENCH|fs_write_lines|{ms_write_stream:.3f}")
print(f"BENCH|fs_read_text|{ms_read_text:.3f}")
print(f"BENCH|fs_read_lines|{ms_read_lines:.3f}")
print(f"BENCH|fs_each_line|{ms_each_line:.3f}")
print(f"BENCH|fs_read_chunk64k|{ms_read_chunk:.3f}")
print(f"BENCH|crypto_sha256_50m|{ms_sha:.3f}")
print(f"BENCH|crypto_b64_enc_50m|{ms_b64e:.3f}")
print(f"BENCH|crypto_b64_dec_50m|{ms_b64d:.3f}")
print(f"BENCH|crypto_uuid_10k|{ms_uuid:.3f}")
print(f"BENCH|path_join_norm_100k|{ms_path:.3f}")
print(f"BENCH|process_run_50x|{ms_proc:.3f}")
print(f"BENCH|json_encode_10k|{ms_json_enc:.3f}")
print(f"BENCH|json_decode_10k|{ms_json_dec:.3f}")
print(f"BENCH|pipeline_1m|{ms_pipe:.3f}")
