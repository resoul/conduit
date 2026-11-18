import Foundation
#if canImport(CryptoKit)
import CryptoKit
#endif

func msSince(_ t0: UInt64) -> Double {
    return Double(DispatchTime.now().uptimeNanoseconds - t0) / 1_000_000.0
}

func vmFib(_ n: Int) -> Int {
    return n < 2 ? n : vmFib(n - 1) + vmFib(n - 2)
}

var tVm = DispatchTime.now().uptimeNanoseconds
if vmFib(35) != 9227465 { exit(1) }
let msVmFib = msSince(tVm)

tVm = DispatchTime.now().uptimeNanoseconds
var arithAcc = 0
for i in 0..<10_000_000 {
    arithAcc = (arithAcc &* 1664525 &+ 1013904223 &+ i) % 1000000007
}
let msVmArith = msSince(tVm)
if arithAcc < 0 { exit(1) }

tVm = DispatchTime.now().uptimeNanoseconds
var vmM: [Int: Int] = Dictionary(minimumCapacity: 100_000)
for i in 0..<100_000 { vmM[i] = i * 3 + 1 }
var mapAcc = 0
for i in 0..<100_000 { mapAcc += vmM[i] ?? 0 }
let msVmMap = msSince(tVm)
if mapAcc < 0 { exit(1) }

struct Item: Codable {
    let id: Int
    let user: String
    let score: Int
}

// 1. FS I/O
let fm = FileManager.default
let base = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("_swift_bench_\(ProcessInfo.processInfo.processIdentifier)")
try? fm.createDirectory(at: base, withIntermediateDirectories: true)
let p = base.appendingPathComponent("data.txt")
let pBulk = base.appendingPathComponent("bulk.txt")
let lineData = Data("line payload number 0123456789 abcdefghijklmnopqrstuvwxyz\n".utf8)

var t0 = DispatchTime.now().uptimeNanoseconds
fm.createFile(atPath: p.path, contents: nil)
if let fh = try? FileHandle(forWritingTo: p) {
    var buf = Data(capacity: 65536)
    for _ in 0..<100000 {
        buf.append(lineData)
        if buf.count >= 65536 {
            fh.write(buf)
            buf.removeAll(keepingCapacity: true)
        }
    }
    if !buf.isEmpty { fh.write(buf) }
    try? fh.close()
}
let msWriteStream = msSince(t0)

let dataStr = (try? String(contentsOf: p, encoding: .utf8)) ?? ""
_ = try? String(contentsOf: p, encoding: .utf8)

t0 = DispatchTime.now().uptimeNanoseconds
try? dataStr.write(to: pBulk, atomically: false, encoding: .utf8)
let msWriteText = msSince(t0)

t0 = DispatchTime.now().uptimeNanoseconds
let rt = (try? String(contentsOf: p, encoding: .utf8)) ?? ""
let msReadText = msSince(t0)

t0 = DispatchTime.now().uptimeNanoseconds
let rt2 = (try? String(contentsOf: p, encoding: .utf8)) ?? ""
let rl = rt2.split(separator: "\n", omittingEmptySubsequences: false)
let msReadLines = msSince(t0)

t0 = DispatchTime.now().uptimeNanoseconds
var lineTotal = 0
if let fh = try? FileHandle(forReadingFrom: p) {
    var rem = Data()
    while true {
        let chunk = fh.readData(ofLength: 65536)
        if chunk.isEmpty { break }
        rem.append(chunk)
        var start = rem.startIndex
        while let nl = rem[start...].firstIndex(of: 0x0a) {
            lineTotal += (nl - start)
            start = nl + 1
        }
        rem.removeSubrange(rem.startIndex..<start)
    }
    try? fh.close()
}
let msEachLine = msSince(t0)

t0 = DispatchTime.now().uptimeNanoseconds
if let fh = try? FileHandle(forReadingFrom: p) {
    while !fh.readData(ofLength: 65536).isEmpty {}
    try? fh.close()
}
let msReadChunk = msSince(t0)
try? fm.removeItem(at: base)

// 2. Crypto, Path, Process
let bigStr = String(repeating: "0123456789abcdef", count: 4 * 16384)
let bigData = Data(bigStr.utf8)

#if canImport(CryptoKit)
_ = SHA256.hash(data: bigData)
t0 = DispatchTime.now().uptimeNanoseconds
for _ in 0..<50 {
    let d = SHA256.hash(data: bigData)
    _ = d.map { String(format: "%02x", $0) }.joined()
}
let msSha = msSince(t0)
#endif

var b64 = ""
t0 = DispatchTime.now().uptimeNanoseconds
for _ in 0..<50 {
    b64 = bigData.base64EncodedString()
}
let msB64e = msSince(t0)

t0 = DispatchTime.now().uptimeNanoseconds
for _ in 0..<50 {
    if let d = Data(base64Encoded: b64) {
        _ = String(data: d, encoding: .utf8)
    }
}
let msB64d = msSince(t0)

t0 = DispatchTime.now().uptimeNanoseconds
for _ in 0..<10000 {
    _ = UUID().uuidString
}
let msUuid = msSince(t0)

t0 = DispatchTime.now().uptimeNanoseconds
for _ in 0..<100000 {
    _ = (("/var/log" as NSString).appendingPathComponent("app/../service/output.log") as NSString).standardizingPath
}
let msPath = msSince(t0)

t0 = DispatchTime.now().uptimeNanoseconds
for _ in 0..<50 {
    let proc = Process()
    #if os(Windows)
    proc.executableURL = URL(fileURLWithPath: "C:\\Windows\\System32\\cmd.exe")
    proc.arguments = ["/c", "exit", "0"]
    #else
    proc.executableURL = URL(fileURLWithPath: "/usr/bin/true")
    #endif
    proc.standardOutput = Pipe()
    proc.standardError = Pipe()
    try? proc.run()
    proc.waitUntilExit()
}
let msProc = msSince(t0)

// 3. JSON + Pipeline
var items: [Item] = []
items.reserveCapacity(10000)
for i in 0..<10000 {
    items.append(Item(id: i, user: "alice", score: i % 100))
}
let enc = JSONEncoder()
let dec = JSONDecoder()

t0 = DispatchTime.now().uptimeNanoseconds
let rawJson = (try? enc.encode(items)) ?? Data()
let msJsonEnc = msSince(t0)

t0 = DispatchTime.now().uptimeNanoseconds
let decoded = (try? dec.decode([Item].self, from: rawJson)) ?? []
let msJsonDec = msSince(t0)

var nums = [Int](repeating: 0, count: 1_000_000)
for i in 0..<1_000_000 { nums[i] = i }

t0 = DispatchTime.now().uptimeNanoseconds
let sum = nums.lazy.filter { $0 % 2 == 0 }.map { $0 * 3 + 1 }.reduce(0, +)
let msPipe = msSince(t0)

if sum < 0 || decoded.isEmpty || rt.isEmpty || rl.isEmpty || lineTotal < 0 { exit(1) }
print("CHECK|\(sum)")

let fnums = (0..<1_000_000).map { Double($0) }
t0 = DispatchTime.now().uptimeNanoseconds
let fsum = fnums.lazy.filter { $0 > 250000.0 }.map { $0 * 1.5 + 0.25 }.reduce(0.0, +)
let msFPipe = msSince(t0)
if fsum < 0 { exit(1) }

let sampleUrl = "https://api.example.com:8443/v1/items?tag=prod&limit=50&offset=100#top"
t0 = DispatchTime.now().uptimeNanoseconds
var urlAcc = 0
for _ in 0..<50000 {
    if let uc = URLComponents(string: sampleUrl) {
        urlAcc += uc.queryItems?.count ?? 0
    }
}
let msUrl = msSince(t0)
if urlAcc < 0 { exit(1) }

struct RouteSeg { let kind: Int; let val: String }
struct RouteDef { let method: String; let segs: [RouteSeg]; let wildcard: Bool; let handler: ([String: String]) -> (Int, String) }
func parsePat(_ pat: String) -> ([RouteSeg], Bool) {
    let tr = pat.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    if tr.isEmpty { return ([], false) }
    var wc = false
    let segs = tr.split(separator: "/").map { s -> RouteSeg in
        if s.hasPrefix(":") { return RouteSeg(kind: 1, val: String(s.dropFirst())) }
        if s.hasPrefix("*") { wc = true; return RouteSeg(kind: 2, val: String(s.dropFirst())) }
        return RouteSeg(kind: 0, val: String(s))
    }
    return (segs, wc)
}
func mkRoute(_ m: String, _ p: String, _ h: @escaping ([String: String]) -> (Int, String)) -> RouteDef {
    let (s, wc) = parsePat(p)
    return RouteDef(method: m, segs: s, wildcard: wc, handler: h)
}
let routes: [RouteDef] = [
    mkRoute("GET", "/") { _ in (200, "ok") },
    mkRoute("GET", "/health") { _ in (200, "ok") },
    mkRoute("GET", "/api/v1/items") { _ in (200, "items") },
    mkRoute("GET", "/api/v1/users/:id/orders") { p in (200, p["id"] ?? "") },
    mkRoute("GET", "/files/*rest") { _ in (200, "file") },
    mkRoute("POST", "/api/v1/echo") { _ in (200, "") },
]
let samplePaths = ["/", "/health", "/api/v1/users/42/orders", "/files/docs/readme.md"]
t0 = DispatchTime.now().uptimeNanoseconds
var httpAcc = 0
for i in 0..<50000 {
    let tr = samplePaths[i & 3].trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    let pathSegs: [Substring] = tr.isEmpty ? [] : tr.split(separator: "/")
    let nSegs = pathSegs.count
    var st = 404
    var body = ""
    for r in routes {
        if !r.wildcard && r.segs.count != nSegs { continue }
        var params: [String: String] = [:]
        var ok = true
        for idx in 0..<r.segs.count {
            let seg = r.segs[idx]
            if seg.kind == 2 {
                params[seg.val] = pathSegs[idx...].joined(separator: "/")
                break
            } else if seg.kind == 1 {
                if idx >= nSegs || pathSegs[idx].isEmpty { ok = false; break }
                params[seg.val] = String(pathSegs[idx])
            } else if idx >= nSegs || pathSegs[idx] != seg.val {
                ok = false; break
            }
        }
        if ok && r.method == "GET" {
            let res = r.handler(params)
            st = res.0; body = res.1
            break
        }
    }
    httpAcc += st + body.count
}
let msHttp = msSince(t0)
if httpAcc != 10_125_000 { exit(1) }

print(String(format: "BENCH|vm_fib_35|%.3f", msVmFib))
print(String(format: "BENCH|vm_arith_10m|%.3f", msVmArith))
print(String(format: "BENCH|vm_map_100k|%.3f", msVmMap))

print(String(format: "BENCH|fs_write_text|%.3f", msWriteText))
print(String(format: "BENCH|fs_write_lines|%.3f", msWriteStream))
print(String(format: "BENCH|fs_read_text|%.3f", msReadText))
print(String(format: "BENCH|fs_read_lines|%.3f", msReadLines))
print(String(format: "BENCH|fs_each_line|%.3f", msEachLine))
print(String(format: "BENCH|fs_read_chunk64k|%.3f", msReadChunk))
#if canImport(CryptoKit)
print(String(format: "BENCH|crypto_sha256_50m|%.3f", msSha))
#endif
print(String(format: "BENCH|crypto_b64_enc_50m|%.3f", msB64e))
print(String(format: "BENCH|crypto_b64_dec_50m|%.3f", msB64d))
print(String(format: "BENCH|crypto_uuid_10k|%.3f", msUuid))
print(String(format: "BENCH|path_join_norm_100k|%.3f", msPath))
print(String(format: "BENCH|process_run_50x|%.3f", msProc))
print(String(format: "BENCH|json_encode_10k|%.3f", msJsonEnc))
print(String(format: "BENCH|json_decode_10k|%.3f", msJsonDec))
print(String(format: "BENCH|pipeline_1m|%.3f", msPipe))
print(String(format: "BENCH|pipeline_float_1m|%.3f", msFPipe))
print(String(format: "BENCH|url_parse_50k|%.3f", msUrl))
print(String(format: "BENCH|http_router_50k|%.3f", msHttp))
