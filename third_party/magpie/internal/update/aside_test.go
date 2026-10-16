package update

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// fakeRename has renameFile fail with err the first n times, then rename.
func fakeRename(t *testing.T, n int, err error) *int {
	calls := 0
	waits, rename := asideWaits, renameFile
	asideWaits = []time.Duration{time.Millisecond, time.Millisecond, time.Millisecond}
	renameFile = func(from, to string) error {
		calls++
		if calls <= n {
			return &os.LinkError{Op: "rename", Old: from, New: to, Err: err}
		}
		return os.Rename(from, to)
	}
	t.Cleanup(func() { asideWaits, renameFile = waits, rename })
	return &calls
}

// Something holding the running exe open for a moment (an antivirus scan,
// OneDrive) doesn't fail the update: moving it aside is tried again.
func TestMoveAsideTriesAgain(t *testing.T) {
	exe := filepath.Join(t.TempDir(), "magpie-windows-amd64.exe")
	os.WriteFile(exe, []byte("v1"), 0o755)
	calls := fakeRename(t, 2, errSharingViolation)
	if _, err := moveAside(exe); err != nil {
		t.Fatal(err)
	}
	if *calls != 3 {
		t.Fatalf("%d tries, want 3", *calls)
	}
	if b, _ := os.ReadFile(exe + ".old"); string(b) != "v1" {
		t.Fatal("not moved to .old")
	}
}

// One that keeps it says why and what to do, without the paths the
// version row has no room for.
func TestMoveAsideSaysWhy(t *testing.T) {
	exe := filepath.Join(t.TempDir(), "magpie-windows-amd64.exe")
	os.WriteFile(exe, []byte("v1"), 0o755)
	calls := fakeRename(t, 100, errSharingViolation)
	_, err := moveAside(exe)
	if err == nil {
		t.Fatal("moved")
	}
	if *calls != 4 {
		t.Fatalf("%d tries, want 4", *calls)
	}
	msg := err.Error()
	for _, want := range []string{"couldn't move magpie-windows-amd64.exe aside", strings.TrimRight(errSharingViolation.Error(), ". 。"), "another program has it open", "download the new version"} {
		if !strings.Contains(msg, want) {
			t.Errorf("%q lacks %q", msg, want)
		}
	}
	if strings.Contains(msg, exe) {
		t.Errorf("%q has the path", msg)
	}

	// a folder magpie may not change says so
	fakeRename(t, 100, os.ErrPermission)
	_, err = moveAside(exe)
	if msg := err.Error(); !strings.Contains(msg, "doesn't let magpie change files in "+filepath.Dir(exe)) {
		t.Errorf("%q", msg)
	}

	// gone is not tried again
	calls = fakeRename(t, 100, os.ErrNotExist)
	moveAside(exe)
	if *calls != 1 {
		t.Fatalf("%d tries of a missing exe", *calls)
	}
}
