package gateway

import (
	"encoding/json"
	"net/http/httptest"
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// psikent on Discord: an SSH client matches what a model can do by what
// its list says, and a routing group, whose id is the user's own name,
// said nothing of tools, so the client wouldn't hand it one. Every chat
// row of /v1/models — a provider's model and a group alike — says it
// takes tools, as models.dev (tool_call) and OpenRouter
// (supported_parameters) say it, and so does the one model asked by id.
// A group of a 1M and a 128K member is told its largest window, as
// agents have been since #712: a longer conversation skips the 128K one
// (withRoom), and the group's Context setting tells its smallest instead
// (windy on Discord).
func TestGroupListedTakesTools(t *testing.T) {
	fresh(t)
	for id, n := range map[string]int{"a": 1000000, "b": 128000} {
		if err := provider.Save(provider.Provider{ID: id, Name: id, Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"m"}, Contexts: map[string]int{"m": n}}); err != nil {
			t.Fatal(err)
		}
	}
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	type row struct {
		ID       string   `json:"id"`
		ToolCall bool     `json:"tool_call"`
		Params   []string `json:"supported_parameters"`
		Window   int      `json:"context_window"`
	}
	get := func(path string, into any) {
		t.Helper()
		rec := httptest.NewRecorder()
		New().Handler().ServeHTTP(rec, httptest.NewRequest("GET", path, nil))
		if err := json.Unmarshal(rec.Body.Bytes(), into); err != nil {
			t.Fatalf("%s: %d %s", path, rec.Code, rec.Body.String())
		}
	}
	var list struct{ Data []row }
	get("/v1/models", &list)
	seen := map[string]row{}
	for _, r := range list.Data {
		seen[r.ID] = r
	}
	for _, id := range []string{"a/m", "b/m", "group/g"} {
		r, ok := seen[id]
		if !ok {
			t.Fatalf("%s not listed: %+v", id, list.Data)
		}
		if !r.ToolCall || !slices.Contains(r.Params, "tools") || !slices.Contains(r.Params, "tool_choice") {
			t.Errorf("%s doesn't say it takes tools: %+v", id, r)
		}
	}
	if w := seen["group/g"].Window; w != 1000000 {
		t.Errorf("group window %d, want its largest member's 1000000", w)
	}
	var one row
	get("/v1/models/group/g", &one)
	if !one.ToolCall || !slices.Contains(one.Params, "tools") {
		t.Errorf("the group asked by id doesn't say it takes tools: %+v", one)
	}
}
