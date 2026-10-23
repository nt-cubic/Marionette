package gateway

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// Lovest on Discord: a provider plugin says the service tiers its vendor
// serves a model at (serviceTiers, #2514), but only Cursor's plugin's
// models could be switched fast. A plugin's model it says is served at
// priority has fast mode, whichever plugin: it is saved fast in a group
// and a picker, and every agent's fast ask reaches the plugin's fetch as
// service_tier "priority", Claude's speed "fast" too. One served at
// ultrafast as well can be picked Ultrafast. A model served at neither, or
// on Claude's API, has none.
func TestPluginTiersGiveFastMode(t *testing.T) {
	t.Setenv("FAKE_TIERS", "1")
	up := &tierUp{}
	pid := besideFake(t, "tiered", up)
	p, err := provider.Find(pid)
	if err != nil {
		t.Fatal(err)
	}
	for model, want := range map[string][2]bool{
		"fake-1":      {true, true},   // priority and ultrafast
		"fake-resp":   {false, false}, // ultrafast alone: no Fast to send Ultrafast as
		"fake-claude": {false, false}, // says none
	} {
		if got := provider.CanFast(*p, model); got != want[0] {
			t.Errorf("CanFast(%s) = %v, want %v", model, got, want[0])
		}
		if got := provider.CanUltra(*p, model); got != want[1] {
			t.Errorf("CanUltra(%s) = %v, want %v", model, got, want[1])
		}
	}
	if err := provider.SaveGroup(provider.Group{ID: "g", Members: []string{pid + "/fake-claude:fast"}}); err == nil || !strings.Contains(err.Error(), "no fast mode") {
		t.Fatalf("fake-claude saved fast: %v", err)
	}
	if err := provider.SetFastPick("claude", pid+"/fake-claude", true); err == nil {
		t.Fatal("fake-claude picked fast")
	}

	// a group's member sent fast, from each API an agent speaks
	saveFastGroup(t, []string{pid + "/fake-1:fast"}, nil)
	s := New()
	for _, a := range []struct{ name, path, body string }{
		{"chat", "/v1/chat/completions", `{"model":"group/f","messages":[{"role":"user","content":"hi"}]}`},
		{"responses", "/v1/responses", `{"model":"group/f","input":"hi"}`},
		{"anthropic", "/v1/messages", `{"model":"group/f","max_tokens":64,"messages":[{"role":"user","content":"hi"}]}`},
	} {
		r := postProto(t, s, a.path, a.body)
		if len(r.Tries) != 1 || !r.Tries[0].Fast {
			t.Fatalf("%s: tries %+v", a.name, r.Tries)
		}
		if got := up.last(t); got != "priority" {
			t.Errorf("%s: the plugin was sent service_tier %v, want priority", a.name, got)
		}
	}

	// a fast pick in Claude Code's picker, and the same pick Ultrafast
	if err := provider.SetFastPick("claude", pid+"/fake-1", true); err != nil {
		t.Fatal(err)
	}
	ask := `{"model":"` + pid + `/fake-1","max_tokens":64,"messages":[{"role":"user","content":"hi"}]}`
	if r := postFrom(t, s, "claude", "/v1/messages", ask); len(r.Tries) != 1 || !r.Tries[0].Fast {
		t.Fatalf("pick: tries %+v", r.Tries)
	}
	if got := up.last(t); got != "priority" {
		t.Errorf("fast pick: the plugin was sent service_tier %v, want priority", got)
	}
	if err := provider.SetFastTier("claude", pid+"/fake-1", "ultrafast"); err != nil {
		t.Fatal(err)
	}
	postFrom(t, s, "claude", "/v1/messages", ask)
	if got := up.last(t); got != "ultrafast" {
		t.Errorf("Ultrafast pick: the plugin was sent service_tier %v, want ultrafast", got)
	}
	// another agent's request for it goes as it asked
	postFrom(t, s, "opencode", "/v1/messages", ask)
	if got := up.last(t); got != nil {
		t.Errorf("another agent's request was sent service_tier %v", got)
	}
}
