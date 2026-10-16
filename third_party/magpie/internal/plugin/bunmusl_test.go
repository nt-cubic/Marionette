package plugin

import (
	"context"
	"errors"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

// stubPlatform has bunTarget and bunRoot see the machine given.
func stubPlatform(t *testing.T, goos, goarch string, musl bool) {
	t.Helper()
	old := bunPlatform
	bunPlatform = func() (string, string, bool) { return goos, goarch, musl }
	t.Cleanup(func() { bunPlatform = old })
}

// A musl Linux (Alpine) gets Bun's musl build: a glibc one names a loader
// Alpine doesn't have and can't start there (#1877). x64 keeps the
// baseline build, arm64 has none; musl changes nothing off Linux.
func TestBunTargetForMusl(t *testing.T) {
	for _, c := range []struct {
		goos, goarch string
		musl         bool
		want         string
	}{
		{"linux", "amd64", true, "bun-linux-x64-musl-baseline"},
		{"linux", "arm64", true, "bun-linux-aarch64-musl"},
		{"linux", "amd64", false, "bun-linux-x64-baseline"},
		{"linux", "arm64", false, "bun-linux-aarch64"},
		{"darwin", "arm64", true, "bun-darwin-aarch64"},
		{"windows", "amd64", true, "bun-windows-x64-baseline"},
		{"windows", "arm64", false, "bun-windows-aarch64"},
		{"android", "arm64", true, "bun-linux-aarch64-android"},
	} {
		got, err := bunTargetFor(c.goos, c.goarch, c.musl)
		if err != nil || got != c.want {
			t.Errorf("bunTargetFor(%s, %s, musl %v) = %q, %v; want %q", c.goos, c.goarch, c.musl, got, err, c.want)
		}
		if bunChecksum(BunVersion, got) == "" {
			t.Errorf("no checksum for %s", got)
		}
	}
}

// One version is a different build on glibc and on musl, so each keeps
// its own: a machine whose C library changed (a container moved from
// Debian to Alpine) downloads the musl build rather than run the glibc one
// it has, nor reads the glibc one's state. A glibc machine keeps the Bun
// it downloaded before, where it was, with no new download.
func TestMuslBunKeptApartFromGlibc(t *testing.T) {
	bunHome(t, BunVersion, time.Now())
	stubPlatform(t, "linux", "amd64", false)
	glibc := bunExeOf(BunVersion)
	if err := os.MkdirAll(filepath.Dir(glibc), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(glibc, []byte("glibc build"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := writeBunState(bunState{Bad: []string{"9.9.9"}}); err != nil {
		t.Fatal(err)
	}
	if rel, _ := filepath.Rel(filepath.Dir(bunRoot()), glibc); rel != filepath.Join("bun", BunVersion, magpieBun()) {
		t.Fatalf("a glibc Bun is kept at %s, not where it was before", glibc)
	}

	stubPlatform(t, "linux", "amd64", true)
	if HasBun() {
		t.Fatal("HasBun on musl with only the glibc build on disk")
	}
	if s := readBunState(); len(s.Bad) != 0 {
		t.Fatalf("musl reads the glibc build's state: %+v", s)
	}
	asked := bunReleases(t, []byte("musl build"))
	exe, err := Bun(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if b, _ := os.ReadFile(exe); string(b) != "musl build" || asked.Load() != 1 || exe == glibc {
		t.Fatalf("musl Bun = %s holding %q after %d downloads", exe, b, asked.Load())
	}

	stubPlatform(t, "linux", "amd64", false)
	exe, err = Bun(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if b, _ := os.ReadFile(exe); exe != glibc || string(b) != "glibc build" || asked.Load() != 1 {
		t.Fatalf("glibc Bun = %s holding %q after %d downloads", exe, b, asked.Load())
	}
	if s := readBunState(); len(s.Bad) != 1 {
		t.Fatalf("glibc state = %+v", s)
	}
}

// unstartable is a bun on disk that exec can't start with "no such file
// or directory": the interpreter it names isn't there, as glibc's loader
// isn't on Alpine.
func unstartable(t *testing.T, path string) {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("Windows has no interpreter line")
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("#!/lib64/no-such-ld-linux-x86-64.so.2\n"), 0o755); err != nil {
		t.Fatal(err)
	}
}

// A bun that is there but can't be started is said to be a build for
// another C library, with MAGPIE_BUN as the way round, not left reading
// as a missing file. A bun that really is missing, and any other error,
// stay as they are.
func TestCantStartSaysCLibrary(t *testing.T) {
	bunHome(t, BunVersion, time.Now())
	bun := filepath.Join(t.TempDir(), "magpie-bun")
	unstartable(t, bun)
	err := exec.Command(bun).Start()
	if !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("starting it: %v, want ENOENT", err)
	}
	got := cantStart(bun, err)
	if msg := got.Error(); !strings.Contains(msg, "C library") || !strings.Contains(msg, "MAGPIE_BUN") || !strings.Contains(msg, bun) || !errors.Is(got, fs.ErrNotExist) {
		t.Fatalf("cantStart = %v", got)
	}

	t.Setenv("MAGPIE_BUN", bun)
	if msg := cantStart(bun, err).Error(); !strings.Contains(msg, "$MAGPIE_BUN") || !strings.Contains(msg, "C library") {
		t.Fatalf("cantStart of $MAGPIE_BUN = %s", msg)
	}

	missing := filepath.Join(t.TempDir(), "bun")
	merr := exec.Command(missing).Start()
	if got := cantStart(missing, merr); got != merr {
		t.Fatalf("cantStart of a missing bun = %v, want %v", got, merr)
	}
	other := errors.New("exit status 3")
	if got := cantStart(bun, other); got != other {
		t.Fatalf("cantStart of another error = %v", got)
	}
}

// The plugin host on a downloaded Bun that can't be started says why, and
// the Bun isn't set aside: the one before is the same build for this
// system, so falling back can't help and would mark a good version bad.
func TestHostOnUnstartableBunSaysCLibrary(t *testing.T) {
	bunHome(t, "9.9.9", time.Now().Add(-72*time.Hour))
	t.Cleanup(Settle)
	unstartable(t, bunExeOf("9.9.9"))
	unstartable(t, bunExeOf(BunVersion))
	if err := writeBunState(bunState{Current: "9.9.9", Previous: BunVersion}); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("testdata/fake/index.js")
	if _, err := Add(ctx, abs); err != nil && !strings.Contains(err.Error(), "C library") {
		t.Fatal(err)
	}
	_, err := Providers(ctx)
	if err == nil || !strings.Contains(err.Error(), "C library") || !strings.Contains(err.Error(), "MAGPIE_BUN") {
		t.Fatalf("Providers on an unstartable Bun: %v", err)
	}
	if s := readBunState(); s.Current != "9.9.9" || len(s.Bad) != 0 {
		t.Fatalf("state %+v after an unstartable Bun", s)
	}
}

// alpineLoader is what musl's loader printed on a bare Alpine 3.20 when
// asked to run Bun 1.4.2's musl build (bun --version), cut after a few of
// its 40 "Error relocating" lines.
const alpineLoader = `Error loading shared library libstdc++.so.6: No such file or directory (needed by /sbx/.cache/magpie/bun/musl/1.4.2/magpie-bun)
Error relocating /sbx/.cache/magpie/bun/musl/1.4.2/magpie-bun: _ZSt15get_new_handlerv: symbol not found
Error relocating /sbx/.cache/magpie/bun/musl/1.4.2/magpie-bun: _ZSt24__throw_out_of_range_fmtPKcz: symbol not found
Error relocating /sbx/.cache/magpie/bun/musl/1.4.2/magpie-bun: __once_proxy: symbol not found
`

// lacksLibstdcxx is a bun as Bun's musl build is on a bare Alpine: the
// loader says what it lacks and exits 127.
func lacksLibstdcxx(t *testing.T, path string) {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("the bun is a shell script")
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	script := "#!/bin/sh\ncat >&2 <<'OUT'\n" + alpineLoader + "OUT\nexit 127\n"
	if err := os.WriteFile(path, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
}

// Bun's musl build on an Alpine without libstdc++ says which library is
// missing and how to add it, where the loader's first line was cut by its
// screenful of relocation errors; other output says nothing.
func TestMissingLibsNamesThem(t *testing.T) {
	got := missingLibs(alpineLoader)
	if !strings.Contains(got, "libstdc++.so.6") || !strings.Contains(got, "apk add libstdc++") || !strings.Contains(got, "MAGPIE_BUN") {
		t.Fatalf("missingLibs = %q", got)
	}
	if got := missingLibs("error: GET https://registry.npmjs.org/x - 404\n"); got != "" {
		t.Fatalf("missingLibs of a 404 = %q", got)
	}

	bunHome(t, BunVersion, time.Now())
	lacksLibstdcxx(t, bunExeOf(BunVersion))
	err := install(context.Background(), "@magpie-community/opencode-cursor-auth@latest")
	if err == nil || !strings.Contains(err.Error(), "libstdc++.so.6") || !strings.Contains(err.Error(), "apk add") {
		t.Fatalf("bun add on a bun lacking libstdc++: %v", err)
	}
}

// The host on a bun lacking a library says which, and the Bun isn't set
// aside for it.
func TestHostOnBunLackingLibsSaysWhich(t *testing.T) {
	bunHome(t, BunVersion, time.Now())
	t.Cleanup(Settle)
	lacksLibstdcxx(t, bunExeOf(BunVersion))
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("testdata/fake/index.js")
	if _, err := Add(ctx, abs); err != nil && !strings.Contains(err.Error(), "libstdc++") {
		t.Fatal(err)
	}
	_, err := Providers(ctx)
	if err == nil || !strings.Contains(err.Error(), "libstdc++.so.6") || !strings.Contains(err.Error(), "apk add") {
		t.Fatalf("Providers on a bun lacking libstdc++: %v", err)
	}
	if s := readBunState(); len(s.Bad) != 0 {
		t.Fatalf("state %+v", s)
	}
}
