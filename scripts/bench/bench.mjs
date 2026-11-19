import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import crypto from "node:crypto";
import { spawnSync } from "node:child_process";

const ms = (t0) => Number(process.hrtime.bigint() - t0) / 1e6;

const vmFib = (n) => (n < 2 ? n : vmFib(n - 1) + vmFib(n - 2));
let tVm = process.hrtime.bigint();
if (vmFib(35) !== 9227465) process.exit(1);
const msVmFib = ms(tVm);

tVm = process.hrtime.bigint();
let arithAcc = 0;
for (let i = 0; i < 10000000; i++) {
  arithAcc = (arithAcc * 1664525 + 1013904223 + i) % 1000000007;
}
const msVmArith = ms(tVm);
if (arithAcc < 0) process.exit(1);

tVm = process.hrtime.bigint();
const vmM = new Map();
for (let i = 0; i < 100000; i++) vmM.set(i, i * 3 + 1);
let mapAcc = 0;
for (let i = 0; i < 100000; i++) mapAcc += vmM.get(i);
const msVmMap = ms(tVm);
if (mapAcc < 0) process.exit(1);

tVm = process.hrtime.bigint();
let adtAcc = 0;
for (let i = 0; i < 1000000; i++) {
  const r = i & 3;
  const ev =
    r === 0
      ? { type: 0 }
      : r === 1
      ? { type: 1, a: i, b: 7 }
      : r === 2
      ? { type: 2, v: i, f: 3 }
      : { type: 1, a: i, b: -2 };
  switch (ev.type) {
    case 0:
      adtAcc += 1;
      break;
    case 1:
      adtAcc += ev.a + ev.b;
      break;
    case 2:
      adtAcc += ev.v * ev.f;
      break;
  }
}
const msAdt = ms(tVm);
if (adtAcc !== 625001500000) process.exit(1);

// 1. FS I/O
const base = path.join(os.tmpdir(), `_js_bench_${process.pid}`);
fs.mkdirSync(base, { recursive: true });
const p = path.join(base, "data.txt");
const pBulk = path.join(base, "bulk.txt");
const line = "line payload number 0123456789 abcdefghijklmnopqrstuvwxyz\n";

let t0 = process.hrtime.bigint();
const fdW = fs.openSync(p, "w");
const bigChunk = line.repeat(1000);
for (let i = 0; i < 100; i++) fs.writeSync(fdW, bigChunk);
fs.closeSync(fdW);
// Fair line-by-line buffered write simulation using array buffer
let tWs = process.hrtime.bigint();
const fdW2 = fs.openSync(p, "w");
let bufStr = "";
for (let i = 0; i < 100000; i++) {
  bufStr += line;
  if (bufStr.length >= 65536) {
    fs.writeSync(fdW2, bufStr);
    bufStr = "";
  }
}
if (bufStr.length > 0) fs.writeSync(fdW2, bufStr);
fs.closeSync(fdW2);
const msWriteStream = ms(tWs);

const data = fs.readFileSync(p, "utf8");
fs.readFileSync(p, "utf8");

t0 = process.hrtime.bigint();
fs.writeFileSync(pBulk, data, "utf8");
const msWriteText = ms(t0);

t0 = process.hrtime.bigint();
const rt = fs.readFileSync(p, "utf8");
const msReadText = ms(t0);

t0 = process.hrtime.bigint();
const rl = fs.readFileSync(p, "utf8").split(/\r?\n/);
const msReadLines = ms(t0);

const buf64 = Buffer.allocUnsafe(65536);
t0 = process.hrtime.bigint();
const fdL = fs.openSync(p, "r");
let rem = "";
let total = 0;
while (true) {
  const n = fs.readSync(fdL, buf64, 0, 65536, null);
  if (n === 0) break;
  const chunk = rem + buf64.toString("utf8", 0, n);
  const parts = chunk.split("\n");
  rem = parts.pop();
  for (let i = 0; i < parts.length; i++) total += parts[i].length;
}
fs.closeSync(fdL);
const msEachLine = ms(t0);

t0 = process.hrtime.bigint();
const fdC = fs.openSync(p, "r");
while (fs.readSync(fdC, buf64, 0, 65536, null) > 0) {}
fs.closeSync(fdC);
const msReadChunk = ms(t0);
fs.rmSync(base, { recursive: true, force: true });

// 2. Crypto, Path, Process
const big = "0123456789abcdef".repeat(4 * 16384);
crypto.createHash("sha256").update(big).digest("hex");
Buffer.from(big).toString("base64");

t0 = process.hrtime.bigint();
for (let i = 0; i < 50; i++) crypto.createHash("sha256").update(big).digest("hex");
const msSha = ms(t0);

let b64 = "";
t0 = process.hrtime.bigint();
for (let i = 0; i < 50; i++) b64 = Buffer.from(big, "utf8").toString("base64");
const msB64e = ms(t0);

t0 = process.hrtime.bigint();
for (let i = 0; i < 50; i++) Buffer.from(b64, "base64").toString("utf8");
const msB64d = ms(t0);

t0 = process.hrtime.bigint();
for (let i = 0; i < 10000; i++) crypto.randomUUID();
const msUuid = ms(t0);

t0 = process.hrtime.bigint();
for (let i = 0; i < 100000; i++) path.normalize(path.join("/var/log", "app/../service/output.log"));
const msPath = ms(t0);

const isWin = os.platform() === "win32";
t0 = process.hrtime.bigint();
for (let i = 0; i < 50; i++) {
  if (isWin) spawnSync("cmd", ["/c", "exit", "0"]);
  else spawnSync("true", []);
}
const msProc = ms(t0);

// 3. JSON + Pipeline
const items = [];
for (let i = 0; i < 10000; i++) items.push({ id: i, user: "alice", score: i % 100 });

t0 = process.hrtime.bigint();
const rawJson = JSON.stringify(items);
const msJsonEnc = ms(t0);

t0 = process.hrtime.bigint();
const decoded = JSON.parse(rawJson);
const msJsonDec = ms(t0);

const nums = new Array(1000000);
for (let i = 0; i < 1000000; i++) nums[i] = i;

t0 = process.hrtime.bigint();
const sum = nums
  .filter((x) => x % 2 === 0)
  .map((x) => x * 3 + 1)
  .reduce((acc, x) => acc + x, 0);
const msPipe = ms(t0);
if (sum < 0 || decoded.length < 0 || rt.length < 0 || rl.length < 0 || total < 0) process.exit(1);
console.log(`CHECK|${sum}`);

const fnums = Array.from({ length: 1000000 }, (_, i) => i * 1.0);
t0 = process.hrtime.bigint();
const fsum = fnums
  .filter((x) => x > 250000.0)
  .map((x) => x * 1.5 + 0.25)
  .reduce((acc, x) => acc + x, 0.0);
const msFPipe = ms(t0);
if (fsum < 0) process.exit(1);

const sampleUrl = "https://api.example.com:8443/v1/items?tag=prod&limit=50&offset=100#top";
t0 = process.hrtime.bigint();
let urlAcc = 0;
for (let i = 0; i < 50000; i++) {
  const u = new URL(sampleUrl);
  let n = 0;
  for (const _ of u.searchParams) n++;
  urlAcc += n;
}
const msUrl = ms(t0);
if (urlAcc < 0) process.exit(1);

const parsePat = (pat) => {
  const tr = pat.replace(/^\/+|\/+$/g, "");
  if (!tr) return [[], false];
  let wc = false;
  const segs = tr.split("/").map((s) => {
    if (s[0] === ":") return [1, s.slice(1)];
    if (s[0] === "*") { wc = true; return [2, s.slice(1)]; }
    return [0, s];
  });
  return [segs, wc];
};
const routes = [
  ["GET", ...parsePat("/"), () => [200, "ok"]],
  ["GET", ...parsePat("/health"), () => [200, "ok"]],
  ["GET", ...parsePat("/api/v1/items"), () => [200, "items"]],
  ["GET", ...parsePat("/api/v1/users/:id/orders"), (p) => [200, p.id ?? ""]],
  ["GET", ...parsePat("/files/*rest"), () => [200, "file"]],
  ["POST", ...parsePat("/api/v1/echo"), () => [200, ""]],
];
const samplePaths = ["/", "/health", "/api/v1/users/42/orders", "/files/docs/readme.md"];
t0 = process.hrtime.bigint();
let httpAcc = 0;
for (let i = 0; i < 50000; i++) {
  const tr = samplePaths[i & 3].replace(/^\/+|\/+$/g, "");
  const pathSegs = tr ? tr.split("/") : [];
  const nSegs = pathSegs.length;
  let st = 404;
  let body = "";
  for (const [rm, segs, wc, h] of routes) {
    if (!wc && segs.length !== nSegs) continue;
    const params = {};
    let ok = true;
    for (let idx = 0; idx < segs.length; idx++) {
      const [kind, val] = segs[idx];
      if (kind === 2) { params[val] = pathSegs.slice(idx).join("/"); break; }
      if (kind === 1) {
        if (idx >= nSegs || !pathSegs[idx]) { ok = false; break; }
        params[val] = pathSegs[idx];
      } else if (idx >= nSegs || pathSegs[idx] !== val) { ok = false; break; }
    }
    if (ok && rm === "GET") {
      [st, body] = h(params);
      break;
    }
  }
  httpAcc += st + body.length;
}
const msHttp = ms(t0);
if (httpAcc !== 10125000) process.exit(1);

const jwtSecret = "conduit_bench_jwt_secret_key_32b";
const jwtHdr = Buffer.from('{"alg":"HS256","typ":"JWT"}').toString("base64url");
t0 = process.hrtime.bigint();
let jwtAcc = 0;
for (let i = 0; i < 10000; i++) {
  const payB64 = Buffer.from(JSON.stringify({ sub: "user", uid: i })).toString("base64url");
  const sigInput = `${jwtHdr}.${payB64}`;
  const sigB64 = crypto.createHmac("sha256", jwtSecret).update(sigInput).digest("base64url");
  const tok = `${sigInput}.${sigB64}`;

  const parts = tok.split(".");
  if (parts.length !== 3) process.exit(1);
  const expectedSig = crypto
    .createHmac("sha256", jwtSecret)
    .update(`${parts[0]}.${parts[1]}`)
    .digest("base64url");
  if (
    parts[2].length !== expectedSig.length ||
    !crypto.timingSafeEqual(Buffer.from(parts[2]), Buffer.from(expectedSig))
  ) {
    process.exit(1);
  }
  const cl = JSON.parse(Buffer.from(parts[1], "base64url").toString("utf8"));
  jwtAcc += cl.uid;
}
const msJwt = ms(t0);
if (jwtAcc !== 49995000) process.exit(1);

const lru = new Map();
const lruSet = (k, v) => {
  if (lru.has(k)) {
    lru.delete(k);
  } else if (lru.size >= 4096) {
    const oldest = lru.keys().next().value;
    lru.delete(oldest);
  }
  lru.set(k, v);
};
const lruGet = (k) => {
  if (!lru.has(k)) return undefined;
  const v = lru.get(k);
  lru.delete(k);
  lru.set(k, v);
  return v;
};
t0 = process.hrtime.bigint();
let cacheAcc = 0;
for (let i = 0; i < 100000; i++) {
  lruSet(`k${i & 8191}`, "v");
  if (lruGet(`k${(i + 6144) & 8191}`) !== undefined) {
    cacheAcc++;
  }
}
const msCache = ms(t0);
if (cacheAcc !== 97905) process.exit(1);

console.log(`BENCH|vm_fib_35|${msVmFib.toFixed(3)}`);
console.log(`BENCH|vm_arith_10m|${msVmArith.toFixed(3)}`);
console.log(`BENCH|vm_map_100k|${msVmMap.toFixed(3)}`);
console.log(`BENCH|adt_switch_1m|${msAdt.toFixed(3)}`);

console.log(`BENCH|fs_write_text|${msWriteText.toFixed(3)}`);
console.log(`BENCH|fs_write_lines|${msWriteStream.toFixed(3)}`);
console.log(`BENCH|fs_read_text|${msReadText.toFixed(3)}`);
console.log(`BENCH|fs_read_lines|${msReadLines.toFixed(3)}`);
console.log(`BENCH|fs_each_line|${msEachLine.toFixed(3)}`);
console.log(`BENCH|fs_read_chunk64k|${msReadChunk.toFixed(3)}`);
console.log(`BENCH|crypto_sha256_50m|${msSha.toFixed(3)}`);
console.log(`BENCH|crypto_b64_enc_50m|${msB64e.toFixed(3)}`);
console.log(`BENCH|crypto_b64_dec_50m|${msB64d.toFixed(3)}`);
console.log(`BENCH|crypto_uuid_10k|${msUuid.toFixed(3)}`);
console.log(`BENCH|path_join_norm_100k|${msPath.toFixed(3)}`);
console.log(`BENCH|process_run_50x|${msProc.toFixed(3)}`);
console.log(`BENCH|json_encode_10k|${msJsonEnc.toFixed(3)}`);
console.log(`BENCH|json_decode_10k|${msJsonDec.toFixed(3)}`);
console.log(`BENCH|pipeline_1m|${msPipe.toFixed(3)}`);
console.log(`BENCH|pipeline_float_1m|${msFPipe.toFixed(3)}`);
console.log(`BENCH|url_parse_50k|${msUrl.toFixed(3)}`);
console.log(`BENCH|http_router_50k|${msHttp.toFixed(3)}`);
console.log(`BENCH|jwt_hs256_10k|${msJwt.toFixed(3)}`);
console.log(`BENCH|cache_lru_100k|${msCache.toFixed(3)}`);
