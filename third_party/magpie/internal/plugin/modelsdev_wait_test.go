package plugin

import (
	"context"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/testenv"
)

var (
	hangInitOnce sync.Once
	hangEnabled  atomic.Bool
	hangStopMu   sync.Mutex
	hangStopCh   chan struct{}
)

func installHangTransport() {
	hangInitOnce.Do(func() {
		tr, ok := http.DefaultTransport.(*http.Transport)
		if !ok {
			return
		}
		orig := tr.DialContext
		tr.DialContext = func(ctx context.Context, network, addr string) (net.Conn, error) {
			if hangEnabled.Load() && (addr == "models.dev:443" || strings.HasPrefix(addr, "models.dev:")) {
				hangStopMu.Lock()
				stop := hangStopCh
				hangStopMu.Unlock()
				if stop != nil {
					select {
					case <-ctx.Done():
						return nil, ctx.Err()
					case <-stop:
						return nil, context.Canceled
					}
				}
			}
			if orig != nil {
				return orig(ctx, network, addr)
			}
			return (&net.Dialer{}).DialContext(ctx, network, addr)
		}
	})
}

// hangModelsDev wraps http.DefaultTransport's DialContext so dials to
// models.dev:443 block until the request's context is canceled or the test ends.
func hangModelsDev(t *testing.T) {
	t.Helper()
	installHangTransport()
	stop := make(chan struct{})
	hangStopMu.Lock()
	hangStopCh = stop
	hangStopMu.Unlock()
	hangEnabled.Store(true)

	t.Cleanup(func() {
		hangEnabled.Store(false)
		close(stop)
	})
}

// startBound is how long a host start may take here: far under the 25s
// the hanging models.dev held it for before, and far over a bun start on a
// loaded runner, so the test is red only for the wait it is about.
const startBound = 15 * time.Second

// TestHostStartDoesNotWaitOnHangingModelsDev (#1928): with no cached catalog,
// a hanging models.dev server must not stall startOn, both when alone and when
// another catalog.Sync is already holding the lock in flight.
func TestHostStartDoesNotWaitOnHangingModelsDev(t *testing.T) {
	bun := testenv.Bun(t)
	hangModelsDev(t)
	sandbox(t)
	catalog.Reset()

	t.Run("alone", func(t *testing.T) {
		ctx, cancel := context.WithTimeout(context.Background(), 25*time.Second)
		defer cancel()

		start := time.Now()
		h, _, err := startOn(ctx, bun)
		if err != nil {
			t.Fatalf("startOn failed: %v", err)
		}
		defer h.stop()
		elapsed := time.Since(start)

		if elapsed >= startBound {
			t.Fatalf("startOn took %v; want fast start not waiting on models.dev", elapsed)
		}
	})

	t.Run("with_sync_in_flight", func(t *testing.T) {
		syncCtx, syncCancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer syncCancel()

		// Start a Sync that holds the lock while hanging on models.dev
		syncStarted := make(chan struct{})
		syncDone := make(chan struct{})
		go func() {
			close(syncStarted)
			defer close(syncDone)
			_ = catalog.Sync(syncCtx)
		}()
		<-syncStarted
		time.Sleep(50 * time.Millisecond) // ensure Sync acquired lock

		ctx, cancel := context.WithTimeout(context.Background(), 25*time.Second)
		defer cancel()

		start := time.Now()
		h, _, err := startOn(ctx, bun)
		if err != nil {
			t.Fatalf("startOn failed with sync in flight: %v", err)
		}
		defer h.stop()
		elapsed := time.Since(start)

		if elapsed >= startBound {
			t.Fatalf("startOn took %v with sync in flight; want fast start without blocking", elapsed)
		}

		syncCancel()
		<-syncDone
	})
}

// TestHostReloadsCatalogWhenFileAppears verifies that if the host starts
// before models.json exists on disk, it dynamically reloads models.dev when the
// catalog file appears or updates.
func TestHostReloadsCatalogWhenFileAppears(t *testing.T) {
	sandbox(t)
	catalog.Reset()

	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()

	abs, err := filepath.Abs("testdata/fake/index.js")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := Add(ctx, abs); err != nil {
		t.Fatal(err)
	}

	// Before catalog exists, Providers returns only the plugin's own models
	psBefore, err := Providers(ctx)
	if err != nil {
		t.Fatalf("Providers before cache: %v", err)
	}
	if len(psBefore) != 1 || psBefore[0].ID != "fakeco" {
		t.Fatalf("Providers before cache = %+v", psBefore)
	}
	hasModel := func(ps []Provider, id string) bool {
		for _, m := range ps[0].Models {
			if m.ID == id {
				return true
			}
		}
		return false
	}
	if hasModel(psBefore, "mdev-fake-model") {
		t.Fatal("mdev-fake-model present before cache was written")
	}

	// Write models.json to CachePath
	p := catalog.CachePath()
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		t.Fatal(err)
	}
	catalogJSON := `{"fakeco":{"id":"fakeco","name":"FakeCo ModelsDev","models":{"mdev-fake-model":{"id":"mdev-fake-model"}}}}`
	if err := os.WriteFile(p, []byte(catalogJSON), 0o644); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.Remove(p) })

	// Invalidate Go-side cache so it asks the host again
	forgetProviders()

	psAfter, err := Providers(ctx)
	if err != nil {
		t.Fatalf("Providers after cache: %v", err)
	}
	if len(psAfter) != 1 || psAfter[0].ID != "fakeco" {
		t.Fatalf("Providers after cache = %+v", psAfter)
	}
	if !hasModel(psAfter, "mdev-fake-model") {
		t.Fatalf("mdev-fake-model missing in Models after models.json appeared; host.js did not reload catalog: %+v", psAfter[0].Models)
	}
}

// TestHostBackgroundSyncForgetsProviders verifies that when a background
// catalog sync succeeds, it calls forgetProviders() so Cached() serves the
// newly available models.dev models without requiring manual cache invalidation.
func TestHostBackgroundSyncForgetsProviders(t *testing.T) {
	sandbox(t)
	catalog.Reset()

	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()

	abs, err := filepath.Abs("testdata/fake/index.js")
	if err != nil {
		t.Fatal(err)
	}

	syncRelease := make(chan struct{})
	wasSync := syncCatalog
	syncCatalog = func(ctx context.Context) error {
		select {
		case <-syncRelease:
		case <-ctx.Done():
			return ctx.Err()
		}
		p := catalog.CachePath()
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			return err
		}
		catalogJSON := `{"fakeco":{"id":"fakeco","name":"FakeCo ModelsDev","models":{"mdev-fake-model":{"id":"mdev-fake-model"}}}}`
		return os.WriteFile(p, []byte(catalogJSON), 0o644)
	}
	t.Cleanup(func() { syncCatalog = wasSync })

	if _, err := Add(ctx, abs); err != nil {
		t.Fatal(err)
	}

	// First call to Providers starts the host (triggering background syncCatalog).
	// Because syncCatalog is waiting on syncRelease, models.json is not on disk yet.
	psBefore, err := Providers(ctx)
	if err != nil {
		t.Fatalf("Providers before sync: %v", err)
	}
	hasModel := func(ps []Provider, id string) bool {
		if len(ps) == 0 {
			return false
		}
		for _, m := range ps[0].Models {
			if m.ID == id {
				return true
			}
		}
		return false
	}
	if hasModel(psBefore, "mdev-fake-model") {
		t.Fatal("mdev-fake-model present before catalog sync completed")
	}

	// Release syncCatalog to write models.json and return nil (which calls forgetProviders()).
	close(syncRelease)
	catalogSyncing.Wait()

	// Cached() triggers background refresh when !provGood.
	Cached()
	Refreshed()

	psAfter := Cached()
	if !hasModel(psAfter, "mdev-fake-model") {
		t.Fatalf("mdev-fake-model missing in Cached() after background sync; forgetProviders() was not called: %+v", psAfter)
	}
}

// TestHostKeepsTheCatalogThroughAHalfWrite: catalog.Sync writes
// models.json in place, so a refresh can be read half written. That reading
// is unknown, not an empty catalog: the host keeps the catalog it read
// before, rather than listing the plugins' providers without it (which
// the Go side would keep until its next refresh), and reads the file again
// once it is whole, even within the same mtime.
func TestHostKeepsTheCatalogThroughAHalfWrite(t *testing.T) {
	sandbox(t)
	catalog.Reset()
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, err := filepath.Abs("testdata/fake/index.js")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	p := catalog.CachePath()
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.Remove(p) })
	whole := `{"fakeco":{"id":"fakeco","name":"FakeCo ModelsDev","models":{"mdev-fake-model":{"id":"mdev-fake-model"}}}}`
	write := func(s string, at time.Time) {
		if err := os.WriteFile(p, []byte(s), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.Chtimes(p, at, at); err != nil {
			t.Fatal(err)
		}
	}
	has := func() bool {
		forgetProviders()
		ps, err := Providers(ctx)
		if err != nil {
			t.Fatal(err)
		}
		for _, m := range ps[0].Models {
			if m.ID == "mdev-fake-model" {
				return true
			}
		}
		return false
	}
	before := time.Now().Add(-2 * time.Minute).Truncate(time.Second)
	refresh := before.Add(time.Minute)
	write(whole, before)
	if !has() {
		t.Fatal("the catalog's model isn't listed")
	}
	write(whole[:40], refresh)
	if !has() {
		t.Fatal("a catalog read half written dropped the catalog read before")
	}
	write(whole, refresh)
	if !has() {
		t.Fatal("the whole catalog isn't read once the half write is done")
	}
}
