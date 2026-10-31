package davsync

import (
	"context"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/backup"
	"github.com/yetone/magpie/internal/profile"
	"github.com/yetone/magpie/internal/provider"
)

// codexPick writes Codex's model and effort into this computer's
// config.toml, as a pick in Codex or in magpie does.
func codexPick(t *testing.T, model, effort string) {
	t.Helper()
	dir := os.Getenv("CODEX_HOME")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	body := "model = \"" + model + "\"\nmodel_reasoning_effort = \"" + effort + "\"\n"
	if err := os.WriteFile(filepath.Join(dir, "config.toml"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

func codexHere() (string, string) {
	f := profile.Fields()
	return f["codex.model"], f["codex.effort"]
}

// Two computers on one GitHub sync, each wanting its own Codex model
// (hayden4608 on Discord). A sets its model and syncs; B picks another and
// syncs, which brings B's over A's at A's next sync, as before. A puts its
// own back: from then on it stays A's, neither B's later picks come in nor
// A's own go up. The rest still syncs both ways: Codex's effort, Claude
// Code's model, a provider. Follow the others gives it back to sync.
func TestOwnModelStaysOnEachComputer(t *testing.T) {
	f := newGithubFixture(t)
	ctx := context.Background()
	c := githubConfig()
	now := func(t *testing.T) {
		t.Helper()
		if err := Now(ctx); err != nil {
			t.Fatal(err)
		}
		if st := Status(); st.Notice != nil || st.Error != "" {
			t.Fatalf("a sync that replaced nothing: %+v %q", st.Notice, st.Error)
		}
	}
	server := func(t *testing.T) map[string]string {
		t.Helper()
		b, err := backup.Open(f.objects[backupAt], c.Passphrase)
		if err != nil {
			t.Fatal(err)
		}
		return b.Agents
	}
	own := func(t *testing.T, want int) {
		t.Helper()
		v := Status()
		if len(v.Own) != want {
			t.Fatalf("own fields: %+v, want %d", v.Own, want)
		}
		if want == 1 && (!strings.Contains(v.Own[0].Agent, "Codex") || v.Own[0].Field != "model") {
			t.Fatalf("own fields: %+v, want Codex's model", v.Own)
		}
	}
	a, b := newComputer(t), newComputer(t)

	// A sets its model and syncs
	a.use(t)
	codexPick(t, "gpt-a1", "low")
	if m, _ := codexHere(); m != "gpt-a1" {
		t.Skipf("Codex isn't seen here: %v", profile.Fields())
	}
	if err := Configure(c); err != nil {
		t.Fatal(err)
	}
	now(t)

	// B joins: A's model comes in, as before (a joining computer's own
	// setup is saved aside, with a notice)
	b.use(t)
	codexPick(t, "gpt-b0", "low")
	if err := Configure(c); err != nil {
		t.Fatal(err)
	}
	if err := Now(ctx); err != nil {
		t.Fatal(err)
	}
	if err := Dismiss(); err != nil {
		t.Fatal(err)
	}
	if m, _ := codexHere(); m != "gpt-a1" {
		t.Fatalf("B joining: model %q, want A's", m)
	}

	// B picks a new model: it goes up, and comes in at A, as before
	codexPick(t, "gpt-b1", "low")
	now(t)
	if m := server(t)["codex.model"]; m != "gpt-b1" {
		t.Fatalf("B's pick didn't go up: %q", m)
	}
	own(t, 0)
	a.use(t)
	now(t)
	if m, _ := codexHere(); m != "gpt-b1" {
		t.Fatalf("A after B's pick: %q", m)
	}

	// A puts its own back: it stays on A, the server keeps B's
	codexPick(t, "gpt-a1", "low")
	now(t)
	if m, _ := codexHere(); m != "gpt-a1" {
		t.Fatalf("A's own: %q", m)
	}
	if m := server(t)["codex.model"]; m != "gpt-b1" {
		t.Fatalf("A's own went up: %q", m)
	}
	own(t, 1)

	// B picks again, with another effort, and adds a provider
	b.use(t)
	now(t)
	codexPick(t, "gpt-b2", "high")
	if err := provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Chat: "https://api.deepseek.com/v1", Key: "fixture-provider-key"}); err != nil {
		t.Fatal(err)
	}
	now(t)
	if m := server(t)["codex.model"]; m != "gpt-b2" {
		t.Fatalf("B's pick didn't go up: %q", m)
	}

	// A picks its own again before it syncs: a change of its own is no
	// change on both sides, so B's effort and provider come in with no
	// notice, and A's model stays
	a.use(t)
	codexPick(t, "gpt-a1b", "low")
	now(t)
	m, e := codexHere()
	if m != "gpt-a1b" {
		t.Fatalf("B's later pick came over A's own: %q", m)
	}
	if e != "high" {
		t.Fatalf("B's effort didn't come in: %q", e)
	}
	if !slices.Equal(ids(), []string{"deepseek=fixture-provider-key"}) {
		t.Fatalf("B's provider didn't come in: %v", ids())
	}

	// A picks again, a new effort too, and Claude Code's model: those go
	// up, Codex's model stays A's
	codexPick(t, "gpt-a2", "medium")
	if _, err := profile.ApplyFields(map[string]string{"claude.model": "claude-from-a"}); err != nil {
		t.Fatal(err)
	}
	if profile.Fields()["claude.model"] != "claude-from-a" {
		t.Skipf("Claude Code isn't seen here: %v", profile.Fields())
	}
	now(t)
	if got := server(t); got["codex.model"] != "gpt-b2" || got["codex.effort"] != "medium" || got["claude.model"] != "claude-from-a" {
		t.Fatalf("A's second pick: server has %q %q %q", got["codex.model"], got["codex.effort"], got["claude.model"])
	}
	own(t, 1)
	b.use(t)
	now(t)
	if m, e := codexHere(); m != "gpt-b2" || e != "medium" || profile.Fields()["claude.model"] != "claude-from-a" {
		t.Fatalf("B after A's second pick: %q %q %q", m, e, profile.Fields()["claude.model"])
	}
	own(t, 0)

	// A follows the others again: the server's model comes in
	a.use(t)
	if err := Follow(ctx); err != nil {
		t.Fatal(err)
	}
	if m, _ := codexHere(); m != "gpt-b2" {
		t.Fatalf("A following: %q", m)
	}
	if v := Status(); len(v.Own) != 0 || v.Notice != nil {
		t.Fatalf("A following: own %+v, notice %+v", v.Own, v.Notice)
	}
	// and B's next pick reaches A again
	b.use(t)
	codexPick(t, "gpt-b3", "medium")
	now(t)
	a.use(t)
	now(t)
	if m, _ := codexHere(); m != "gpt-b3" {
		t.Fatalf("A after following: %q", m)
	}
}

// New picks still go both ways: a computer that picks a model it never had
// changes it for the others, however often the two take turns. Only
// putting back the value sync wrote over keeps one.
func TestNewPicksStillGoBothWays(t *testing.T) {
	newGithubFixture(t)
	ctx := context.Background()
	c := githubConfig()
	a, b := newComputer(t), newComputer(t)
	a.use(t)
	codexPick(t, "gpt-1", "low")
	if m, _ := codexHere(); m != "gpt-1" {
		t.Skipf("Codex isn't seen here: %v", profile.Fields())
	}
	if err := Configure(c); err != nil {
		t.Fatal(err)
	}
	joined := map[computer]bool{a: true}
	for _, step := range []struct {
		c     computer
		pick  string
		model string
	}{
		{a, "", "gpt-1"},
		{b, "", "gpt-1"},
		{b, "gpt-2", "gpt-2"},
		{a, "", "gpt-2"},
		{a, "gpt-3", "gpt-3"},
		{b, "", "gpt-3"},
		{b, "gpt-4", "gpt-4"},
		{a, "", "gpt-4"},
		{a, "gpt-5", "gpt-5"},
		{b, "", "gpt-5"},
	} {
		step.c.use(t)
		if !joined[step.c] {
			joined[step.c] = true
			if err := Configure(c); err != nil {
				t.Fatal(err)
			}
		}
		if step.pick != "" {
			codexPick(t, step.pick, "low")
		}
		if err := Now(ctx); err != nil {
			t.Fatal(err)
		}
		if m, _ := codexHere(); m != step.model {
			t.Fatalf("model %q, want %q", m, step.model)
		}
		if v := Status(); len(v.Own) != 0 {
			t.Fatalf("own: %+v", v.Own)
		}
	}
}
