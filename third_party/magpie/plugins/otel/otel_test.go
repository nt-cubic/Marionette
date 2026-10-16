package otel

import (
	"io"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/otlp"
	"github.com/yetone/magpie/internal/pluginrt"
	"github.com/yetone/magpie/internal/testenv"
	"github.com/yetone/magpie/internal/usage"
)

func TestMain(m *testing.M) { testenv.Main(m) }

func manifest(t *testing.T) pluginrt.Manifest {
	t.Helper()
	for _, m := range pluginrt.Registered() {
		if m.ID == "otel" {
			return m
		}
	}
	t.Fatal("otel not registered")
	return pluginrt.Manifest{}
}

// On, the plugin's exporter observes the gateway and a ledger row reaches
// the collector; off, there is no observer and nothing is sent, though
// settings still say to send.
func TestOnExportsOffSendsNothing(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(t.TempDir(), "config"))
	received := make(chan string, 8)
	collector := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		received <- string(b)
		io.WriteString(w, `{}`)
	}))
	defer collector.Close()
	t.Setenv("MAGPIE_OTEL_ENABLED", "true")
	t.Setenv("MAGPIE_OTEL_ENDPOINT", collector.URL)
	t.Setenv("MAGPIE_OTEL_HEADERS", "")
	t.Setenv("MAGPIE_OTEL_METRICS", "false")
	every := otlp.OTelEvery
	otlp.OTelEvery = 50 * time.Millisecond
	t.Cleanup(func() { otlp.OTelEvery = every })
	stop := usage.ServeObservers()
	defer stop()

	rt := pluginrt.New([]pluginrt.Manifest{manifest(t)})
	defer rt.Stop()
	// seen is every model the collector got until want came, or "" when it
	// didn't within a minute. The first export prices its rows, which loads
	// the catalog: 10s wasn't enough on a machine at load 100 (the full
	// suite). Waiting longer costs a passing run nothing, and "off sends
	// nothing" doesn't hang on it: what came before model-again is checked.
	seen := func(want string) string {
		var got strings.Builder
		deadline := time.After(time.Minute)
		for {
			select {
			case b := <-received:
				got.WriteString(b)
				if strings.Contains(b, want) {
					return got.String()
				}
			case <-deadline:
				return ""
			}
		}
	}
	row := func(model string) {
		usage.Append(usage.Record{Time: time.Now(), Agent: "pi", Via: "office", Model: model, Input: 1, Status: 200})
	}
	on := func() {
		t.Helper()
		rt.Reconcile(map[string]bool{"otel": true}, nil)
		if !rt.Active("otel") {
			t.Fatalf("otel not active: %+v", rt.Statuses())
		}
		if obs := usage.Observers(); len(obs) != 1 {
			t.Fatalf("observers on: %d, want 1", len(obs))
		} else if _, ok := obs[0].(*otlp.Exporter); !ok {
			t.Fatalf("observer is %T", obs[0])
		}
	}

	on()
	row("model-on")
	if seen("model-on") == "" {
		t.Fatal("on: the row was not exported")
	}
	rt.Reconcile(map[string]bool{"otel": false}, nil)
	if obs := usage.Observers(); len(obs) != 0 {
		t.Fatalf("observers off: %d, want 0", len(obs))
	}
	if usage.ObservedBodies() != usage.NoBodies {
		t.Fatal("off: the gateway still keeps bodies")
	}
	row("model-off")
	// on again: what comes is what came after, never the row sent while off
	on()
	row("model-again")
	got := seen("model-again")
	if got == "" {
		t.Fatal("on again: the row was not exported")
	}
	if strings.Contains(got, "model-off") {
		t.Fatal("off: the row was exported")
	}
}

// MAGPIE_OTEL_ENABLED alone keeps it on, as settings.json's switch does:
// off, an export set up only in the environment would send nothing.
func TestInUseByEnvironment(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(t.TempDir(), "config"))
	m := manifest(t)
	t.Setenv("MAGPIE_OTEL_ENABLED", "false")
	if n, err := m.InUse(); err != nil || n != 0 {
		t.Fatalf("off: %d %v", n, err)
	}
	t.Setenv("MAGPIE_OTEL_ENDPOINT", "http://127.0.0.1:1")
	t.Setenv("MAGPIE_OTEL_ENABLED", "true")
	if n, err := m.InUse(); err != nil || n != 1 {
		t.Fatalf("on by the environment: %d %v", n, err)
	}
}
