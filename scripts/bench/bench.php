<?php
ini_set('memory_limit', '512M');

function ms_since(int $t0): float {
    return (hrtime(true) - $t0) / 1e6;
}

function vm_fib(int $n): int {
    return $n < 2 ? $n : vm_fib($n - 1) + vm_fib($n - 2);
}

$t0 = hrtime(true);
if (vm_fib(35) !== 9227465) exit(1);
$msVmFib = ms_since($t0);

$t0 = hrtime(true);
$arithAcc = 0;
for ($i = 0; $i < 10000000; $i++) {
    $arithAcc = ($arithAcc * 1664525 + 1013904223 + $i) % 1000000007;
}
$msVmArith = ms_since($t0);
if ($arithAcc < 0) exit(1);

$t0 = hrtime(true);
$vmM = [];
for ($i = 0; $i < 100000; $i++) {
    $vmM[$i] = $i * 3 + 1;
}
$mapAcc = 0;
for ($i = 0; $i < 100000; $i++) {
    $mapAcc += $vmM[$i];
}
$msVmMap = ms_since($t0);
if ($mapAcc < 0) exit(1);

$t0 = hrtime(true);
$adtAcc = 0;
for ($i = 0; $i < 1000000; $i++) {
    $r = $i & 3;
    $ev = match ($r) {
        0 => ['t' => 0],
        1 => ['t' => 1, 'a' => $i, 'b' => 7],
        2 => ['t' => 2, 'v' => $i, 'f' => 3],
        default => ['t' => 1, 'a' => $i, 'b' => -2],
    };
    $adtAcc += match ($ev['t']) {
        0 => 1,
        1 => $ev['a'] + $ev['b'],
        2 => $ev['v'] * $ev['f'],
    };
}
$msAdt = ms_since($t0);
if ($adtAcc !== 625001500000) exit(1);

// 1. FS I/O
$base = sys_get_temp_dir() . DIRECTORY_SEPARATOR . '_php_bench_' . getmypid();
@mkdir($base, 0755, true);
$p = $base . DIRECTORY_SEPARATOR . 'data.txt';
$pBulk = $base . DIRECTORY_SEPARATOR . 'bulk.txt';
$line = "line payload number 0123456789 abcdefghijklmnopqrstuvwxyz\n";

$t0 = hrtime(true);
$fw = fopen($p, 'wb');
stream_set_write_buffer($fw, 65536);
for ($i = 0; $i < 100000; $i++) {
    fwrite($fw, $line);
}
fclose($fw);
$msWriteStream = ms_since($t0);

$data = file_get_contents($p);
file_get_contents($p);

$t0 = hrtime(true);
file_put_contents($pBulk, $data);
$msWriteText = ms_since($t0);

$t0 = hrtime(true);
$rt = file_get_contents($p);
$msReadText = ms_since($t0);

$t0 = hrtime(true);
$rl = file($p, FILE_IGNORE_NEW_LINES);
$msReadLines = ms_since($t0);

$t0 = hrtime(true);
$fr = fopen($p, 'rb');
$total = 0;
while (($ln = fgets($fr)) !== false) {
    $total += strlen($ln);
}
fclose($fr);
$msEachLine = ms_since($t0);

$t0 = hrtime(true);
$fc = fopen($p, 'rb');
while (!feof($fc)) {
    fread($fc, 65536);
}
fclose($fc);
$msReadChunk = ms_since($t0);

@unlink($p);
@unlink($pBulk);
@rmdir($base);

// 2. Crypto, Path, Process
$big = str_repeat('0123456789abcdef', 4 * 16384);
hash('sha256', $big);
base64_encode($big);

$t0 = hrtime(true);
for ($i = 0; $i < 50; $i++) {
    hash('sha256', $big);
}
$msSha = ms_since($t0);

$b64 = '';
$t0 = hrtime(true);
for ($i = 0; $i < 50; $i++) {
    $b64 = base64_encode($big);
}
$msB64e = ms_since($t0);

$t0 = hrtime(true);
for ($i = 0; $i < 50; $i++) {
    base64_decode($b64, true);
}
$msB64d = ms_since($t0);

$t0 = hrtime(true);
for ($i = 0; $i < 10000; $i++) {
    $b = random_bytes(16);
    $b[6] = chr((ord($b[6]) & 0x0f) | 0x40);
    $b[8] = chr((ord($b[8]) & 0x3f) | 0x80);
    vsprintf('%s%s-%s-%s-%s-%s%s%s', str_split(bin2hex($b), 4));
}
$msUuid = ms_since($t0);

$t0 = hrtime(true);
for ($i = 0; $i < 100000; $i++) {
    $joined = '/var/log' . DIRECTORY_SEPARATOR . 'app/../service/output.log';
    $parts = explode('/', str_replace('\\', '/', $joined));
    $stack = [];
    foreach ($parts as $part) {
        if ($part === '' || $part === '.') continue;
        if ($part === '..') { array_pop($stack); }
        else { $stack[] = $part; }
    }
    '/' . implode('/', $stack);
}
$msPath = ms_since($t0);

$cmd = (PHP_OS_FAMILY === 'Windows') ? 'cmd /c exit 0' : 'true';
$t0 = hrtime(true);
for ($i = 0; $i < 50; $i++) {
    $out = [];
    $code = 0;
    exec($cmd, $out, $code);
}
$msProc = ms_since($t0);

// 3. JSON + Pipeline
$items = [];
for ($i = 0; $i < 10000; $i++) {
    $items[] = ['id' => $i, 'user' => 'alice', 'score' => $i % 100];
}

$t0 = hrtime(true);
$rawJson = json_encode($items);
$msJsonEnc = ms_since($t0);

$t0 = hrtime(true);
$decoded = json_decode($rawJson, true);
$msJsonDec = ms_since($t0);

$nums = range(0, 999999);
$t0 = hrtime(true);
$filtered = array_filter($nums, fn($x) => ($x % 2) === 0);
$mapped = array_map(fn($x) => $x * 3 + 1, $filtered);
$sum = array_sum($mapped);
$msPipe = ms_since($t0);
printf("CHECK|%d\n", $sum);

$fnums = array_map('floatval', range(0, 999999));
$t0 = hrtime(true);
$ffiltered = array_filter($fnums, fn($x) => $x > 250000.0);
$fmapped = array_map(fn($x) => $x * 1.5 + 0.25, $ffiltered);
$fsum = array_sum($fmapped);
$msFPipe = ms_since($t0);
if ($fsum < 0) exit(1);

$csvLines = ["id,name,note"];
for ($i = 0; $i < 20000; $i++) {
    $csvLines[] = "$i,\"user, #$i\",\"ok \"\"v1\"\"\"";
}
$csvRaw = implode("\n", $csvLines);
$t0 = hrtime(true);
$csvRows = [];
foreach (explode("\n", $csvRaw) as $l) {
    $csvRows[] = str_getcsv($l, ",", "\"", "");
}
$msCsv = ms_since($t0);
if (count($csvRows) < 0) exit(1);

$sampleUrl = "https://api.example.com:8443/v1/items?tag=prod&limit=50&offset=100#top";
$t0 = hrtime(true);
$urlAcc = 0;
for ($i = 0; $i < 50000; $i++) {
    $u = parse_url($sampleUrl);
    $q = [];
    parse_str($u['query'] ?? '', $q);
    $urlAcc += count($q);
}
$msUrl = ms_since($t0);
if ($urlAcc < 0) exit(1);

$parsePat = function(string $pat): array {
    $tr = trim($pat, '/');
    if ($tr === '') return [[], false];
    $parts = explode('/', $tr);
    $segs = [];
    $wc = false;
    foreach ($parts as $s) {
        if ($s[0] === ':') $segs[] = [1, substr($s, 1)];
        elseif ($s[0] === '*') { $segs[] = [2, substr($s, 1)]; $wc = true; }
        else $segs[] = [0, $s];
    }
    return [$segs, $wc];
};
$routes = [
    ['GET', ...$parsePat('/'), fn($p) => [200, 'ok']],
    ['GET', ...$parsePat('/health'), fn($p) => [200, 'ok']],
    ['GET', ...$parsePat('/api/v1/items'), fn($p) => [200, 'items']],
    ['GET', ...$parsePat('/api/v1/users/:id/orders'), fn($p) => [200, $p['id'] ?? '']],
    ['GET', ...$parsePat('/files/*rest'), fn($p) => [200, 'file']],
    ['POST', ...$parsePat('/api/v1/echo'), fn($p) => [200, '']],
];
$samplePaths = ['/', '/health', '/api/v1/users/42/orders', '/files/docs/readme.md'];
$t0 = hrtime(true);
$httpAcc = 0;
for ($i = 0; $i < 50000; $i++) {
    $tr = trim($samplePaths[$i & 3], '/');
    $pathSegs = $tr === '' ? [] : explode('/', $tr);
    $nSegs = count($pathSegs);
    $st = 404;
    $body = '';
    foreach ($routes as $r) {
        [$rm, $segs, $wc, $h] = $r;
        if (!$wc && count($segs) !== $nSegs) continue;
        $params = [];
        $ok = true;
        foreach ($segs as $idx => $seg) {
            if ($seg[0] === 2) {
                $params[$seg[1]] = implode('/', array_slice($pathSegs, $idx));
                break;
            } elseif ($seg[0] === 1) {
                if ($idx >= $nSegs || $pathSegs[$idx] === '') { $ok = false; break; }
                $params[$seg[1]] = $pathSegs[$idx];
            } elseif ($idx >= $nSegs || $pathSegs[$idx] !== $seg[1]) {
                $ok = false; break;
            }
        }
        if ($ok && $rm === 'GET') {
            [$st, $body] = $h($params);
            break;
        }
    }
    $httpAcc += $st + strlen($body);
}
$msHttp = ms_since($t0);
if ($httpAcc !== 10125000) exit(1);

$b64urlEnc = fn(string $s): string => rtrim(strtr(base64_encode($s), '+/', '-_'), '=');
$b64urlDec = fn(string $s): string => base64_decode(strtr($s, '-_', '+/'), true) ?: '';
$jwtSecret = 'conduit_bench_jwt_secret_key_32b';
$jwtHdr = $b64urlEnc('{"alg":"HS256","typ":"JWT"}');
$t0 = hrtime(true);
$jwtAcc = 0;
for ($i = 0; $i < 10000; $i++) {
    $payB64 = $b64urlEnc(json_encode(['sub' => 'user', 'uid' => $i]));
    $sigInput = $jwtHdr . '.' . $payB64;
    $sigB64 = $b64urlEnc(hash_hmac('sha256', $sigInput, $jwtSecret, true));
    $tok = $sigInput . '.' . $sigB64;

    $parts = explode('.', $tok);
    if (count($parts) !== 3) exit(1);
    $expectedSig = $b64urlEnc(hash_hmac('sha256', $parts[0] . '.' . $parts[1], $jwtSecret, true));
    if (!hash_equals($expectedSig, $parts[2])) exit(1);
    $cl = json_decode($b64urlDec($parts[1]), true);
    $jwtAcc += $cl['uid'];
}
$msJwt = ms_since($t0);
if ($jwtAcc !== 49995000) exit(1);

$lru = [];
$t0 = hrtime(true);
$cacheAcc = 0;
for ($i = 0; $i < 100000; $i++) {
    $ks = 'k' . ($i & 8191);
    if (isset($lru[$ks])) {
        unset($lru[$ks]);
    } elseif (count($lru) >= 4096) {
        $oldest = array_key_first($lru);
        unset($lru[$oldest]);
    }
    $lru[$ks] = 'v';

    $kg = 'k' . (($i + 6144) & 8191);
    if (isset($lru[$kg])) {
        $v = $lru[$kg];
        unset($lru[$kg]);
        $lru[$kg] = $v;
        $cacheAcc++;
    }
}
$msCache = ms_since($t0);
if ($cacheAcc !== 97905) exit(1);

printf("BENCH|vm_fib_35|%.3f\n", $msVmFib);
printf("BENCH|vm_arith_10m|%.3f\n", $msVmArith);
printf("BENCH|vm_map_100k|%.3f\n", $msVmMap);
printf("BENCH|adt_switch_1m|%.3f\n", $msAdt);

printf("BENCH|fs_write_text|%.3f\n", $msWriteText);
printf("BENCH|fs_write_lines|%.3f\n", $msWriteStream);
printf("BENCH|fs_read_text|%.3f\n", $msReadText);
printf("BENCH|fs_read_lines|%.3f\n", $msReadLines);
printf("BENCH|fs_each_line|%.3f\n", $msEachLine);
printf("BENCH|fs_read_chunk64k|%.3f\n", $msReadChunk);
printf("BENCH|crypto_sha256_50m|%.3f\n", $msSha);
printf("BENCH|crypto_b64_enc_50m|%.3f\n", $msB64e);
printf("BENCH|crypto_b64_dec_50m|%.3f\n", $msB64d);
printf("BENCH|crypto_uuid_10k|%.3f\n", $msUuid);
printf("BENCH|path_join_norm_100k|%.3f\n", $msPath);
printf("BENCH|process_run_50x|%.3f\n", $msProc);
printf("BENCH|json_encode_10k|%.3f\n", $msJsonEnc);
printf("BENCH|json_decode_10k|%.3f\n", $msJsonDec);
printf("BENCH|pipeline_1m|%.3f\n", $msPipe);
printf("BENCH|pipeline_float_1m|%.3f\n", $msFPipe);
printf("BENCH|csv_parse_20k|%.3f\n", $msCsv);
printf("BENCH|url_parse_50k|%.3f\n", $msUrl);
printf("BENCH|http_router_50k|%.3f\n", $msHttp);
printf("BENCH|jwt_hs256_10k|%.3f\n", $msJwt);
printf("BENCH|cache_lru_100k|%.3f\n", $msCache);
