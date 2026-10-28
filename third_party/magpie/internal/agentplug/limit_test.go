package agentplug

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
	"testing/synctest"
	"time"
)

// A call can end just as its limit runs out: the timer's func has begun
// and not yet given its interrupt. Stop doesn't wait for it, and moejs
// keeps an interrupt that lands while nothing runs for the next call,
// whatever it is. That call's limit never fired, so nothing cleared it,
// and the module's every connect after failed with "interrupted: took too
// long" until magpie loaded it again (#3727). Here the timer's func is
// held at that moment, and limit waits for it: it hasn't returned before
// the func is let go. A call that finished keeps what it returned, one that
// failed reads took too long, and the next connect answers in time.
func TestLimitRunsOutAsTheCallEnds(t *testing.T) {
	dir := t.TempDir()
	file := filepath.Join(dir, "agent.js")
	if err := os.WriteFile(file, []byte(`export const agent = { id: "late", config: "~/.late.json", model: "model" };
export function connect() { return { model: "m" }; }
`), 0o644); err != nil {
		t.Fatal(err)
	}
	for _, c := range []struct {
		name string
		ends error // what the call returns as its limit runs out
		want error
	}{
		{"finished", nil, nil},
		{"failed", errors.New("the module threw"), errTimeout},
	} {
		t.Run(c.name, func(t *testing.T) {
			synctest.Test(t, func(t *testing.T) {
				// a runtime of the case's own, loaded under the bubble's time
				a, err := Compile(dir, file)
				if err != nil {
					t.Fatal(err)
				}
				begun, release := make(chan struct{}), make(chan struct{})
				afterFunc = func(d time.Duration, f func()) *time.Timer {
					return time.AfterFunc(d, func() {
						close(begun)
						<-release
						f()
					})
				}
				t.Cleanup(func() { afterFunc = time.AfterFunc })
				got := make(chan error, 1)
				go func() {
					got <- a.limit(time.Second, func() error {
						<-begun // the second has passed, and the timer's func began
						return c.ends
					})
				}()
				<-begun
				synctest.Wait() // limit waits for the func
				if len(got) != 0 {
					t.Error("limit returned while its timer's func hadn't given its interrupt")
				}
				close(release)
				if err := <-got; !errors.Is(err, c.want) {
					t.Errorf("limit returned %v, want %v", err, c.want)
				}
				synctest.Wait() // the timer's func has given its interrupt
				afterFunc = time.AfterFunc
				start := time.Now()
				keys, err := a.Connect(map[string]any{})
				if err != nil || keys["model"] != "m" {
					t.Fatalf("the connect after: %v, %v; want the module's model", keys, err)
				}
				if d := time.Since(start); d != 0 {
					t.Errorf("the connect after, in time, took %v of its limit", d)
				}
			})
		})
	}
}

// A module still running when its time is up, limit's everyday case: a
// connect that never ends, at once or after an await, reads took too long,
// and the connects after it answer; a load that never ends reads took too
// long. Nothing here ends but by the interrupt, so the 50ms limits bound how
// long the test takes, not whether it passes.
func TestLimitStopsAModuleStillRunning(t *testing.T) {
	for _, c := range []struct{ name, src string }{
		{"loop", `export function connect() { if (++n === 1) for (;;) {} return { model: "m" }; }`},
		{"after an await", `export async function connect() { if (++n === 1) { await null; for (;;) {} } return { model: "m" }; }`},
	} {
		t.Run(c.name, func(t *testing.T) {
			dir := t.TempDir()
			file := filepath.Join(dir, "agent.js")
			if err := os.WriteFile(file, []byte("export const agent = { id: \"slow\", config: \"~/.slow.json\", model: \"model\" };\nlet n = 0;\n"+c.src+"\n"), 0o644); err != nil {
				t.Fatal(err)
			}
			a, err := Compile(dir, file)
			if err != nil {
				t.Fatal(err)
			}
			connectLimit = 50 * time.Millisecond
			t.Cleanup(func() { connectLimit = time.Second })
			if _, err := a.Connect(map[string]any{}); err == nil || err.Error() != "connect: took too long" {
				t.Errorf("the connect that runs out: %v, want connect: took too long", err)
			}
			connectLimit = time.Second
			for i := 1; i <= 3; i++ {
				if keys, err := a.Connect(map[string]any{}); err != nil || keys["model"] != "m" {
					t.Errorf("connect %d after it: %v, %v; want the module's model", i, keys, err)
				}
			}
		})
	}
	t.Run("load", func(t *testing.T) {
		dir := t.TempDir()
		file := filepath.Join(dir, "agent.js")
		if err := os.WriteFile(file, []byte("for (;;) {}\nexport const agent = { id: \"slow\", config: \"~/.slow.json\", model: \"model\" };\n"), 0o644); err != nil {
			t.Fatal(err)
		}
		loadLimit = 50 * time.Millisecond
		t.Cleanup(func() { loadLimit = time.Second })
		if _, err := Compile(dir, file); err == nil || err.Error() != "took too long" {
			t.Errorf("the load that runs out: %v, want took too long", err)
		}
	})
}
