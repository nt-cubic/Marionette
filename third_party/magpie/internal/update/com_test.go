package update

import (
	"bytes"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

// The launcher reads the name comText writes with its own copy of the
// marker (launcher/args.go): the two are one.
func TestComMagicIsTheLaunchers(t *testing.T) {
	b, err := os.ReadFile(filepath.Join("launcher", "args.go"))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(b), `const magic = "\x00magpie-com-target"`) || comMagic != "\x00magpie-com-target" {
		t.Fatal("magpie.com's marker differs between the app and the launcher")
	}
}

// #3737: a shell doesn't wait for the GUI-linked magpie.exe, so magpie
// writes magpie.com (run first by PATHEXT) beside it, naming the app it
// runs; written once, again only when its bytes differ (an update).
func TestWriteCom(t *testing.T) {
	dir := t.TempDir()
	exe := filepath.Join(dir, "magpie-windows-amd64.exe")
	if err := os.WriteFile(exe, []byte("MZ"), 0o755); err != nil {
		t.Fatal(err)
	}
	launcher := []byte("MZ launcher v1")
	p, err := writeCom(launcher, exe)
	if err != nil || p != filepath.Join(dir, "magpie.com") {
		t.Fatalf("writeCom: %q, %v", p, err)
	}
	b, _ := os.ReadFile(p)
	if !bytes.HasPrefix(b, launcher) || comTarget(b) != "magpie-windows-amd64.exe" {
		t.Fatalf("magpie.com: %q", b)
	}
	if !comRuns(p, exe) || comRuns(p, filepath.Join(dir, "magpie.exe")) {
		t.Fatal("magpie.com not read as running this app alone")
	}
	st1, _ := os.Stat(p)
	if _, err := writeCom(launcher, exe); err != nil {
		t.Fatal(err)
	}
	if st2, _ := os.Stat(p); !os.SameFile(st1, st2) {
		t.Fatal("an up-to-date magpie.com written again")
	}
	// the next version's launcher
	if _, err := writeCom([]byte("MZ launcher v2"), exe); err != nil {
		t.Fatal(err)
	}
	if b, _ := os.ReadFile(p); !bytes.HasPrefix(b, []byte("MZ launcher v2")) {
		t.Fatalf("magpie.com not brought up to date: %q", b)
	}
	// one moved aside while it ran goes on the next run
	if err := os.WriteFile(p+".old", []byte("MZ launcher v1"), 0o755); err != nil {
		t.Fatal(err)
	}
	if _, err := writeCom([]byte("MZ launcher v2"), exe); err != nil || exists(p+".old") {
		t.Fatalf("magpie.com.old left: %v", err)
	}
	if left, _ := filepath.Glob(filepath.Join(dir, ".magpie-com-*")); len(left) > 0 {
		t.Fatalf("temp files left: %q", left)
	}
}

func TestComOwner(t *testing.T) {
	dir := t.TempDir()
	dl := filepath.Join(dir, "magpie-windows-amd64.exe")
	if !comOwner(dl) {
		t.Fatal("a lone download doesn't own magpie.com")
	}
	if err := os.WriteFile(filepath.Join(dir, "magpie.exe"), []byte("MZ"), 0o755); err != nil {
		t.Fatal(err)
	}
	if comOwner(dl) {
		t.Fatal("a copy beside magpie.exe takes `magpie` from it")
	}
	if !comOwner(filepath.Join(dir, "Magpie.exe")) {
		t.Fatal("magpie.exe doesn't own its magpie.com")
	}
}

// EnsureCompanion writes where a terminal runs magpie from, and nowhere
// else: a downloads folder off PATH gets nothing.
func TestEnsureCompanion(t *testing.T) {
	old := comLauncher
	t.Cleanup(func() { comLauncher = old })
	sep := string(os.PathListSeparator)

	comLauncher = nil
	dir := t.TempDir()
	exe := filepath.Join(dir, "magpie.exe")
	t.Setenv("PATH", dir)
	if err := EnsureCompanion(exe); err != nil || exists(filepath.Join(dir, comName)) {
		t.Fatalf("a build with no launcher wrote magpie.com: %v", err)
	}

	comLauncher = []byte("MZ launcher")
	off := t.TempDir()
	t.Setenv("PATH", "/x"+sep+dir+string(filepath.Separator))
	if err := EnsureCompanion(filepath.Join(off, "magpie.exe")); err != nil || exists(filepath.Join(off, comName)) {
		t.Fatalf("magpie.com written off PATH: %v", err)
	}
	if err := EnsureCompanion(exe); err != nil || !comRuns(filepath.Join(dir, comName), exe) {
		t.Fatalf("magpie.com not written in a folder on PATH: %v", err)
	}

	// off PATH, but a magpie.cmd says a terminal runs it from there
	shim := t.TempDir()
	dl := filepath.Join(shim, "magpie-windows-arm64.exe")
	if err := os.WriteFile(filepath.Join(shim, shimName), []byte("@echo off\r\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := EnsureCompanion(dl); err != nil || !comRuns(filepath.Join(shim, comName), dl) {
		t.Fatalf("magpie.com not written beside a magpie.cmd: %v", err)
	}
}

// findCLI names the program a shell runs: magpie.com, before magpie.exe
// in the same folder.
func TestFindCLIPrefersCom(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("PATHEXT is Windows'")
	}
	dir := t.TempDir()
	for _, n := range []string{"magpie.exe", "magpie.com"} {
		if err := os.WriteFile(filepath.Join(dir, n), []byte("MZ"), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	if got := findCLI([]string{dir}); !strings.EqualFold(got, filepath.Join(dir, "magpie.com")) {
		t.Fatalf("findCLI: %q", got)
	}
}
