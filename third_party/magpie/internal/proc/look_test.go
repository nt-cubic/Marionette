package proc

import (
	"os"
	"path/filepath"
	"runtime"
	"sync"
	"testing"
	"time"
)

// installTool writes an executable name into dir, as an installer would.
func installTool(t *testing.T, dir, name string) string {
	t.Helper()
	if runtime.GOOS == "windows" {
		name += ".exe"
	}
	p := filepath.Join(dir, name)
	if err := os.WriteFile(p, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	return p
}

// lookWindowFor sets the window a test binary trusts the folders' stamp
// for, as the app does (lookFresh), and puts it back after.
func lookWindowFor(t *testing.T, d time.Duration) {
	old := lookFreshInTests
	lookFreshInTests = d
	Forget()
	t.Cleanup(func() { lookFreshInTests = old; Forget() })
}

// A command looked up again while PATH's folders are unchanged isn't
// looked for again: every page asked once per agent and once per Pi
// session, each miss 111ms of stats on a Windows PATH of 47 folders, so
// pages took seconds and Sessions half a minute (#1771).
func TestLookPathAsksOnceWhileFoldersAreUnchanged(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("PATH", dir)
	// within the app's window, and past it with the folders unchanged
	for _, window := range []time.Duration{lookFresh, 0} {
		lookWindowFor(t, window)
		asks := 0
		for i := 0; i < 300; i++ {
			looked("look\x00omp", func() lookAnswer { asks++; return lookAnswer{} })
		}
		if asks != 1 {
			t.Fatalf("window %v: 300 lookups of one command asked %d times, want 1", window, asks)
		}
	}
}

// A tool installed or removed is seen at the next lookup once the stamp is
// taken again: its folder's modification time changed.
func TestLookPathSeesAnInstallAndARemoval(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("PATH", dir)
	lookWindowFor(t, 0)
	if p, err := LookPath("magpie-fake-agent"); err == nil {
		t.Fatalf("found %q before it was installed", p)
	}
	if p := FindTool("magpie-fake-agent"); p != "" {
		t.Fatalf("FindTool found %q before it was installed", p)
	}
	time.Sleep(20 * time.Millisecond) // a folder time a coarse clock tells apart
	want := installTool(t, dir, "magpie-fake-agent")
	if p, err := LookPath("magpie-fake-agent"); err != nil || p != want {
		t.Fatalf("after installing: %q, %v; want %q", p, err, want)
	}
	if p := FindTool("magpie-fake-agent"); p != want {
		t.Fatalf("FindTool after installing: %q, want %q", p, want)
	}
	time.Sleep(20 * time.Millisecond)
	os.Remove(want)
	if p, err := LookPath("magpie-fake-agent"); err == nil {
		t.Fatalf("after removing it, still found %q", p)
	}
	if p := FindTool("magpie-fake-agent"); p != "" {
		t.Fatalf("FindTool after removing it, still found %q", p)
	}
}

// Within the app's window an answer is kept without a stat; a changed PATH
// is asked about at once, and so is everything after Forget (the Agents
// page's "Look again").
func TestLookPathWindowPathAndForget(t *testing.T) {
	a, b := t.TempDir(), t.TempDir()
	t.Setenv("PATH", a)
	lookWindowFor(t, time.Hour)
	if _, err := LookPath("magpie-fake-agent"); err == nil {
		t.Fatal("found before installing")
	}
	inA := installTool(t, a, "magpie-fake-agent")
	if p, err := LookPath("magpie-fake-agent"); err == nil {
		t.Fatalf("within the window the miss is kept, found %q", p)
	}
	Forget()
	if p, err := LookPath("magpie-fake-agent"); err != nil || p != inA {
		t.Fatalf("after Forget: %q, %v; want %q", p, err, inA)
	}

	inB := installTool(t, b, "magpie-fake-agent")
	t.Setenv("PATH", b+string(os.PathListSeparator)+a)
	if p, err := LookPath("magpie-fake-agent"); err != nil || p != inB {
		t.Fatalf("after PATH changed: %q, %v; want %q", p, err, inB)
	}
}

// Lookups from many requests at once, with Forget among them, are safe.
func TestLookPathConcurrent(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("PATH", dir)
	installTool(t, dir, "magpie-fake-agent")
	lookWindowFor(t, 0)
	var wg sync.WaitGroup
	for i := 0; i < 16; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			for j := 0; j < 50; j++ {
				if i == 0 && j%10 == 0 {
					Forget()
				}
				if _, err := LookPath("magpie-fake-agent"); err != nil {
					t.Error(err)
					return
				}
				FindTool("magpie-no-such-tool")
			}
		}(i)
	}
	wg.Wait()
}
