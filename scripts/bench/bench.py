import base64, collections, csv, hashlib, hmac, io, json, os, shutil, subprocess, tempfile, time, urllib.parse, uuid

def vm_fib(n):
    return n if n < 2 else vm_fib(n - 1) + vm_fib(n - 2)

t0 = time.perf_counter()
if vm_fib(35) != 9227465:
    raise RuntimeError("unreachable")
ms_vm_fib = (time.perf_counter() - t0) * 1000.0

t0 = time.perf_counter()
arith_acc = 0
for i in range(10000000):
    arith_acc = (arith_acc * 1664525 + 1013904223 + i) % 1000000007
ms_vm_arith = (time.perf_counter() - t0) * 1000.0
if arith_acc < 0:
    raise RuntimeError("unreachable")

t0 = time.perf_counter()
vm_m = {}
for i in range(100000):
    vm_m[i] = i * 3 + 1
map_acc = 0
for i in range(100000):
    map_acc += vm_m[i]
ms_vm_map = (time.perf_counter() - t0) * 1000.0
if map_acc < 0:
    raise RuntimeError("unreachable")

t0 = time.perf_counter()
adt_acc = 0
for i in range(1000000):
    r = i & 3
    if r == 0:
        ev = (0,)
    elif r == 1:
        ev = (1, i, 7)
    elif r == 2:
        ev = (2, i, 3)
    else:
        ev = (1, i, -2)
    t_kind = ev[0]
    if t_kind == 0:
        adt_acc += 1
    elif t_kind == 1:
        adt_acc += ev[1] + ev[2]
    else:
        adt_acc += ev[1] * ev[2]
ms_adt = (time.perf_counter() - t0) * 1000.0
if adt_acc != 625001500000:
    raise RuntimeError("unreachable")

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
print(f"CHECK|{s}")

fnums = [float(i) for i in range(1000000)]
t0 = time.perf_counter()
fs = sum(x * 1.5 + 0.25 for x in fnums if x > 250000.0)
ms_fpipe = (time.perf_counter() - t0) * 1000.0
if fs < 0:
    raise RuntimeError("unreachable")

csv_raw = "\n".join(["id,name,note"] + [f'{i},"user, #{i}","ok ""v1"""' for i in range(20000)])
t0 = time.perf_counter()
csv_rows = list(csv.reader(io.StringIO(csv_raw)))
ms_csv = (time.perf_counter() - t0) * 1000.0
if len(csv_rows) < 0:
    raise RuntimeError("unreachable")

sample_url = "https://api.example.com:8443/v1/items?tag=prod&limit=50&offset=100#top"
t0 = time.perf_counter()
url_acc = 0
for _ in range(50000):
    u = urllib.parse.urlparse(sample_url)
    q = urllib.parse.parse_qs(u.query)
    url_acc += len(q)
ms_url = (time.perf_counter() - t0) * 1000.0
if url_acc < 0:
    raise RuntimeError("unreachable")

def parse_pat(pat):
    tr = pat.strip("/")
    if not tr:
        return [], False
    segs = []
    wc = False
    for s in tr.split("/"):
        if s.startswith(":"):
            segs.append((1, s[1:]))
        elif s.startswith("*"):
            segs.append((2, s[1:]))
            wc = True
        else:
            segs.append((0, s))
    return segs, wc

routes = [
    ("GET", *parse_pat("/"), lambda p: (200, "ok")),
    ("GET", *parse_pat("/health"), lambda p: (200, "ok")),
    ("GET", *parse_pat("/api/v1/items"), lambda p: (200, "items")),
    ("GET", *parse_pat("/api/v1/users/:id/orders"), lambda p: (200, p.get("id", ""))),
    ("GET", *parse_pat("/files/*rest"), lambda p: (200, "file")),
    ("POST", *parse_pat("/api/v1/echo"), lambda p: (200, "")),
]
sample_paths = ["/", "/health", "/api/v1/users/42/orders", "/files/docs/readme.md"]
t0 = time.perf_counter()
http_acc = 0
for i in range(50000):
    tr = sample_paths[i & 3].strip("/")
    path_segs = tr.split("/") if tr else []
    n_segs = len(path_segs)
    st, body = 404, ""
    for rm, segs, wc, h in routes:
        if not wc and len(segs) != n_segs:
            continue
        params = {}
        ok = True
        for idx, (kind, val) in enumerate(segs):
            if kind == 2:
                params[val] = "/".join(path_segs[idx:])
                break
            elif kind == 1:
                if idx >= n_segs or not path_segs[idx]:
                    ok = False
                    break
                params[val] = path_segs[idx]
            elif idx >= n_segs or path_segs[idx] != val:
                ok = False
                break
        if ok and rm == "GET":
            st, body = h(params)
            break
    http_acc += st + len(body)
ms_http = (time.perf_counter() - t0) * 1000.0
if http_acc != 10125000:
    raise RuntimeError("unreachable")

def b64url_enc(b: bytes) -> str:
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode("ascii")

def b64url_dec(s: str) -> bytes:
    pad = "=" * ((4 - len(s) % 4) % 4)
    return base64.urlsafe_b64decode(s + pad)

jwt_secret = b"conduit_bench_jwt_secret_key_32b"
jwt_hdr = b64url_enc(b'{"alg":"HS256","typ":"JWT"}')
t0 = time.perf_counter()
jwt_acc = 0
for i in range(10000):
    pay_b64 = b64url_enc(json.dumps({"sub": "user", "uid": i}, separators=(",", ":")).encode("utf-8"))
    sig_input = f"{jwt_hdr}.{pay_b64}"
    sig_b64 = b64url_enc(hmac.new(jwt_secret, sig_input.encode("ascii"), hashlib.sha256).digest())
    tok = f"{sig_input}.{sig_b64}"

    parts = tok.split(".")
    if len(parts) != 3:
        raise RuntimeError("unreachable")
    expected_sig = b64url_enc(hmac.new(jwt_secret, f"{parts[0]}.{parts[1]}".encode("ascii"), hashlib.sha256).digest())
    if not hmac.compare_digest(parts[2], expected_sig):
        raise RuntimeError("unreachable")
    cl = json.loads(b64url_dec(parts[1]))
    jwt_acc += cl["uid"]
ms_jwt = (time.perf_counter() - t0) * 1000.0
if jwt_acc != 49995000:
    raise RuntimeError("unreachable")

lru = collections.OrderedDict()
t0 = time.perf_counter()
cache_acc = 0
for i in range(100000):
    ks = f"k{i & 8191}"
    if ks in lru:
        lru.move_to_end(ks)
    elif len(lru) >= 4096:
        lru.popitem(last=False)
    lru[ks] = "v"

    kg = f"k{(i + 6144) & 8191}"
    if kg in lru:
        lru.move_to_end(kg)
        cache_acc += 1
ms_cache = (time.perf_counter() - t0) * 1000.0
if cache_acc != 97905:
    raise RuntimeError("unreachable")

print(f"BENCH|vm_fib_35|{ms_vm_fib:.3f}")
print(f"BENCH|vm_arith_10m|{ms_vm_arith:.3f}")
print(f"BENCH|vm_map_100k|{ms_vm_map:.3f}")
print(f"BENCH|adt_switch_1m|{ms_adt:.3f}")

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
print(f"BENCH|pipeline_float_1m|{ms_fpipe:.3f}")
print(f"BENCH|csv_parse_20k|{ms_csv:.3f}")
print(f"BENCH|url_parse_50k|{ms_url:.3f}")
print(f"BENCH|http_router_50k|{ms_http:.3f}")
print(f"BENCH|jwt_hs256_10k|{ms_jwt:.3f}")
print(f"BENCH|cache_lru_100k|{ms_cache:.3f}")
