package uninstall

import (
	"os"
	"path/filepath"
	"testing"
)

// A magpie installed from a download keeps the profile it had there;
// uninstalling it removes that one too, and never a folder a note names
// outside %APPDATA%.
func TestProfilesTakeTheNotedOne(t *testing.T) {
	appData := t.TempDir()
	own := filepath.Join(appData, "magpie.exe")
	kept := filepath.Join(appData, "magpie-windows-amd64.exe")
	other := filepath.Join(t.TempDir(), "elsewhere")
	for _, p := range []string{own, kept, other} {
		if err := os.MkdirAll(filepath.Join(p, "EBWebView"), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	got := profiles(appData, "magpie.exe", kept)
	if len(got) != 2 || got[0] != own || got[1] != kept {
		t.Fatalf("profiles = %q, want %q and %q", got, own, kept)
	}
	if got := profiles(appData, "magpie.exe", other); len(got) != 1 || got[0] != own {
		t.Fatalf("a note outside AppData: profiles = %q, want only %q", got, own)
	}
	if got := profiles(appData, "magpie.exe", own); len(got) != 1 {
		t.Fatalf("the note naming the exe's own: profiles = %q, want it once", got)
	}
	plain := filepath.Join(appData, "plain")
	os.MkdirAll(plain, 0o755)
	if got := profiles(appData, "nothing.exe", plain); len(got) != 0 {
		t.Fatalf("folders WebView2 didn't make: profiles = %q, want none", got)
	}
}
