import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import crypto from "node:crypto";
import { spawnSync } from "node:child_process";

const ms = (t0) => Number(process.hrtime.bigint() - t0) / 1e6;

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
