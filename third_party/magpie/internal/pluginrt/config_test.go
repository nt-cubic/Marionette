package pluginrt

import (
	"context"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func home(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("MAGPIE_PORTABLE", "")
}

var three = []Manifest{
	{ID: "core", Essential: true, Defaults: map[string]any{"n": 1, "deep": map[string]any{"a": 1, "b": 2}}},
	{ID: "extra"},
	{ID: "used", InUse: func() (int, error) { return 2, nil }},
}

func byID(cs []Composed) map[string]Composed {
	m := map[string]Composed{}
	for _, c := range cs {
		m[c.ID] = c
	}
	return m
}

// The layers, field by field: the default, the preset, the user's rows,
// and in use over them all; each value says where it came from.
func TestCompose(t *testing.T) {
	no := false
	cs := byID(Compose(three, File{Preset: PresetEssentials, Plugins: []Row{
		{ID: "core", Config: map[string]any{"deep": map[string]any{"b": 3}}},
		{ID: "used", Enabled: &no},
	}}))
	if c := cs["core"]; !c.Enabled || c.Source != FromDefault {
		t.Errorf("core: %+v", c)
	}
	if c := cs["core"]; !reflect.DeepEqual(c.Config, map[string]any{"n": 1, "deep": map[string]any{"a": 1, "b": 3}}) || c.Sources["deep"] != FromUser || c.Sources["n"] != FromDefault {
		t.Errorf("core's config: %+v %+v", c.Config, c.Sources)
	}
	if c := cs["extra"]; c.Enabled || c.Source != FromPreset {
		t.Errorf("extra under Essentials: %+v", c)
	}
	if c := cs["used"]; !c.Enabled || c.Chosen || c.Source != FromInUse || c.InUse != 2 {
		t.Errorf("used, turned off: %+v", c)
	}
	// All, or no file at all: every one on
	for _, f := range []File{{Preset: PresetAll}, {}} {
		for _, c := range Compose(three, f) {
			if !c.Enabled {
				t.Errorf("%q: %s off", f.Preset, c.ID)
			}
		}
	}
}

// A new install is written Essentials, one that ran before All; the file
// there is never written over by Settle; Set keeps the other rows and the
// preset, and the file it replaces is kept beside it.
func TestSettleAndSet(t *testing.T) {
	home(t)
	if wrote, err := Settle(true); err != nil || !wrote {
		t.Fatalf("settle: %v %v", wrote, err)
	}
	if f, there, err := Load(); err != nil || !there || f.Preset != PresetEssentials {
		t.Fatalf("after settle: %+v %v %v", f, there, err)
	}
	if wrote, _ := Settle(false); wrote {
		t.Fatal("settled over a plugins.yml there")
	}
	if err := Set("extra", true); err != nil {
		t.Fatal(err)
	}
	if err := Set("used", false); err != nil {
		t.Fatal(err)
	}
	if err := Set("extra", false); err != nil {
		t.Fatal(err)
	}
	f, _, err := Load()
	if err != nil || f.Preset != PresetEssentials || len(f.Plugins) != 2 || *f.Plugins[0].Enabled || f.Plugins[0].ID != "extra" {
		t.Fatalf("after sets: %+v %v", f, err)
	}
	if _, err := os.Stat(Path() + ".bak"); err != nil {
		t.Fatalf("no .bak: %v", err)
	}
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	if wrote, _ := Settle(false); !wrote {
		t.Fatal("not settled")
	}
	if f, _, _ := Load(); f.Preset != PresetAll {
		t.Fatalf("an existing user: %+v", f)
	}
}

// A plugins.yml that doesn't parse is an error, never an empty file, and
// is never written over.
func TestBrokenFileIsKept(t *testing.T) {
	home(t)
	if err := os.MkdirAll(filepath.Dir(Path()), 0o700); err != nil {
		t.Fatal(err)
	}
	broken := []byte("plugins: [ {id: x\n")
	if err := os.WriteFile(Path(), broken, 0o600); err != nil {
		t.Fatal(err)
	}
	if _, _, err := Load(); err == nil {
		t.Fatal("a broken file read")
	}
	if err := Set("x", true); err == nil {
		t.Fatal("a broken file written")
	}
	if wrote, _ := Settle(true); wrote {
		t.Fatal("a broken file settled over")
	}
	if b, _ := os.ReadFile(Path()); string(b) != string(broken) {
		t.Fatalf("the broken file changed: %q", b)
	}
	if err := os.WriteFile(Path(), []byte("preset: slim\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, _, err := Load(); err == nil || !strings.Contains(err.Error(), "slim") {
		t.Fatalf("an unknown preset: %v", err)
	}
}

// A plugin turned off or on in plugins.yml by another process is disposed
// or applied in the one keeping fresh, without a restart.
func TestKeepFreshFollowsPluginsYml(t *testing.T) {
	home(t)
	var applied atomic.Int32
	Register(Manifest{ID: "fresh-test", Apply: func(c *Ctx) error {
		applied.Add(1)
		c.OnDispose(func() { applied.Add(-1) })
		return nil
	}})
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() { KeepFresh(ctx, 10*time.Millisecond); close(done) }()
	defer func() { cancel(); <-done; Set("fresh-test", false); Refresh() }()
	wait := func(want int32) {
		t.Helper()
		deadline := time.Now().Add(5 * time.Second)
		for applied.Load() != want && time.Now().Before(deadline) {
			time.Sleep(5 * time.Millisecond)
		}
		if got := applied.Load(); got != want {
			t.Fatalf("applied = %d, want %d", got, want)
		}
	}
	wait(1) // no plugins.yml: every plugin on
	if err := Set("fresh-test", false); err != nil {
		t.Fatal(err)
	}
	wait(0)
	if err := Set("fresh-test", true); err != nil {
		t.Fatal(err)
	}
	wait(1)
}
