package gui

import (
	"net/http"
	"net/http/httptest"
	"os"
	"testing"

	"github.com/yetone/magpie/internal/agent"
	"github.com/yetone/magpie/internal/provider"
)

// An agent on a routing group whose ref carries Claude Code's [1m] mark is
// on that group: the gateway asks GroupFor, which takes the mark off, and
// the Providers page asks GroupFinder, which did not — so the page listed
// the agent on none of the group's providers while every request it made
// went through them.
func TestGroupFinderTakesTheOneMMark(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	p := provider.Provider{ID: "relay", Name: "Relay", Chat: "http://127.0.0.1:1/v1", Key: "key",
		Models: []string{"m1"}}
	if err := provider.Save(p); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "m1", Name: "M1", Members: []string{"relay/m1"}}); err != nil {
		t.Fatal(err)
	}

	find := provider.GroupFinder()
	for _, ref := range []string{"group/m1", "group/m1[1m]"} {
		g, members, ok := find(ref)
		if !ok {
			t.Errorf("%s: the group is not found, though the gateway routes it", ref)
			continue
		}
		if g.ID != "m1" {
			t.Errorf("%s: found %q", ref, g.ID)
		}
		if len(members) != 1 || members[0].Provider.ID != "relay" || members[0].Model != "m1" {
			t.Errorf("%s: members %+v", ref, members)
		}
	}
	// the gateway's own way of asking, for the same ref: it takes the mark
	// off already, which is the difference this page was on the wrong side of
	if id, ok := provider.GroupFor("m1[1m]"); !ok || id != "group/m1" {
		t.Errorf("GroupFor: %q %v", id, ok)
	}
}

// The Providers page row: an agent on a group is shown on each of that
// group's providers, however its ref is marked.
func TestProviderInfoShowsAnAgentOnItsGroupsProviders(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Write([]byte(`{"data":[{"id":"m1"}]}`))
	}))
	defer up.Close()
	p := provider.Provider{ID: "relay", Name: "Relay", Chat: up.URL + "/v1", Key: "key"}
	if err := provider.Save(p); err != nil {
		t.Fatal(err)
	}
	if _, err := p.Fetch(t.Context()); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "g", Name: "G", Members: []string{"relay/m1"}}); err != nil {
		t.Fatal(err)
	}

	a := &agent.Agent{ID: "claude", Name: "Claude Code", Icon: "claude-color", Bin: "claude"}
	a.Fields = []agent.Field{{Key: "model", Label: "model", Get: func() string { return "magpie/group/g[1m]" }}}
	uses := agentUses([]*agent.Agent{a}, provider.GroupFinder())
	if !uses[0].inGroup {
		t.Fatalf("the agent is on a group: %+v", uses[0])
	}
	if len(uses[0].members) != 1 {
		t.Fatalf("members %+v", uses[0].members)
	}
	j := providerInfo(p, uses)
	if len(j.Agents) != 1 || !j.Agents[0].Current || j.Agents[0].Group != "G" {
		t.Fatalf("the row tells of no agent on it: %+v", j.Agents)
	}
	if j.Agents[0].Model != "m1" {
		t.Errorf("model %q", j.Agents[0].Model)
	}
}
