package provider

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

// Bailian's decision model (#647): a preset at the host of the key's
// workspace or the Token Plan's, asked as decision-model-preview.
func TestBailianDecisionPreset(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	p, err := FromPreset("bailian-decision")
	if err != nil {
		t.Fatal(err)
	}
	if !p.DecideOnly() || !strings.Contains(p.Decide, WorkspaceID) {
		t.Fatalf("preset: %+v", p)
	}
	p.Key = "sk-test"
	if _, err := Add(p); err == nil || !strings.Contains(err.Error(), "workspace ID") {
		t.Fatalf("added with no workspace: %v", err)
	}
	pr := Preset("bailian-decision")
	var plans []string
	for _, r := range pr.Regions {
		plans = append(plans, r.ID+" "+r.Decide)
	}
	want := []string{
		"cn-beijing https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
		"ap-southeast-1 https://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
		"token-plan https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
	}
	if !slices.Equal(plans, want) {
		t.Fatalf("plans: %q", plans)
	}
	p.Decide = strings.ReplaceAll(p.Decide, WorkspaceID, "ws-1")
	id, err := Add(p)
	if err != nil {
		t.Fatal(err)
	}
	saved, err := Find(id)
	if err != nil {
		t.Fatal(err)
	}
	if saved.DecideVia() != ViaSystemOne || saved.Jev() != BailianDecision {
		t.Fatalf("asked as %s via %s", saved.Jev(), saved.DecideVia())
	}
	if ds := Deciders(); len(ds) != 1 || ds[0].ID != id+"/"+BailianDecision {
		t.Fatalf("classifiers: %+v", ds)
	}
	for _, e := range providerEntries() {
		if e.Provider.ID == id {
			t.Fatalf("an agent is offered %s", e.ID)
		}
	}
	if got, m, err := RouteDecider(id); err != nil || got.ID != id || m != BailianDecision {
		t.Fatalf("route %s: %s %s %v", id, got.ID, m, err)
	}
	if got, m, err := RouteDecider(BailianDecision); err != nil || got.ID != id || m != BailianDecision {
		t.Fatalf("route %s: %s %s %v", BailianDecision, got.ID, m, err)
	}
}

// a System One API of a vendor's own: its model is named anything, its
// list (none here) needn't name it, and it is asked at POST …/systemone
func TestCustomSystemOne(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	asked := 0
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.URL.Path != "/compatible-mode/v1/systemone" {
			http.NotFound(w, r)
			return
		}
		var q struct {
			Model     string                    `json:"model"`
			State     map[string]any            `json:"state"`
			Questions map[string]map[string]any `json:"questions"`
		}
		json.NewDecoder(r.Body).Decode(&q)
		if r.Header.Get("Authorization") != "Bearer sk-1" {
			w.WriteHeader(401)
			w.Write([]byte(`{"error":{"message":"Invalid API-key provided."}}`))
			return
		}
		if q.Model != "my-decider" || len(q.State) == 0 || len(q.Questions) == 0 {
			w.WriteHeader(400)
			w.Write([]byte(`{"error":{"message":"bad request"}}`))
			return
		}
		asked++
		w.Write([]byte(`{"model":"my-decider","answers":{"ok":{"type":"noul","noul":0.99}},"usage":{"input_tokens":12}}`))
	}))
	defer up.Close()
	p := Provider{ID: "dec", Name: "Decider", Key: "sk-1", Decide: up.URL + "/compatible-mode/v1", Models: []string{"my-decider"}}
	if err := Save(p); err != nil {
		t.Fatal(err)
	}
	if p.Jev() != "my-decider" {
		t.Fatalf("asked as %s", p.Jev())
	}
	ms, err := p.Fetch(context.Background())
	if err != nil || len(ms) != 1 || ms[0].ID != "my-decider" || asked != 1 {
		t.Fatalf("fetch: %+v %v (asked %d)", ms, err, asked)
	}
	if ds := Deciders(); len(ds) != 1 || ds[0].ID != "dec/my-decider" {
		t.Fatalf("classifiers: %+v", ds)
	}
	if rs := p.Test(context.Background()); len(rs) != 1 || !rs[0].OK || rs[0].Model != "my-decider" {
		t.Fatalf("test: %+v", rs)
	}
	// the editor's probe, a new provider with the address pasted whole
	q := Provider{Key: "sk-1", Decide: up.URL + "/compatible-mode/v1/systemone/"}
	if d := q.DetectDecide(context.Background(), "my-decider"); !d.OK || d.Base != up.URL+"/compatible-mode/v1" || d.Protocol != "decide" {
		t.Fatalf("probe: %+v", d)
	}
	q.Key = "sk-2"
	if d := q.DetectDecide(context.Background(), "my-decider"); d.OK || !strings.Contains(d.Error, "Invalid API-key") {
		t.Fatalf("probe with a wrong key: %+v", d)
	}
	if d := (Provider{Decide: "https://" + WorkspaceID + ".cn-beijing.maas.aliyuncs.com/compatible-mode/v1"}).DetectDecide(context.Background(), ""); d.OK || d.Model != BailianDecision || !strings.Contains(d.Error, "workspace") {
		t.Fatalf("probe with no workspace: %+v", d)
	}
}
