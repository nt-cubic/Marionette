package plugin

import (
	"context"
	"path/filepath"
	"testing"
	"time"
)

// StringKe on Discord: the Antigravity plugin can't sign in. An OAuth
// method that asks nothing is called with no inputs, as OpenCode's TUI
// calls it; opencode-antigravity-auth, handed an empty object, took it for
// OpenCode's CLI and waited on a "Project ID" prompt on magpie's stdin, the
// sign-in's page never given.
func TestAuthorizeNoPrompts(t *testing.T) {
	sandbox(t)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("testdata/cliflow/index.js")
	if _, err := Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	actx, acancel := context.WithTimeout(ctx, 15*time.Second)
	defer acancel()
	a, err := Authorize(actx, "cliflow", 0, map[string]string{}, NewAccount)
	if err != nil || a.URL != "https://cliflow.invalid/auth" || a.Method != "code" {
		t.Fatalf("Authorize = %+v, %v", a, err)
	}
	if got, err := Finish(ctx, a.Session, "good"); err != nil || got != (Saved{"cliflow", "cliflow"}) {
		t.Fatalf("Finish = %+v, %v", got, err)
	}
}
