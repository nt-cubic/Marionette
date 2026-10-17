package agent

import (
	"strings"
	"testing"

	"github.com/pelletier/go-toml/v2"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// setAsyncCompact sets Codex's background compaction as the Settings row
// does, put back when the test ends.
func setAsyncCompact(t *testing.T, on bool, percent int) {
	t.Helper()
	was := settings.Load()
	t.Cleanup(func() {
		s := settings.Load()
		s.CodexAsyncCompact, s.CodexAsyncCompactPercent = was.CodexAsyncCompact, was.CodexAsyncCompactPercent
		if err := settings.Save(s); err != nil {
			t.Errorf("restoring settings: %v", err)
		}
	})
	s := settings.Load()
	s.CodexAsyncCompact, s.CodexAsyncCompactPercent = on, percent
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
}

// asyncOf is the config's async_compaction value and the capability on
// magpie's provider table.
func asyncOf(t *testing.T, config string) (feature any, capability any) {
	t.Helper()
	var doc map[string]any
	if err := toml.Unmarshal([]byte(config), &doc); err != nil {
		t.Fatalf("%v:\n%s", err, config)
	}
	features, _ := doc["features"].(map[string]any)
	providers, _ := doc["model_providers"].(map[string]any)
	p, _ := providers[magpieID].(map[string]any)
	caps, _ := p["capabilities"].(map[string]any)
	return features["async_compaction"], caps["remote_compaction"]
}

// The switch on, Codex routed through magpie as its provider (no ChatGPT
// sign-in) gets [features.async_compaction] with the threshold in that
// table, where Codex 0.163 reads it, and remote_compaction = "v2" on
// magpie's provider table: without it Codex never starts a background
// compaction on a provider other than OpenAI's (tried with Codex 0.163:
// none in a whole turn past the threshold; with it, one per step, its
// summary taken in). A model switch writing the table again keeps it, and
// the switch off takes all of it out again.
func TestCodexAsyncCompactionWritten(t *testing.T) {
	quietCodex(t, false, provider.CodexDaemonCheck{})
	home, read := codexHome(t, "", "model = \"gpt-5.5\"\n")
	setAsyncCompact(t, true, 70)
	cx := codex(home)
	route(t, cx, read)
	feature, capability := asyncOf(t, read())
	table, _ := feature.(map[string]any)
	if table["enabled"] != true || table["model_async_compact_threshold_percent"] != int64(70) {
		t.Fatalf("async_compaction = %v:\n%s", feature, read())
	}
	if capability != "v2" {
		t.Fatalf("no remote_compaction = \"v2\" on magpie's provider:\n%s", read())
	}
	// a model switch writes magpie's provider table again
	if err := cx.Fields[0].Set("fake/m1"); err != nil {
		t.Fatal(err)
	}
	if err := cx.Sync(); err != nil {
		t.Fatal(err)
	}
	if _, capability := asyncOf(t, read()); capability != "v2" {
		t.Fatalf("the capability went as the table was written again:\n%s", read())
	}
	if strings.Count(read(), "capabilities") != 1 || strings.Count(read(), "[features.async_compaction]") != 1 {
		t.Fatalf("written twice:\n%s", read())
	}
	// another threshold
	setAsyncCompact(t, true, 80)
	if err := cx.Sync(); err != nil {
		t.Fatal(err)
	}
	if feature, _ := asyncOf(t, read()); feature.(map[string]any)["model_async_compact_threshold_percent"] != int64(80) {
		t.Fatalf("threshold not moved to 80:\n%s", read())
	}
	setAsyncCompact(t, false, 80)
	if err := cx.Sync(); err != nil {
		t.Fatal(err)
	}
	if feature, capability := asyncOf(t, read()); feature != nil || capability != nil {
		t.Fatalf("left behind with the switch off: %v %v\n%s", feature, capability, read())
	}
	if _, ok := stashLoad()["codex.async"]; ok {
		t.Fatal("the stash still claims what magpie wrote")
	}
}

// A value the user set is theirs: on, it is never written over; off, it
// stays. The feature the user turned on by hand gets the capability it
// needs on magpie's provider, the switch on or not (#2493).
func TestCodexAsyncCompactionKeepsTheUsers(t *testing.T) {
	for _, tc := range []struct {
		name, config string
		on           bool
		want         string // the config holds this after
		capability   any    // on magpie's provider once routed
	}{
		{"enabled false, threshold", "[features.async_compaction]\nenabled = false\nmodel_async_compact_threshold_percent = 50\n", true,
			"[features.async_compaction]\nenabled = false\nmodel_async_compact_threshold_percent = 50\n", "v2"},
		{"bare true", "[features]\nasync_compaction = true\n", false, "[features]\nasync_compaction = true\n", "v2"},
		{"bare false", "[features]\nasync_compaction = false\n", true, "[features]\nasync_compaction = false\n", "v2"},
		{"inline", "[features]\nasync_compaction = { enabled = true }\n", false, "async_compaction = { enabled = true }\n", "v2"},
		{"bare true, switch on", "[features]\nasync_compaction = true\n", true, "[features]\nasync_compaction = true\n", "v2"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			quietCodex(t, false, provider.CodexDaemonCheck{})
			home, read := codexHome(t, "", "model = \"gpt-5.5\"\n\n"+tc.config)
			setAsyncCompact(t, tc.on, 70)
			cx := codex(home)
			route(t, cx, read)
			if !strings.Contains(read(), tc.want) || strings.Contains(read(), "= 70") {
				t.Fatalf("the user's value was written over:\n%s", read())
			}
			if _, capability := asyncOf(t, read()); capability != tc.capability {
				t.Fatalf("capability %v, want %v:\n%s", capability, tc.capability, read())
			}
			setAsyncCompact(t, false, 0)
			if err := cx.Sync(); err != nil {
				t.Fatal(err)
			}
			if !strings.Contains(read(), tc.want) {
				t.Fatalf("the user's value went with the switch off:\n%s", read())
			}
		})
	}
}

// A capability the user gave magpie's provider stays as they wrote it.
func TestCodexAsyncCompactionKeepsUsersCapability(t *testing.T) {
	quietCodex(t, false, provider.CodexDaemonCheck{})
	home, read := codexHome(t, "", "model = \"gpt-5.5\"\n")
	cx := codex(home)
	route(t, cx, read)
	b := strings.Replace(read(), "wire_api = \"responses\"\n", "wire_api = \"responses\"\ncapabilities = { remote_compaction = \"unsupported\" }\n", 1)
	writeFile(t, cx.Path, b)
	setAsyncCompact(t, true, 0)
	if err := cx.Fields[0].Set("fake/m1"); err != nil {
		t.Fatal(err)
	}
	if err := cx.Sync(); err != nil {
		t.Fatal(err)
	}
	if _, capability := asyncOf(t, read()); capability != "unsupported" {
		t.Fatalf("the user's capability was written over:\n%s", read())
	}
	setAsyncCompact(t, false, 0)
	if err := cx.Sync(); err != nil {
		t.Fatal(err)
	}
	if _, capability := asyncOf(t, read()); capability != "unsupported" {
		t.Fatalf("the user's capability went:\n%s", read())
	}
}
