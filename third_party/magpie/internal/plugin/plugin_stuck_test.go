package plugin

import (
	"context"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// stuckWaits shortens how long the host waits on one plugin's load and on
// one provider's model list, for the test.
func stuckWaits(t *testing.T) {
	t.Helper()
	load, list := pluginLoadWait, pluginListWait
	pluginLoadWait, pluginListWait = 2*time.Second, 2*time.Second
	t.Cleanup(func() { pluginLoadWait, pluginListWait = load, list })
}

// addStuck adds the stuck plugin ahead of the fake one, so the host meets
// it first, as a plugin earlier in the user's list.
func addStuck(t *testing.T, ctx context.Context) {
	t.Helper()
	for _, p := range []string{"testdata/stuck/index.js", "testdata/fake/index.js"} {
		abs, _ := filepath.Abs(p)
		if _, err := Add(ctx, abs); err != nil {
			t.Fatal(err)
		}
	}
}

// A plugin whose load never finishes, as one whose sign-in is refused and
// tried again, holds up none of the plugins after it: they load and are
// listed, and it is told as not loaded, saying why (shualaitianduai on
// Discord). Before, the host loaded the plugins one after another and
// answered init only when all were in, so every plugin waited until
// magpie's two-minute deadline failed the whole host.
func TestStuckPluginLoadHoldsNoOther(t *testing.T) {
	sandbox(t)
	stuckWaits(t)
	t.Setenv("STUCK_LOAD", "1")
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	addStuck(t, ctx)

	ls, err := Plugins(ctx)
	if err != nil || len(ls) != 2 {
		t.Fatalf("Plugins = %+v, %v", ls, err)
	}
	for _, l := range ls {
		stuck := strings.Contains(l.Spec, "stuck")
		if stuck && !strings.Contains(l.Error, "didn't finish") {
			t.Errorf("the stuck plugin says %q, not that it didn't finish loading", l.Error)
		}
		if !stuck && l.Error != "" {
			t.Errorf("the plugin after the stuck one didn't load: %s", l.Error)
		}
	}
	ps, err := Providers(ctx)
	if err != nil || len(ps) != 1 || ps[0].ID != "fakeco" || len(ps[0].Models) == 0 {
		t.Fatalf("Providers = %+v, %v; want fakeco with its models", ps, err)
	}
}

// A provider whose models hook never answers holds up no other provider's
// listing: the list comes back within the host's wait, the other provider
// with its models and the stuck one with a list error saying it gave up.
// Before, the host listed the providers one after another, and the whole
// list waited on the stuck one past refreshDeadline.
func TestStuckModelListHoldsNoOther(t *testing.T) {
	sandbox(t)
	stuckWaits(t)
	t.Setenv("STUCK_MODELS", "1")
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	addStuck(t, ctx)
	for _, id := range []string{"stuckco", "fakeco"} {
		if _, err := APIKey(ctx, id, 0, nil, "k-"+id, NewAccount); err != nil {
			t.Fatal(err)
		}
	}

	lctx, lcancel := context.WithTimeout(ctx, 15*time.Second)
	defer lcancel()
	ps, err := Providers(lctx)
	if err != nil || len(ps) != 2 {
		t.Fatalf("Providers = %+v, %v", ps, err)
	}
	for _, p := range ps {
		switch p.ID {
		case "fakeco":
			if p.FellBack || len(p.Models) == 0 {
				t.Errorf("fakeco listed fellBack=%v with %d models, %q", p.FellBack, len(p.Models), p.ListError)
			}
		case "stuckco":
			if !p.FellBack || !strings.Contains(p.ListError, "didn't finish") {
				t.Errorf("stuckco listed fellBack=%v saying %q", p.FellBack, p.ListError)
			}
		default:
			t.Errorf("listed %q", p.ID)
		}
	}
}
