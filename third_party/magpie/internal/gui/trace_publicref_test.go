package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/usage"
)

// The Routing page names a group magpie found as agents pick it,
// auto/<model>, wherever a route names one: the model the agent asked
// for, the group and the groups in it, the request a search was for and a
// subagent's pick (未寒 on Discord: the live view still said
// group/auto-<model> after the rename). The gateway's trace and history
// keep group/auto-<slug>; a group the user made stays group/<id>.
func TestRoutesNameFoundGroupsByTheirPublicID(t *testing.T) {
	sandboxHome(t)
	old := served.Swap(nil)
	t.Cleanup(func() { served.Store(old) })
	if err := os.MkdirAll(filepath.Dir(catalog.CachePath()), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(catalog.CachePath(), []byte(`{"anthropic": {"models": {
	  "claude-opus-5.5": {"id":"claude-opus-5.5","name":"Claude Opus 5.5","limit":{"context":400000,"output":96000}}}}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	for id, m := range map[string]string{"a": "claude-opus-5-5", "b": "claude-opus-5.5"} {
		if err := provider.Save(provider.Provider{ID: id, Name: id, Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{m, "x"}}); err != nil {
			t.Fatal(err)
		}
	}
	if err := provider.SaveGroup(provider.Group{Name: "Mine", Members: []string{"a/x", "b/x"}}); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(gateway.HistoryDir(), 0o700); err != nil {
		t.Fatal(err)
	}
	// as the gateway wrote them, before the trace kept a group's ref
	lines := `{"id":1,"model":"group/auto-claude-opus-5-5:high","provider":"a","done":true,` +
		`"group":{"id":"auto-claude-opus-5-5","name":"Claude Opus 5.5","routing":"","affinity":"","members":["a/claude-opus-5-5"]},` +
		`"subagent":{"asked":"group/auto-claude-opus-5-5","to":"group/mine"}}` + "\n" +
		`{"id":2,"model":"group/mine","provider":"a","done":true,"kind":"web_search","for":{"agent":"claude","model":"group/auto-claude-opus-5-5"},` +
		`"group":{"id":"mine","name":"Mine","routing":"","affinity":"","members":["a/x"],"subs":[{"id":"auto-claude-opus-5-5","name":"Claude Opus 5.5","routing":"","in":"mine"}]}}` + "\n"
	if err := os.WriteFile(filepath.Join(gateway.HistoryDir(), "2026-10-20.jsonl"), []byte(lines), 0o600); err != nil {
		t.Fatal(err)
	}
	mux := http.NewServeMux()
	traceRoutes(mux)
	w := httptest.NewRecorder()
	mux.ServeHTTP(w, httptest.NewRequest("GET", "/api/gateway/history?day=2026-10-20", nil))
	var got struct {
		Routes []gateway.Route `json:"routes"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &got); err != nil || len(got.Routes) != 2 {
		t.Fatalf("history: %v %s", err, w.Body)
	}
	byID := map[int64]gateway.Route{}
	for _, r := range got.Routes {
		byID[r.ID] = r
	}
	one, two := byID[1], byID[2]
	if one.Model != "auto/claude-opus-5.5:high" {
		t.Errorf("the model asked for: %q", one.Model)
	}
	if one.Group == nil || one.Group.Ref != "auto/claude-opus-5.5" || one.Group.ID != "auto-claude-opus-5-5" {
		t.Errorf("the group: %+v", one.Group)
	}
	if s := one.Subagent; s == nil || s.Asked != "auto/claude-opus-5.5" || s.To != "group/mine" {
		t.Errorf("the subagent's pick: %+v", s)
	}
	if two.Model != "group/mine" || two.Group == nil || two.Group.Ref != "group/mine" {
		t.Errorf("the user's group: %q %+v", two.Model, two.Group)
	}
	if two.For == nil || two.For.Model != "auto/claude-opus-5.5" {
		t.Errorf("what the search was for: %+v", two.For)
	}
	if two.Group == nil || len(two.Group.Subs) != 1 || two.Group.Subs[0].Ref != "auto/claude-opus-5.5" {
		t.Errorf("the group in the group: %+v", two.Group)
	}

	// the request's own route, as the panel's link opens it
	w = httptest.NewRecorder()
	mux.ServeHTTP(w, httptest.NewRequest("GET", "/api/gateway/route?id=1&day=2026-10-20", nil))
	var r gateway.Route
	if err := json.Unmarshal(w.Body.Bytes(), &r); err != nil || r.Model != "auto/claude-opus-5.5:high" {
		t.Errorf("one route: %q %v", r.Model, err)
	}

	// the Context page's sessions and agents, of the same routes (today's)
	h := filepath.Join(gateway.HistoryDir(), time.Now().Format("2006-01-02")+".jsonl")
	if err := os.WriteFile(h, []byte(lines), 0o600); err != nil {
		t.Fatal(err)
	}
	found := false
	for _, r := range contextRoutes(httptest.NewRequest("GET", "/api/context", nil), 1) {
		if r.ID == 1 {
			found = true
			if r.Model != "auto/claude-opus-5.5:high" {
				t.Errorf("the Context page's model: %q", r.Model)
			}
		}
	}
	if !found {
		t.Error("the Context page has no route 1")
	}

	// Usage › Requests' Requested column, which the ledger keeps the same way
	for _, req := range []string{"group/auto-claude-opus-5-5", "group/mine"} {
		usage.Append(usage.Record{Time: time.Now(), Provider: "a", Model: "claude-opus-5-5", Requested: req, Status: 200, Input: 10})
	}
	usageRoutes(mux, folderOnly{})
	w = httptest.NewRecorder()
	mux.ServeHTTP(w, httptest.NewRequest("GET", "/api/usage/requests?period=all", nil))
	var ledger ledgerJSON
	if err := json.Unmarshal(w.Body.Bytes(), &ledger); err != nil {
		t.Fatal(err, w.Body)
	}
	var asked []string
	for _, row := range ledger.Rows {
		asked = append(asked, row.Requested)
	}
	slices.Sort(asked)
	if !slices.Equal(asked, []string{"auto/claude-opus-5.5", "group/mine"}) {
		t.Errorf("the ledger's Requested: %q", asked)
	}
}
