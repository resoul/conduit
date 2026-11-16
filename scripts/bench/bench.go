package main

import (
	"bufio"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"
)

type Item struct {
	ID    int    `json:"id"`
	User  string `json:"user"`
	Score int    `json:"score"`
}

func msSince(t time.Time) float64 {
	return float64(time.Since(t).Nanoseconds()) / 1e6
}

func main() {
	base := filepath.Join(os.TempDir(), fmt.Sprintf("_go_bench_%d", os.Getpid()))
	_ = os.MkdirAll(base, 0755)
	p := filepath.Join(base, "data.txt")
	pBulk := filepath.Join(base, "bulk.txt")
	line := "line payload number 0123456789 abcdefghijklmnopqrstuvwxyz\n"

	t0 := time.Now()
	f, _ := os.Create(p)
	bw := bufio.NewWriterSize(f, 65536)
	for i := 0; i < 100000; i++ {
		_, _ = bw.WriteString(line)
	}
	_ = bw.Flush()
	_ = f.Close()
	msWriteStream := msSince(t0)

	data, _ := os.ReadFile(p)
	_, _ = os.ReadFile(p)

	t0 = time.Now()
	_ = os.WriteFile(pBulk, data, 0644)
	msWriteText := msSince(t0)

	t0 = time.Now()
	b, _ := os.ReadFile(p)
	_ = len(string(b))
	msReadText := msSince(t0)

	t0 = time.Now()
	b2, _ := os.ReadFile(p)
	_ = len(strings.Split(string(b2), "\n"))
	msReadLines := msSince(t0)

	t0 = time.Now()
	fr, _ := os.Open(p)
	sc := bufio.NewScanner(fr)
	total := 0
	for sc.Scan() {
		total += len(sc.Text())
	}
	_ = fr.Close()
	msEachLine := msSince(t0)

	t0 = time.Now()
	fc, _ := os.Open(p)
	buf := make([]byte, 65536)
	for {
		_, err := fc.Read(buf)
		if err == io.EOF {
			break
		}
	}
	_ = fc.Close()
	msReadChunk := msSince(t0)
	_ = os.RemoveAll(base)

	big := strings.Repeat("0123456789abcdef", 4*16384)
	bigB := []byte(big)

	t0 = time.Now()
	for i := 0; i < 50; i++ {
		sum := sha256.Sum256(bigB)
		_ = hex.EncodeToString(sum[:])
	}
	msSha := msSince(t0)

	var b64 string
	t0 = time.Now()
	for i := 0; i < 50; i++ {
		b64 = base64.StdEncoding.EncodeToString(bigB)
	}
	msB64e := msSince(t0)

	t0 = time.Now()
	for i := 0; i < 50; i++ {
		dec, _ := base64.StdEncoding.DecodeString(b64)
		_ = string(dec)
	}
	msB64d := msSince(t0)

	t0 = time.Now()
	var u [16]byte
	for i := 0; i < 10000; i++ {
		_, _ = rand.Read(u[:])
		u[6] = (u[6] & 0x0f) | 0x40
		u[8] = (u[8] & 0x3f) | 0x80
		_ = fmt.Sprintf("%08x-%04x-%04x-%04x-%012x", u[0:4], u[4:6], u[6:8], u[8:10], u[10:16])
	}
	msUuid := msSince(t0)

	t0 = time.Now()
	for i := 0; i < 100000; i++ {
		_ = filepath.Clean(filepath.Join("/var/log", "app/../service/output.log"))
	}
	msPath := msSince(t0)

	t0 = time.Now()
	for i := 0; i < 50; i++ {
		if runtime.GOOS == "windows" {
			_, _ = exec.Command("cmd", "/c", "exit", "0").Output()
		} else {
			_, _ = exec.Command("true").Output()
		}
	}
	msProc := msSince(t0)

	items := make([]Item, 10000)
	for i := 0; i < 10000; i++ {
		items[i] = Item{ID: i, User: "alice", Score: i % 100}
	}
	t0 = time.Now()
	rawJSON, _ := json.Marshal(items)
	msJSONEnc := msSince(t0)

	t0 = time.Now()
	var decoded []Item
	_ = json.Unmarshal(rawJSON, &decoded)
	msJSONDec := msSince(t0)

	nums := make([]int64, 1000000)
	for i := 0; i < 1000000; i++ {
		nums[i] = int64(i)
	}
	t0 = time.Now()
	var sum int64
	for _, x := range nums {
		if x%2 == 0 {
			sum += x*3 + 1
		}
	}
	msPipe := msSince(t0)
	_ = sum

	fmt.Printf("BENCH|fs_write_text|%.3f\n", msWriteText)
	fmt.Printf("BENCH|fs_write_lines|%.3f\n", msWriteStream)
	fmt.Printf("BENCH|fs_read_text|%.3f\n", msReadText)
	fmt.Printf("BENCH|fs_read_lines|%.3f\n", msReadLines)
	fmt.Printf("BENCH|fs_each_line|%.3f\n", msEachLine)
	fmt.Printf("BENCH|fs_read_chunk64k|%.3f\n", msReadChunk)
	fmt.Printf("BENCH|crypto_sha256_50m|%.3f\n", msSha)
	fmt.Printf("BENCH|crypto_b64_enc_50m|%.3f\n", msB64e)
	fmt.Printf("BENCH|crypto_b64_dec_50m|%.3f\n", msB64d)
	fmt.Printf("BENCH|crypto_uuid_10k|%.3f\n", msUuid)
	fmt.Printf("BENCH|path_join_norm_100k|%.3f\n", msPath)
	fmt.Printf("BENCH|process_run_50x|%.3f\n", msProc)
	fmt.Printf("BENCH|json_encode_10k|%.3f\n", msJSONEnc)
	fmt.Printf("BENCH|json_decode_10k|%.3f\n", msJSONDec)
	fmt.Printf("BENCH|pipeline_1m|%.3f\n", msPipe)
}
