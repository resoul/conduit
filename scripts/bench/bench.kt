import java.io.BufferedOutputStream
import java.io.File
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Paths
import java.security.MessageDigest
import java.util.Base64
import java.util.LinkedHashMap
import java.util.UUID
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

sealed interface BenchEvent {
    object Tick : BenchEvent
    class Add(val a: Long, val b: Long) : BenchEvent
    class Scale(val v: Long, val f: Long) : BenchEvent
}

fun msSince(t0: Long): Double = (System.nanoTime() - t0) / 1_000_000.0

fun vmFib(n: Long): Long = if (n < 2L) n else vmFib(n - 1L) + vmFib(n - 2L)

fun main() {
    var tVm = System.nanoTime()
    if (vmFib(35L) != 9227465L) return
    val msVmFib = msSince(tVm)

    tVm = System.nanoTime()
    var arithAcc = 0L
    for (i in 0L until 10_000_000L) {
        arithAcc = (arithAcc * 1664525L + 1013904223L + i) % 1000000007L
    }
    val msVmArith = msSince(tVm)
    if (arithAcc < 0L) return

    tVm = System.nanoTime()
    val vmM = HashMap<Int, Int>(100_000)
    for (i in 0 until 100_000) vmM[i] = i * 3 + 1
    var mapAcc = 0L
    for (i in 0 until 100_000) mapAcc += (vmM[i] ?: 0)
    val msVmMap = msSince(tVm)
    if (mapAcc < 0L) return

    tVm = System.nanoTime()
    var adtAcc = 0L
    for (i in 0L until 1_000_000L) {
        val ev: BenchEvent = when ((i and 3L).toInt()) {
            0 -> BenchEvent.Tick
            1 -> BenchEvent.Add(i, 7L)
            2 -> BenchEvent.Scale(i, 3L)
            else -> BenchEvent.Add(i, -2L)
        }
        adtAcc += when (ev) {
            is BenchEvent.Tick -> 1L
            is BenchEvent.Add -> ev.a + ev.b
            is BenchEvent.Scale -> ev.v * ev.f
        }
    }
    val msAdt = msSince(tVm)
    if (adtAcc != 625_001_500_000L) return
    // 1. FS I/O
    val base = Files.createTempDirectory("_kt_bench_").toFile()
    val p = File(base, "data.txt").toPath()
    val pBulk = File(base, "bulk.txt").toPath()
    val line = "line payload number 0123456789 abcdefghijklmnopqrstuvwxyz\n".toByteArray(StandardCharsets.UTF_8)

    var t0 = System.nanoTime()
    BufferedOutputStream(Files.newOutputStream(p), 65536).use { out ->
        for (i in 0 until 100000) out.write(line)
    }
    val msWriteStream = msSince(t0)

    val dataStr = Files.readString(p)
    Files.readString(p)

    t0 = System.nanoTime()
    Files.writeString(pBulk, dataStr)
    val msWriteText = msSince(t0)

    t0 = System.nanoTime()
    val rt = Files.readString(p)
    val msReadText = msSince(t0)

    t0 = System.nanoTime()
    val rl = Files.readAllLines(p)
    val msReadLines = msSince(t0)

    t0 = System.nanoTime()
    var lineTotal = 0
    Files.newBufferedReader(p).use { br ->
        while (true) {
            val ln = br.readLine() ?: break
            lineTotal += ln.length
        }
    }
    val msEachLine = msSince(t0)

    t0 = System.nanoTime()
    val buf64 = ByteArray(65536)
    Files.newInputStream(p).use { ins ->
        while (ins.read(buf64) > 0) {}
    }
    val msReadChunk = msSince(t0)
    base.deleteRecursively()

    // 2. Crypto, Path, Process
    val bigBytes = "0123456789abcdef".repeat(4 * 16384).toByteArray(StandardCharsets.UTF_8)
    val md = MessageDigest.getInstance("SHA-256")
    md.digest(bigBytes)
    val b64Enc = Base64.getEncoder()
    val b64Dec = Base64.getDecoder()
    b64Enc.encodeToString(bigBytes)

    t0 = System.nanoTime()
    for (i in 0 until 50) {
        val d = md.digest(bigBytes)
        d.joinToString("") { "%02x".format(it) }
    }
    val msSha = msSince(t0)

    var b64 = ""
    t0 = System.nanoTime()
    for (i in 0 until 50) b64 = b64Enc.encodeToString(bigBytes)
    val msB64e = msSince(t0)

    t0 = System.nanoTime()
    for (i in 0 until 50) String(b64Dec.decode(b64), StandardCharsets.UTF_8)
    val msB64d = msSince(t0)

    t0 = System.nanoTime()
    for (i in 0 until 10000) UUID.randomUUID().toString()
    val msUuid = msSince(t0)

    t0 = System.nanoTime()
    for (i in 0 until 100000) Paths.get("/var/log", "app/../service/output.log").normalize().toString()
    val msPath = msSince(t0)

    val isWin = System.getProperty("os.name").lowercase().contains("win")
    val cmd = if (isWin) listOf("cmd", "/c", "exit", "0") else listOf("true")
    t0 = System.nanoTime()
    for (i in 0 until 50) {
        val pr = ProcessBuilder(cmd).start()
        pr.waitFor()
    }
    val msProc = msSince(t0)

    // 3. Pipeline
    val nums = LongArray(1_000_000) { it.toLong() }
    t0 = System.nanoTime()
    var sum = 0L
    for (x in nums) {
        if (x % 2L == 0L) sum += x * 3L + 1L
    }
    val msPipe = msSince(t0)
    if (sum < 0 || rt.isEmpty() || rl.isEmpty() || lineTotal < 0) return
    println("CHECK|$sum")

    val fnums = DoubleArray(1_000_000) { it.toDouble() }
    t0 = System.nanoTime()
    var fsum = 0.0
    for (x in fnums) {
        if (x > 250000.0) fsum += x * 1.5 + 0.25
    }
    val msFPipe = msSince(t0)
    if (fsum < 0.0) return

    val sampleUrl = "https://api.example.com:8443/v1/items?tag=prod&limit=50&offset=100#top"
    t0 = System.nanoTime()
    var urlAcc = 0
    for (i in 0 until 50_000) {
        val u = java.net.URI(sampleUrl)
        val q = u.rawQuery ?: ""
        val pairs = if (q.isEmpty()) 0 else q.split('&').size
        urlAcc += pairs
    }
    val msUrl = msSince(t0)
    if (urlAcc < 0) return

    data class RouteDef(val method: String, val segs: List<Pair<Int, String>>, val wildcard: Boolean, val handler: (Map<String, String>) -> Pair<Int, String>)
    fun parsePat(pat: String): Pair<List<Pair<Int, String>>, Boolean> {
        val tr = pat.trim('/')
        if (tr.isEmpty()) return emptyList<Pair<Int, String>>() to false
        var wc = false
        val segs = tr.split('/').map { s ->
            when {
                s.startsWith(':') -> 1 to s.substring(1)
                s.startsWith('*') -> { wc = true; 2 to s.substring(1) }
                else -> 0 to s
            }
        }
        return segs to wc
    }
    fun mkRoute(m: String, p: String, h: (Map<String, String>) -> Pair<Int, String>): RouteDef {
        val (segs, wc) = parsePat(p)
        return RouteDef(m, segs, wc, h)
    }
    val routes = listOf(
        mkRoute("GET", "/") { 200 to "ok" },
        mkRoute("GET", "/health") { 200 to "ok" },
        mkRoute("GET", "/api/v1/items") { 200 to "items" },
        mkRoute("GET", "/api/v1/users/:id/orders") { p -> 200 to (p["id"] ?: "") },
        mkRoute("GET", "/files/*rest") { 200 to "file" },
        mkRoute("POST", "/api/v1/echo") { 200 to "" },
    )
    val samplePaths = arrayOf("/", "/health", "/api/v1/users/42/orders", "/files/docs/readme.md")
    t0 = System.nanoTime()
    var httpAcc = 0
    for (i in 0 until 50_000) {
        val tr = samplePaths[i and 3].trim('/')
        val pathSegs = if (tr.isEmpty()) emptyList() else tr.split('/')
        val nSegs = pathSegs.size
        var st = 404
        var body = ""
        for (r in routes) {
            if (!r.wildcard && r.segs.size != nSegs) continue
            val params = HashMap<String, String>(2)
            var ok = true
            for (idx in r.segs.indices) {
                val (kind, value) = r.segs[idx]
                if (kind == 2) { params[value] = pathSegs.subList(idx, nSegs).joinToString("/"); break }
                else if (kind == 1) {
                    if (idx >= nSegs || pathSegs[idx].isEmpty()) { ok = false; break }
                    params[value] = pathSegs[idx]
                } else if (idx >= nSegs || pathSegs[idx] != value) { ok = false; break }
            }
            if (ok && r.method == "GET") {
                val res = r.handler(params)
                st = res.first
                body = res.second
                break
            }
        }
        httpAcc += st + body.length
    }
    val msHttp = msSince(t0)
    if (httpAcc != 10_125_000) return

    val b64UrlEnc = Base64.getUrlEncoder().withoutPadding()
    val b64UrlDec = Base64.getUrlDecoder()
    val jwtSecret = "conduit_bench_jwt_secret_key_32b".toByteArray(StandardCharsets.UTF_8)
    val macInst = Mac.getInstance("HmacSHA256")
    macInst.init(SecretKeySpec(jwtSecret, "HmacSHA256"))
    val jwtHdr = b64UrlEnc.encodeToString("{\"alg\":\"HS256\",\"typ\":\"JWT\"}".toByteArray(StandardCharsets.UTF_8))
    t0 = System.nanoTime()
    var jwtAcc = 0
    for (i in 0 until 10_000) {
        val payJson = "{\"sub\":\"user\",\"uid\":$i}"
        val payB64 = b64UrlEnc.encodeToString(payJson.toByteArray(StandardCharsets.UTF_8))
        val sigInput = "$jwtHdr.$payB64"
        val sigB64 = b64UrlEnc.encodeToString(macInst.doFinal(sigInput.toByteArray(StandardCharsets.UTF_8)))
        val tok = "$sigInput.$sigB64"

        val parts = tok.split('.')
        if (parts.size != 3) return
        val expectedSig = b64UrlEnc.encodeToString(
            macInst.doFinal("${parts[0]}.${parts[1]}".toByteArray(StandardCharsets.UTF_8))
        )
        if (!MessageDigest.isEqual(
                parts[2].toByteArray(StandardCharsets.UTF_8),
                expectedSig.toByteArray(StandardCharsets.UTF_8)
            )
        ) return
        val rawPay = String(b64UrlDec.decode(parts[1]), StandardCharsets.UTF_8)
        val colonIdx = rawPay.lastIndexOf(':')
        val braceIdx = rawPay.lastIndexOf('}')
        if (colonIdx < 0 || braceIdx < 0) return
        jwtAcc += rawPay.substring(colonIdx + 1, braceIdx).trim().toInt()
    }
    val msJwt = msSince(t0)
    if (jwtAcc != 49_995_000) return

    val lru = object : LinkedHashMap<String, String>(4096, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, String>?): Boolean {
            return size > 4096
        }
    }
    t0 = System.nanoTime()
    var cacheAcc = 0
    for (i in 0 until 100_000) {
        lru["k${i and 8191}"] = "v"
        if (lru["k${(i + 6144) and 8191}"] != null) {
            cacheAcc++
        }
    }
    val msCache = msSince(t0)
    if (cacheAcc != 97905) return

    println("BENCH|vm_fib_35|%.3f".format(msVmFib))
    println("BENCH|vm_arith_10m|%.3f".format(msVmArith))
    println("BENCH|vm_map_100k|%.3f".format(msVmMap))
    println("BENCH|adt_switch_1m|%.3f".format(msAdt))

    println("BENCH|fs_write_text|%.3f".format(msWriteText))
    println("BENCH|fs_write_lines|%.3f".format(msWriteStream))
    println("BENCH|fs_read_text|%.3f".format(msReadText))
    println("BENCH|fs_read_lines|%.3f".format(msReadLines))
    println("BENCH|fs_each_line|%.3f".format(msEachLine))
    println("BENCH|fs_read_chunk64k|%.3f".format(msReadChunk))
    println("BENCH|crypto_sha256_50m|%.3f".format(msSha))
    println("BENCH|crypto_b64_enc_50m|%.3f".format(msB64e))
    println("BENCH|crypto_b64_dec_50m|%.3f".format(msB64d))
    println("BENCH|crypto_uuid_10k|%.3f".format(msUuid))
    println("BENCH|path_join_norm_100k|%.3f".format(msPath))
    println("BENCH|process_run_50x|%.3f".format(msProc))
    println("BENCH|pipeline_1m|%.3f".format(msPipe))
    println("BENCH|pipeline_float_1m|%.3f".format(msFPipe))
    println("BENCH|url_parse_50k|%.3f".format(msUrl))
    println("BENCH|http_router_50k|%.3f".format(msHttp))
    println("BENCH|jwt_hs256_10k|%.3f".format(msJwt))
    println("BENCH|cache_lru_100k|%.3f".format(msCache))
}
