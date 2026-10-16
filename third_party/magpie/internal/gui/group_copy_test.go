package gui

import (
	"net/http"
	"net/http/httptest"
	"os"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// The Routing view's Duplicate (lc on Discord) makes a group of the
// user's that is the group as it is — its models, patterns, routing,
// rules, off members — under the name asked, numbered past one in use,
// listed right after it, switched on; the group it copies is unchanged.
func TestGroupCopy(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := provider.Save(provider.Provider{ID: "a", Name: "a", Key: "ka", Chat: "http://127.0.0.1:1/v1", Models: []string{"m1", "m2", "m3"}}); err != nil {
		t.Fatal(err)
	}
	for _, g := range []provider.Group{
		{ID: "fast", Name: "Fast", Members: []string{"a/m1", "a/m2"}, Match: []string{"a/m3*"}, Routing: provider.Ordered,
			Off: []string{"a/m2"}, Rules: []provider.Rule{{Use: "a/m2", Agents: []string{"codex"}}}},
		{ID: "slow", Name: "Slow", Members: []string{"a/m3"}},
	} {
		if err := provider.SaveGroup(g); err != nil {
			t.Fatal(err)
		}
	}
	if err := provider.SwitchGroup("fast", false); err != nil {
		t.Fatal(err)
	}
	mux := http.NewServeMux()
	groupRoutes(mux)
	post := func(body string) *httptest.ResponseRecorder {
		w := httptest.NewRecorder()
		mux.ServeHTTP(w, httptest.NewRequest("POST", "/api/groups/copy", strings.NewReader(body)))
		return w
	}
	ids := func() []string {
		var out []string
		for _, g := range provider.Groups() {
			if !g.Auto {
				out = append(out, g.ID)
			}
		}
		return out
	}

	if w := post(`{"id":"fast","name":"Fast copy"}`); w.Code != 200 {
		t.Fatalf("%d %s", w.Code, w.Body)
	}
	c, _, ok := provider.FindGroup("group/fast-copy")
	if !ok {
		t.Fatalf("no copy: %v", ids())
	}
	if c.Name != "Fast copy" || c.Routing != provider.Ordered || c.Disabled || c.Auto ||
		!slices.Equal(c.Off, []string{"a/m2"}) || !slices.Equal(c.Match, []string{"a/m3*"}) ||
		len(c.Rules) != 1 || c.Rules[0].Use != "a/m2" || !slices.Equal(c.Rules[0].Agents, []string{"codex"}) ||
		!slices.Contains(c.Members, "a/m1") || !slices.Contains(c.Members, "a/m3") {
		t.Fatalf("copy: %+v", c)
	}
	if got := ids(); !slices.Equal(got, []string{"fast", "fast-copy", "slow"}) {
		t.Errorf("order: %v", got)
	}
	if i := slices.IndexFunc(provider.Groups(), func(g provider.Group) bool { return g.ID == "fast" }); i < 0 || !provider.Groups()[i].Disabled || provider.Groups()[i].Name != "Fast" {
		t.Errorf("original changed: %v", provider.Groups())
	}

	// again: the name is in use, so it is numbered, and so is the id
	if w := post(`{"id":"fast","name":"Fast copy"}`); w.Code != 200 {
		t.Fatalf("%d %s", w.Code, w.Body)
	}
	if c, _, ok := provider.FindGroup("group/fast-copy-2"); !ok || c.Name != "Fast copy 2" {
		t.Fatalf("second copy: %v %+v (%v)", ok, c, ids())
	}
	if got := ids(); !slices.Equal(got, []string{"fast", "fast-copy-2", "fast-copy", "slow"}) {
		t.Errorf("order: %v", got)
	}
	if w := post(`{"id":"nope"}`); w.Code < 400 {
		t.Errorf("copied a group that isn't: %d", w.Code)
	}
}
