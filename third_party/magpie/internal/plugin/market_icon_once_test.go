package plugin

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

// A plugin's icon is looked up for every plugin's row each poll of the
// Providers page, before the market is fetched (or with no network): the
// cached copy was read and parsed whole each time, 13 MB of what twenty
// polls allocated with real accounts. It is parsed once for each copy now,
// and a copy written again is read again.
func TestMarketIconParsesTheCopyOnce(t *testing.T) {
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	marketMu.Lock()
	marketList = nil
	marketMu.Unlock()
	if err := os.MkdirAll(filepath.Dir(marketCache()), 0o755); err != nil {
		t.Fatal(err)
	}
	write := func(b []byte, at time.Time) {
		t.Helper()
		if err := os.WriteFile(marketCache(), b, 0o644); err != nil {
			t.Fatal(err)
		}
		os.Chtimes(marketCache(), at, at)
	}
	old := time.Now().Add(-time.Hour)
	// the copy as GitHub gives it: the list built in is one
	write(builtinMarket, old)
	const grok = "@magpie-community/opencode-grok-auth@0.1.0"
	if got := Icon(grok, "grok"); got != "xai" {
		t.Fatalf("Icon = %q, want the copy's xai", got)
	}
	if n := testing.AllocsPerRun(50, func() { Icon(grok, "grok") }); n > 20 {
		t.Fatalf("an unchanged copy: %.0f allocations a look, parsed again", n)
	}
	write([]byte(`{"version":1,"plugins":[{"package":"opencode-x-auth","name":"X","icon":"x-two","community":true}]}`), old.Add(time.Minute))
	if got := Icon("opencode-x-auth@1.0.0", "x"); got != "x-two" {
		t.Fatalf("after the copy was written again, Icon = %q", got)
	}
	os.Remove(marketCache())
	if got := Icon(grok, "grok"); got != "xai" {
		t.Fatalf("with no copy, the built-in list's: %q", got)
	}
}
