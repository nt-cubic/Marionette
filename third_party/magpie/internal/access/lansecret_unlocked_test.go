package access

import (
	"testing"
	"time"
)

// #1665: with the gateway not shared, LANSecret answers from the settings
// alone, without taking caller-keys.json's lock (every agent's sync asks
// it, ~100 times in a start's first 20s, each a lock file opened and
// locked); shared, it reads the key under the lock, as before.
func TestLANSecretNotSharedTakesNoLock(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	answer := func() (string, bool) {
		got := make(chan string, 1)
		go func() { got <- LANSecret() }()
		select {
		case s := <-got:
			return s, true
		case <-time.After(5 * time.Second):
			return "", false
		}
	}

	mu.Lock()
	s, ok := answer()
	mu.Unlock()
	if !ok || s != "" {
		t.Fatalf("not shared, under another's lock: answered=%v %q; want \"\" at once", ok, s)
	}

	if err := ConfigureLAN(true, false); err != nil {
		t.Fatal(err)
	}
	if s, ok := answer(); !ok || s == "" {
		t.Fatalf("shared: answered=%v %q; want the key", ok, s)
	}
}
