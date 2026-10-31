package update

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"testing"
)

// desktopFixture is magpie.exe on a Desktop, with magpie's cache
// elsewhere and a release asset to download.
type desktopFixture struct {
	desktop, exe, cache string
	asset               Asset
	body                []byte
}

func newDesktopFixture(t *testing.T) *desktopFixture {
	t.Helper()
	root := t.TempDir()
	f := &desktopFixture{
		desktop: filepath.Join(root, "Desktop"),
		cache:   filepath.Join(root, "Local", "magpie"),
		body:    []byte("MZ the next version"),
	}
	f.exe = filepath.Join(f.desktop, "magpie.exe")
	if err := os.MkdirAll(f.desktop, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(f.exe, []byte("MZ this version"), 0o755); err != nil {
		t.Fatal(err)
	}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.Write(f.body) }))
	t.Cleanup(srv.Close)
	sum := sha256.Sum256(f.body)
	f.asset = Asset{URL: srv.URL, SHA256: hex.EncodeToString(sum[:])}
	was, wasStart := stageCache, canStart
	t.Cleanup(func() { stageCache, canStart = was, wasStart })
	stageCache = func() (string, error) { return f.cache, nil }
	canStart = func(string) error { return nil }
	return f
}

// what is in the Desktop besides magpie.exe and what an update moved
// aside (removed at the next start, RemoveOld)
func (f *desktopFixture) beside(t *testing.T) []string {
	t.Helper()
	es, err := os.ReadDir(f.desktop)
	if err != nil {
		t.Fatal(err)
	}
	var out []string
	for _, e := range es {
		if n := e.Name(); n != "magpie.exe" && n != "magpie.exe.old" {
			out = append(out, n)
		}
	}
	return out
}

// An update waiting for the next restart is downloaded to magpie's cache,
// not beside the program as magpie.exe.new, which showed on a Desktop and
// came back after it was deleted (nianshou5 on Discord). The swap still
// puts it in, and leaves nothing of the download behind.
func TestUpdateStagedOutOfTheProgramsFolder(t *testing.T) {
	f := newDesktopFixture(t)
	staged, err := stageBinaryFor(context.Background(), f.asset, f.exe)
	if err != nil {
		t.Fatal(err)
	}
	if got := f.beside(t); len(got) != 0 {
		t.Fatalf("beside magpie.exe while the update waits: %v", got)
	}
	if !inFolder(staged, f.cache) {
		t.Fatalf("staged at %s, not in the cache %s", staged, f.cache)
	}
	if _, err := InstallBinaryAside(staged, f.exe); err != nil {
		t.Fatal(err)
	}
	if b, _ := os.ReadFile(f.exe); string(b) != string(f.body) {
		t.Fatalf("magpie.exe after the swap: %q", b)
	}
	if got := f.beside(t); len(got) != 0 {
		t.Fatalf("beside magpie.exe after the swap: %v", got)
	}
	if es, _ := os.ReadDir(filepath.Join(f.cache, "update")); len(es) != 0 {
		t.Fatalf("left in the cache: %v", es)
	}
}

// With no cache a rename reaches (another volume), the download goes into
// a hidden folder beside the program, which the swap removes.
func TestUpdateStagedHiddenWithoutTheCache(t *testing.T) {
	f := newDesktopFixture(t)
	stageCache = func() (string, error) { return "", errors.New("no cache") }
	staged, err := stageBinaryFor(context.Background(), f.asset, f.exe)
	if err != nil {
		t.Fatal(err)
	}
	if want := filepath.Join(f.desktop, besideStage, "magpie.exe.new"); staged != want {
		t.Fatalf("staged at %s, want %s", staged, want)
	}
	if got := f.beside(t); !slices.Equal(got, []string{besideStage}) {
		t.Fatalf("beside magpie.exe while the update waits: %v", got)
	}
	if !hiddenForTest(filepath.Join(f.desktop, besideStage)) {
		t.Fatal(besideStage + " isn't hidden")
	}
	if _, err := InstallBinaryAside(staged, f.exe); err != nil {
		t.Fatal(err)
	}
	if got := f.beside(t); len(got) != 0 {
		t.Fatalf("beside magpie.exe after the swap: %v", got)
	}
}

// A download a failed restart keeps for the next try stays where it was
// staged, out of sight; a failed download leaves nothing.
func TestFailedDownloadLeavesNothing(t *testing.T) {
	f := newDesktopFixture(t)
	quickRetries(t)
	f.asset.SHA256 = "00"
	if _, err := stageBinaryFor(context.Background(), f.asset, f.exe); err == nil {
		t.Fatal("a download that doesn't match its checksum was staged")
	}
	if got := f.beside(t); len(got) != 0 {
		t.Fatalf("beside magpie.exe: %v", got)
	}
	if es, _ := os.ReadDir(filepath.Join(f.cache, "update")); len(es) != 0 {
		t.Fatalf("left in the cache: %v", es)
	}
}

// What an earlier version downloaded beside the program (magpie.exe.new)
// goes at the next start once it is no newer than the program: an older
// version, or this one over again. A newer one, which a magpie still
// running may put in, stays.
func TestStartClearsAnOlderDownloadBeside(t *testing.T) {
	f := newDesktopFixture(t)
	saved := fileVersion
	t.Cleanup(func() { fileVersion = saved })
	versions := map[string]uint64{f.exe: 5}
	fileVersion = func(p string) uint64 { return versions[p] }
	left := f.exe + ".new"
	os.WriteFile(left, []byte("MZ an older one"), 0o755)
	versions[left] = 4
	RemoveStaleNew(f.exe)
	if exists(left) {
		t.Fatal("an older magpie.exe.new was kept")
	}
	os.WriteFile(left, []byte("MZ a newer one"), 0o755)
	versions[left] = 6
	RemoveStaleNew(f.exe)
	if !exists(left) {
		t.Fatal("a newer magpie.exe.new was removed")
	}
	// the same in the cache
	staged, err := stageBinaryFor(context.Background(), f.asset, f.exe)
	if err != nil {
		t.Fatal(err)
	}
	versions[staged] = 5
	RemoveStaleNew(f.exe)
	if exists(staged) || exists(filepath.Dir(staged)) {
		t.Fatal("a download of this version was kept in the cache")
	}
}

// A start of a magpie on the Desktop doesn't put magpie.com back there
// once deleted, though the Desktop is on PATH; one there is kept up to
// date, and Add (cliPlace's writeCom) still writes it.
func TestNoCompanionPutBackOnTheDesktop(t *testing.T) {
	f := newDesktopFixture(t)
	was, wasDL, wasTo := comLauncher, downloadFolder, installTarget
	t.Cleanup(func() { comLauncher, downloadFolder, installTarget = was, wasDL, wasTo })
	comLauncher = []byte("MZ launcher")
	downloadFolder = func(dir string) bool { return samePath(dir, f.desktop) }
	installTarget = func() string { return "" } // runs where it is (portable, MAGPIE_NO_INSTALL)
	t.Setenv("PATH", f.desktop)
	com := filepath.Join(f.desktop, comName)
	if err := EnsureCompanion(f.exe); err != nil || exists(com) {
		t.Fatalf("magpie.com written on the Desktop: %v", err)
	}
	if _, err := writeCom([]byte("MZ older launcher"), f.exe); err != nil {
		t.Fatal(err)
	}
	if err := EnsureCompanion(f.exe); err != nil || !comRuns(com, f.exe) {
		t.Fatalf("magpie.com there not kept: %v", err)
	}
	if b, _ := os.ReadFile(com); string(b) != string(comText(comLauncher, f.exe)) {
		t.Fatal("magpie.com there not brought up to date")
	}
	os.Remove(com)
	if err := EnsureCompanion(f.exe); err != nil || exists(com) {
		t.Fatalf("a deleted magpie.com came back: %v", err)
	}
	// a folder of the user's own on PATH is as before
	other := filepath.Join(t.TempDir(), "magpie.exe")
	t.Setenv("PATH", filepath.Dir(other))
	if err := EnsureCompanion(other); err != nil || !exists(filepath.Join(filepath.Dir(other), comName)) {
		t.Fatalf("magpie.com not written in a folder of the user's on PATH: %v", err)
	}
}
