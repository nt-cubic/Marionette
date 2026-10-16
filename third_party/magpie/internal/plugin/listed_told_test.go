package plugin

import (
	"os"
	"path/filepath"
	"sync/atomic"
	"testing"

	"github.com/yetone/magpie/internal/settings"
)

// A list the plugins answer with that is another than the one kept is
// told (OnListed), so the model lists agents keep in their own files take
// a plugin's models in once its host has listed them, after a sign-in
// (#2373: Pi's models.json had none of Trae CN's). The same list again,
// or the one an earlier run kept on disk, is no change and isn't told.
func TestNewListIsTold(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("HOME", dir)
	t.Setenv("USERPROFILE", dir)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(dir, "config"))
	if err := os.MkdirAll(settings.Dir(), 0o700); err != nil {
		t.Fatal(err)
	}
	UseCached(nil)
	t.Cleanup(func() { Told(); UseCached(nil) })
	var n atomic.Int64
	onListedMu.Lock()
	was := onListed
	onListed = []func(){func() { n.Add(1) }}
	onListedMu.Unlock()
	t.Cleanup(func() { onListedMu.Lock(); onListed = was; onListedMu.Unlock() })
	told := func(want int64, what string) {
		t.Helper()
		Told()
		if got := n.Load(); got != want {
			t.Fatalf("%s: told %d times, want %d", what, got, want)
		}
	}
	signedOut := []Provider{{ID: "trae-cn", Spec: "@x/trae", Name: "Trae CN"}}
	commitProviders(signedOut, nil)
	told(1, "the first list")
	commitProviders(signedOut, nil)
	told(1, "the same list again")
	listed := []Provider{{ID: "trae-cn", Spec: "@x/trae", Name: "Trae CN", Models: []Model{{ID: "kimi-k3-max", Name: "Kimi-K3 (Max)"}}}}
	commitProviders(listed, nil)
	told(2, "the account's models listed")
	// a start, nothing kept in memory, lists what the last run kept
	UseCached(nil)
	commitProviders(listed, nil)
	told(2, "the list kept on disk, listed again")
}
