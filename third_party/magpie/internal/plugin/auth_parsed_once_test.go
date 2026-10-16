package plugin

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/lastgood"
)

// plugin-auth.json is parsed once for each version of the file, however
// many times the sign-ins are asked for: every build of the providers
// asked once for each plugin, and parsed the whole file twice each time
// (#2502). A new version is read at once; one that doesn't parse is its
// last good generation, still unknown rather than none (#1505).
func TestAuthParsedOncePerVersion(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	os.MkdirAll(filepath.Dir(AuthPath()), 0o700)
	write := func(s string) {
		t.Helper()
		if err := os.WriteFile(AuthPath(), []byte(s), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write(`{"fakeco":{"type":"api","key":"one"},"other#2":{"type":"oauth","accountId":"b@x"}}`)
	if a := Auths("fakeco"); a["fakeco"]["key"] != "one" || len(a) != 1 {
		t.Fatalf("Auths = %v", a)
	}
	SignedIn("other")
	n := authParses.Load()
	for range 100 {
		Auths("fakeco")
		Auths("other")
		SignedIn("other")
		listingProxies()
	}
	if got := authParses.Load() - n; got != 0 {
		t.Fatalf("an unchanged plugin-auth.json was parsed %d more times", got)
	}
	// what a caller is given is its own
	Auths("fakeco")["fakeco"]["key"] = "changed"
	if a := Auths("fakeco"); a["fakeco"]["key"] != "one" {
		t.Fatalf("a caller's change reached the next: %v", a)
	}
	// rewritten at once, the same size: read again
	write(`{"fakeco":{"type":"api","key":"two"},"other#2":{"type":"oauth","accountId":"b@x"}}`)
	if a := Auths("fakeco"); a["fakeco"]["key"] != "two" {
		t.Fatalf("after a rewrite Auths = %v", a)
	}
	// zeroed by a crash: the last good generation
	os.WriteFile(lastgood.Bak(AuthPath()), []byte(`{"fakeco":{"type":"api","key":"kept"}}`), 0o600)
	write("\x00\x00\x00\x00")
	if a := Auths("fakeco"); a["fakeco"]["key"] != "kept" {
		t.Fatalf("a zeroed file read as %v, want its last good generation", a)
	}
	if !SignedIn("fakeco") {
		t.Fatal("a zeroed file's sign-ins read as none")
	}
}
