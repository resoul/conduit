import java.io.BufferedOutputStream
import java.io.File
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.Paths
import java.security.MessageDigest
import java.util.Base64
import java.util.UUID

fun msSince(t0: Long): Double = (System.nanoTime() - t0) / 1_000_000.0

fun main() {
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
}
