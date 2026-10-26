package pii

import (
	"bufio"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/testenv"
)

// The tests run in a home of their own; the test binary started again
// with fakeVar set is a stand-in pf-serve (fakeServe).
func TestMain(m *testing.M) {
	if mode := os.Getenv(fakeVar); mode != "" {
		fakeServe(mode)
		return
	}
	testenv.Main(m)
}

const (
	fakeVar = "PII_FAKE_SERVE"
	// fakeLog is a file the stand-in appends what it did to, a line each
	fakeLog = "PII_FAKE_LOG"
)

// fakeServe speaks pf-serve's protocol and finds each "Alice" as a
// person. mode says how else it behaves:
//
//	ok           as pf-serve does
//	gpudies      on the GPU, ends at its first request
//	hang         never answers a request
//	loadhang     never says the model is loaded
//	slow         answers each request after 300ms
//	noload       says it couldn't load the model
//	novulkan     as pf-serve-vulkan, says Vulkan is missing; else ok
func fakeServe(mode string) {
	name := strings.TrimSuffix(filepath.Base(os.Args[0]), ".exe")
	gpu := os.Getenv("PF_DEVICE") != "cpu"
	device := "MTL0"
	if !gpu {
		device = "CPU"
	}
	logf := func(format string, a ...any) {
		if p := os.Getenv(fakeLog); p != "" {
			f, err := os.OpenFile(p, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
			if err == nil {
				fmt.Fprintf(f, format+"\n", a...)
				f.Close()
			}
		}
	}
	logf("start %s %s", name, device)
	logf("path %s", filepath.Base(filepath.Dir(os.Args[0])))
	logf("residency %s", os.Getenv("GGML_METAL_NO_RESIDENCY"))
	if mode == "noload" || mode == "novulkan" && name == "pf-serve-vulkan" {
		json.NewEncoder(os.Stdout).Encode(map[string]string{"error": "can't load " + mode})
		os.Exit(1)
	}
	if mode == "loadhang" {
		time.Sleep(time.Hour)
	}
	json.NewEncoder(os.Stdout).Encode(map[string]any{"device": device, "gpu": gpu, "ms": 7})
	in := bufio.NewReader(os.Stdin)
	out := bufio.NewWriter(os.Stdout)
	u32 := func() (uint32, error) {
		var b [4]byte
		_, err := io.ReadFull(in, b[:])
		return binary.LittleEndian.Uint32(b[:]), err
	}
	put := func(v any) { binary.Write(out, binary.LittleEndian, v) }
	for {
		n, err := u32()
		if err != nil {
			os.Exit(0) // stdin ended: magpie let it go
		}
		texts := make([]string, n)
		for i := range texts {
			l, _ := u32()
			b := make([]byte, l)
			io.ReadFull(in, b)
			texts[i] = string(b)
		}
		logf("request %d %q", n, texts)
		switch {
		case mode == "gpudies" && gpu:
			fmt.Fprintln(os.Stderr, "ggml_metal: device lost")
			os.Exit(3)
		case mode == "hang":
			time.Sleep(time.Hour)
		case mode == "slow":
			time.Sleep(300 * time.Millisecond)
		}
		put(uint32(0))
		for _, t := range texts {
			var at []int
			for i := 0; ; {
				j := strings.Index(t[i:], "Alice")
				if j < 0 {
					break
				}
				at = append(at, i+j)
				i += j + 5
			}
			put(uint32(len(at)))
			for _, a := range at {
				put(int32(a))
				put(int32(a + 5))
				put(float32(0.98))
				put(uint8(len("private_person")))
				out.WriteString("private_person")
			}
		}
		out.Flush()
	}
}

// fakeInstall puts the stand-in where pf-serve goes (and as
// pf-serve-vulkan too, with vulkan), and a model file, in a folder of the
// test's own; it gives the file the stand-in logs to.
func fakeInstall(t *testing.T, mode string, vulkan bool) string {
	t.Helper()
	upgrades.Wait() // one started before reads dir
	d := t.TempDir()
	was, wasSize, wasIdle := dir, modelSize, idleAfter
	dir, modelSize = func() string { return d }, 4
	t.Cleanup(func() {
		Stop()
		// a request Stop failed lets its turn go
		turn <- struct{}{}
		<-turn
		upgrades.Wait()
		disk.mu.Lock()
		if disk.timer != nil {
			disk.timer.Stop()
			disk.timer = nil
		}
		disk.loaded = ""
		disk.mu.Unlock()
		srv.mu.Lock()
		srv.cpu, srv.gpuLost, srv.failed = false, "", ""
		srv.mu.Unlock()
		cache.mu.Lock()
		cache.ll.Init()
		clear(cache.m)
		cache.mu.Unlock()
		dir, modelSize, idleAfter = was, wasSize, wasIdle
	})
	if err := os.WriteFile(modelPath(), []byte("gguf"), 0o644); err != nil {
		t.Fatal(err)
	}
	self, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(self)
	if err != nil {
		t.Fatal(err)
	}
	os.MkdirAll(serveDir(), 0o755)
	names := []string{"pf-serve"}
	if vulkan {
		names = append(names, "pf-serve-vulkan")
	}
	for _, n := range names {
		if err := os.WriteFile(exe(n), b, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	log := filepath.Join(d, "fake.log")
	t.Setenv(fakeVar, mode)
	t.Setenv(fakeLog, log)
	return log
}

// fakeLines are the lines the stand-in logged that start with prefix.
func fakeLines(t *testing.T, log, prefix string) []string {
	t.Helper()
	b, _ := os.ReadFile(log)
	var out []string
	for _, l := range strings.Split(string(b), "\n") {
		if strings.HasPrefix(l, prefix) {
			out = append(out, l)
		}
	}
	return out
}
