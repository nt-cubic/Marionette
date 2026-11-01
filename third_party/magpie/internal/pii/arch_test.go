package pii

import (
	"archive/zip"
	"bytes"
	"net/http/httptest"
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

// noFullX86 holds fullX86 at have for the test.
func noFullX86(t *testing.T, have bool) {
	was := fullX86
	fullX86 = func() bool { return have }
	t.Cleanup(func() { fullX86 = was })
}

// withBase puts the stand-in as pf-serve-base too.
func withBase(t *testing.T) {
	t.Helper()
	b, err := os.ReadFile(exe("pf-serve"))
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(exe("pf-serve-base"), b, 0o755); err != nil {
		t.Fatal(err)
	}
}

// A CPU without AVX2, FMA or BMI2 ended pf-serve with SIGILL on the GPU
// and on the CPU (echo_yip on X): pf-serve-base runs there, on the CPU
// alone, since pf-serve-vulkan is built like pf-serve.
func TestBaseWhereTheCPULacksAVX2(t *testing.T) {
	log := fakeInstall(t, "ok", true)
	withBase(t)
	noFullX86(t, false)
	if _, err := Find([]string{"hi Alice"}); err != nil {
		t.Fatal(err)
	}
	if starts := fakeLines(t, log, "start"); !reflect.DeepEqual(starts, []string{"start pf-serve-base CPU"}) {
		t.Fatalf("starts = %q", starts)
	}
}

// A CPU with all of them never runs pf-serve-base; and a pf-serve from
// before pf-serve-base was built is run as it was.
func TestBaseOnlyWhereNeeded(t *testing.T) {
	t.Run("full CPU", func(t *testing.T) {
		log := fakeInstall(t, "novulkan", true)
		withBase(t)
		noFullX86(t, true)
		if _, err := Find([]string{"hi Alice"}); err != nil {
			t.Fatal(err)
		}
		if starts := fakeLines(t, log, "start"); !reflect.DeepEqual(starts, []string{"start pf-serve-vulkan MTL0", "start pf-serve CPU"}) {
			t.Fatalf("starts = %q", starts)
		}
	})
	t.Run("no base on disk", func(t *testing.T) {
		log := fakeInstall(t, "novulkan", true)
		noFullX86(t, false)
		if _, err := Find([]string{"hi Alice"}); err != nil {
			t.Fatal(err)
		}
		if starts := fakeLines(t, log, "start"); !reflect.DeepEqual(starts, []string{"start pf-serve-vulkan MTL0", "start pf-serve CPU"}) {
			t.Fatalf("starts = %q", starts)
		}
	})
}

// rosettaInstall is an Intel magpie's pf-serve of serveTag (pf-v9), as
// fetched before magpie knew it ran under Rosetta, and from then on under
// Rosetta, Apple silicon's served at want's paths.
func rosettaInstall(t *testing.T, want map[string][]byte) string {
	t.Helper()
	wasArch, wasTr := osArch, translated
	wasTag, wasOld, wasBase, wasServe := serveTag, oldTags, serveBase, serveSHA
	t.Cleanup(func() {
		upgrades.Wait()
		osArch, translated = wasArch, wasTr
		serveTag, oldTags, serveBase, serveSHA = wasTag, wasOld, wasBase, wasServe
	})
	osArch, translated = "darwin-amd64", func() bool { return false }
	serveTag, oldTags = "pf-v9", nil
	log := fakeInstall(t, "ok", false)
	translated = func() bool { return true }
	if target() != "darwin-arm64" {
		t.Fatalf("target = %s under Rosetta", target())
	}
	s := httptest.NewServer(&files{m: want})
	t.Cleanup(s.Close)
	zipSum := sum([]byte("nothing"))
	if z, ok := want["/rel/pf-v9/pf-serve-darwin-arm64.zip"]; ok {
		zipSum = sum(z)
	}
	serveBase, serveSHA = s.URL+"/rel/", map[string]string{"darwin-arm64": zipSum, "darwin-amd64": "x"}
	return log
}

// Under Rosetta, Apple silicon's pf-serve is fetched before the first
// start, run, and the Intel one deleted.
func TestRosettaRunsAppleSiliconsServe(t *testing.T) {
	self, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(self)
	if err != nil {
		t.Fatal(err)
	}
	var zb bytes.Buffer
	zw := zip.NewWriter(&zb)
	w, _ := zw.Create(filepath.Base(exeIn("", "pf-serve")))
	w.Write(b)
	zw.Close()
	log := rosettaInstall(t, map[string][]byte{"/rel/pf-v9/pf-serve-darwin-arm64.zip": zb.Bytes()})
	intel := filepath.Join(dir(), "pf-v9-darwin-amd64")
	if !Installed() {
		t.Fatal("the Intel pf-serve wasn't taken for installed")
	}
	if _, err := Find([]string{"hi Alice"}); err != nil {
		t.Fatal(err)
	}
	if got := fakeLines(t, log, "path "); !reflect.DeepEqual(got, []string{"path pf-v9-darwin-arm64"}) {
		t.Fatalf("started from %q", got)
	}
	upgrades.Wait()
	if _, err := os.Stat(intel); !os.IsNotExist(err) {
		t.Fatalf("the Intel pf-serve was left: %v", err)
	}
}

// One that can't be fetched leaves the Intel one to run.
func TestRosettaKeepsIntelsWhereNoneIsFetched(t *testing.T) {
	log := rosettaInstall(t, map[string][]byte{})
	if _, err := Find([]string{"hi Alice"}); err != nil {
		t.Fatal(err)
	}
	upgrades.Wait()
	if got := fakeLines(t, log, "path "); !reflect.DeepEqual(got, []string{"path pf-v9-darwin-amd64"}) {
		t.Fatalf("started from %q", got)
	}
	if _, err := os.Stat(exeIn(filepath.Join(dir(), "pf-v9-darwin-amd64"), "pf-serve")); err != nil {
		t.Fatalf("the Intel pf-serve was deleted: %v", err)
	}
}
