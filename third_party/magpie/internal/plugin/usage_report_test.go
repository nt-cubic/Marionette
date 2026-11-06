package plugin

import (
	"context"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"
)

// A plugin reports a request it sent its vendor on its own (Devin's cache
// keepalive ping, plugins#592) through input.magpie.usage, from its own
// timer: magpie is told its provider, the account it names (one of the
// provider's; else the request's in hand, else the provider's own id) and
// the call, its tokens whole numbers from 0. A call with no provider, no
// kind of [a-z0-9_-] or no model throws, and nothing is told of it.
func TestPluginReportsOwnUsage(t *testing.T) {
	sandbox(t)
	type told struct {
		provider, account string
		u                 OwnCall
	}
	var mu sync.Mutex
	var got []told
	OnUsage(func(provider, account string, u OwnCall) {
		mu.Lock()
		got = append(got, told{provider, account, u})
		mu.Unlock()
	})
	t.Cleanup(func() { OnUsage(nil) })
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	dir := filepath.Join(t.TempDir(), "usageco")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	src := `export default async (input) => {
  let threw = 0
  for (const bad of [[5, { kind: "x", model: "m" }], ["usageco", null], ["usageco", [1]], ["usageco", { model: "m" }], ["usageco", { kind: "Bad Kind", model: "m" }], ["usageco", { kind: "cache_keepalive" }], ["", { kind: "x", model: "m" }]]) {
    try { input.magpie.usage(...bad) } catch (e) { if (e instanceof TypeError) threw++ }
  }
  setTimeout(() => {
    input.magpie.usage("usageco", { kind: "cache_keepalive", model: " sonnet ", input: 3.7, output: 1, cacheRead: 10984, cacheWrite: -5, status: 200, ms: 812, account: "usageco#ab" })
    input.magpie.usage("usageco", { kind: "cache_keepalive", model: "sonnet", input: NaN, output: "1", cacheRead: Infinity, status: 0, error: "fetch failed", account: "other#cd" })
    input.magpie.usage("usageco", { kind: "threw", model: String(threw) })
  }, 50)
  return {}
}
`
	if err := os.WriteFile(filepath.Join(dir, "index.js"), []byte(src), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := Add(ctx, filepath.Join(dir, "index.js")); err != nil {
		t.Fatal(err)
	}
	if ls, err := Plugins(ctx); err != nil || len(ls) != 1 || ls[0].Error != "" {
		t.Fatalf("Plugins = %+v, %v", ls, err)
	}
	for {
		mu.Lock()
		n := len(got)
		mu.Unlock()
		if n >= 3 {
			break
		}
		if ctx.Err() != nil {
			t.Fatalf("the plugin's calls never arrived: %+v", got)
		}
		time.Sleep(10 * time.Millisecond)
	}
	Told()
	mu.Lock()
	defer mu.Unlock()
	if len(got) != 3 {
		t.Fatalf("told %d calls, want 3: %+v", len(got), got)
	}
	byModel := map[string]told{}
	for _, x := range got {
		if x.u.Kind == "threw" {
			if x.u.Model != "7" {
				t.Errorf("%s calls threw, want 7", x.u.Model)
			}
			continue
		}
		if x.u.Error == "" {
			byModel["ok"] = x
		} else {
			byModel["failed"] = x
		}
	}
	ok, failed := byModel["ok"], byModel["failed"]
	if want := (told{"usageco", "usageco#ab", OwnCall{Kind: "cache_keepalive", Model: "sonnet", Input: 3, Output: 1, CacheRead: 10984, Status: 200, Millis: 812}}); ok != want {
		t.Errorf("ping = %+v, want %+v", ok, want)
	}
	if want := (told{"usageco", "usageco", OwnCall{Kind: "cache_keepalive", Model: "sonnet", Error: "fetch failed"}}); failed != want {
		t.Errorf("failed ping = %+v, want %+v", failed, want)
	}
}
