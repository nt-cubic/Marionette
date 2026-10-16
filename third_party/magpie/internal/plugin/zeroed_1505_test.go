package plugin

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/lastgood"
)

// zeroedAuth is plugin-auth.json as #1505's Windows crash left it: its full
// length, 29,926 bytes, every one 0x00.
var zeroedAuth = make([]byte, 29926)

func addRenew(t *testing.T, ctx context.Context) {
	t.Helper()
	t.Setenv("FAKE_BASE", "http://127.0.0.1:1/v1")
	t.Setenv("RENEW_LOG", filepath.Join(t.TempDir(), "renewals"))
	abs, _ := filepath.Abs("testdata/renew/index.js")
	if _, err := Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
}

func importRenew(t *testing.T, ctx context.Context, who string) string {
	t.Helper()
	k, err := Import(ctx, "renewco", map[string]any{"type": "oauth", "refresh": "r-ok-" + who, "access": "a", "expires": time.Now().Add(time.Hour).UnixMilli(), "accountId": who})
	if err != nil {
		t.Fatal(err)
	}
	return k
}

// The host reads a zeroed plugin-auth.json from its last good generation,
// tells magpie, which tells the user, and a sign-in added keeps the
// accounts and the zeroed file aside.
func TestZeroedPluginAuthReadFromBackup(t *testing.T) {
	sandbox(t)
	lastgood.Reset()
	t.Cleanup(lastgood.Reset)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	addRenew(t, ctx)
	a := importRenew(t, ctx, "a@renew")
	b := importRenew(t, ctx, "b@renew")
	Restart()
	if _, err := os.Stat(lastgood.Bak(AuthPath())); err != nil {
		t.Fatalf("no last good generation kept: %v", err)
	}
	if err := os.WriteFile(AuthPath(), zeroedAuth, 0o600); err != nil {
		t.Fatal(err)
	}

	// magpie's own reads: the .bak lags one write, so it holds a
	if got := Auths("renewco"); len(got) != 1 || got[a] == nil {
		t.Fatalf("a zeroed plugin-auth.json read as %v", got)
	}
	if len(lastgood.Recovered()) != 1 {
		t.Fatalf("magpie's read not noted: %+v", lastgood.Recovered())
	}
	lastgood.Reset() // the host's own read is told magpie too

	// the host's
	c := importRenew(t, ctx, "c@renew")
	all := authFile(t)
	if all[a] == nil || all[c] == nil {
		t.Fatalf("after a sign-in added over a zeroed file: %v", all)
	}
	_ = b // the write the crash took
	kept, _ := filepath.Glob(AuthPath() + ".bad-*")
	if len(kept) != 1 {
		t.Fatalf("the zeroed file not kept aside: %v", kept)
	}
	if got, _ := os.ReadFile(kept[0]); !bytes.Equal(got, zeroedAuth) {
		t.Fatalf("kept %d bytes", len(got))
	}
	for wait := time.Now().Add(5 * time.Second); len(lastgood.Recovered()) == 0 && time.Now().Before(wait); {
		time.Sleep(20 * time.Millisecond)
	}
	if n := lastgood.Recovered(); len(n) != 1 || n[0].File != "plugin-auth.json" || n[0].Why != "29926 bytes, every one zero" {
		t.Fatalf("the host's recovery isn't told: %+v", n)
	}
}

// With no backup, a zeroed plugin-auth.json is unknown, never none: a
// sign-out doesn't write none over it, magpie's or the host's.
func TestZeroedPluginAuthWithoutBackupNotWrittenOver(t *testing.T) {
	sandbox(t)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	addRenew(t, ctx)
	if _, err := get(ctx); err != nil {
		t.Fatal(err)
	}
	os.MkdirAll(filepath.Dir(AuthPath()), 0o700)
	os.WriteFile(AuthPath(), zeroedAuth, 0o600)
	// one account named, and every one of the provider's
	for _, account := range []string{"renewco#a@renew", ""} {
		if err := SignOut(ctx, "renewco", account); err != nil {
			t.Fatal(err)
		}
		if got, _ := os.ReadFile(AuthPath()); !bytes.Equal(got, zeroedAuth) {
			t.Fatalf("the host's sign-out of %q wrote %q over sign-ins that don't read", account, got)
		}
	}
	Restart()
	if err := SignOut(ctx, "renewco", ""); err == nil {
		t.Fatal("magpie's sign-out of sign-ins that don't read said done")
	}
	if got, _ := os.ReadFile(AuthPath()); !bytes.Equal(got, zeroedAuth) {
		t.Fatalf("magpie's sign-out wrote %q over sign-ins that don't read", got)
	}
	// a sign-in added writes, the zeroed file kept aside first
	k := importRenew(t, ctx, "new@renew")
	if authFile(t)[k] == nil {
		t.Fatal("the sign-in added wasn't kept")
	}
	if kept, _ := filepath.Glob(AuthPath() + ".bad-*"); len(kept) != 1 {
		t.Fatalf("the zeroed file not kept aside: %v", kept)
	}
}

// The host flushes plugin-auth.json before renaming it over the old: a
// flush that fails leaves the sign-ins there as they were.
func TestPluginAuthSyncedBeforeRename(t *testing.T) {
	sandbox(t)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	flag := filepath.Join(t.TempDir(), "fail")
	t.Setenv("FSYNC_FAIL", flag)
	abs, _ := filepath.Abs("testdata/nofsync/index.js")
	if _, err := Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if _, err := Import(ctx, "syncco", map[string]any{"type": "api", "key": "k-old"}); err != nil {
		t.Fatal(err)
	}
	before, err := os.ReadFile(AuthPath())
	if err != nil {
		t.Fatal(err)
	}
	os.WriteFile(flag, nil, 0o600)
	if _, err := Import(ctx, "syncco", map[string]any{"type": "api", "key": "k-new"}); err == nil {
		t.Fatal("a sign-in saved without its flush said done")
	}
	if after, _ := os.ReadFile(AuthPath()); !bytes.Equal(after, before) {
		t.Fatalf("a failed flush changed plugin-auth.json:\n%s\nwas\n%s", after, before)
	}
	if left, _ := filepath.Glob(AuthPath() + ".tmp-*"); len(left) != 0 {
		t.Fatalf("a failed write left %v", left)
	}
}
