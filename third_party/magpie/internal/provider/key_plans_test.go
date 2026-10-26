package provider

import (
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

func keyPlanHome(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
}

// volcMixed is yckonnnn's provider (on X): Volcengine at its Coding Plan,
// with a pay-as-you-go key and an Agent Plan key beside the plan's own.
func volcMixed() Provider {
	pr := Preset("volcengine")
	return Provider{ID: "volc", Name: "Volc", Preset: "volcengine", Chat: pr.Chat, Responses: pr.Responses, Anthropic: pr.Anthropic,
		Key: "k-coding", Keys: []KeyAccount{{Key: "k-api", Plan: "api"}, {Key: "k-agent", Plan: "agent"}}}
}

// Each key goes to its own plan's endpoints, on every protocol; a key that
// names no plan stays on the provider's.
func TestKeyPlanEndpoints(t *testing.T) {
	p := volcMixed()
	if !p.PlansMixed() {
		t.Fatal("keys on three plans read as one")
	}
	for _, c := range []struct{ key, chat, responses, anthropic string }{
		{"k-coding", "https://ark.cn-beijing.volces.com/api/coding/v3", "https://ark.cn-beijing.volces.com/api/coding/v3", "https://ark.cn-beijing.volces.com/api/coding"},
		{"k-api", "https://ark.cn-beijing.volces.com/api/v3", "https://ark.cn-beijing.volces.com/api/v3", ""},
		{"k-agent", "https://ark.cn-beijing.volces.com/api/plan/v3", "https://ark.cn-beijing.volces.com/api/plan/v3", "https://ark.cn-beijing.volces.com/api/plan"},
	} {
		var k KeyAccount
		for _, x := range p.KeysOn() {
			if x.Key == c.key {
				k = x
			}
		}
		q := p.WithKey(k)
		if q.Chat != c.chat || q.Responses != c.responses || q.Anthropic != c.anthropic {
			t.Errorf("%s at %q %q %q", c.key, q.Chat, q.Responses, q.Anthropic)
		}
		if q.PlansMixed() {
			t.Errorf("%s: one key's copy judged mixed again", c.key)
		}
	}
	// at a host of its own (a mirror, a fake vendor) each plan is asked there
	p.Chat, p.Responses, p.Anthropic = "http://127.0.0.1:9/api/coding/v3", "http://127.0.0.1:9/api/coding/v3", "http://127.0.0.1:9/api/coding"
	if q := p.WithKey(p.Keys[1]); q.Chat != "http://127.0.0.1:9/api/plan/v3" || q.Anthropic != "http://127.0.0.1:9/api/plan" {
		t.Errorf("agent key at its own host: %q %q", q.Chat, q.Anthropic)
	}
}

// A model only one plan has goes only to that plan's key; one every plan
// has goes to every key; a model the user set a key to serve it serves.
func TestKeyPlanServing(t *testing.T) {
	p := volcMixed()
	serves := func(model string) string {
		f := p.PlanServing(model)
		var on []string
		for _, k := range p.KeysOn() {
			if f(k) {
				on = append(on, k.Key)
			}
		}
		return strings.Join(on, ",")
	}
	for model, want := range map[string]string{
		"deepseek-v3-250324":  "k-api",                  // pay as you go's alone
		"doubao-seedance-2.0": "k-api,k-agent",          // Agent Plan's video, and the open list
		"kimi-k3":             "k-coding,k-api,k-agent", // every plan
		"Kimi-K3":             "k-coding,k-api,k-agent",
	} {
		if got := serves(model); got != want {
			t.Errorf("%s: %s, want %s", model, got, want)
		}
	}
	p.AccountModels = map[string][]string{keyID("k-coding"): {"deepseek-v3-250324"}}
	if got := serves("deepseek-v3-250324"); got != "k-coding,k-api" {
		t.Errorf("a key set to serve a model: %s", got)
	}
}

// Old configs are as they were: keys that name no plan, a plan that is the
// provider's own, a plan the preset doesn't have, or a preset whose keys
// are all on one plan.
func TestKeyPlanOldConfigsUnchanged(t *testing.T) {
	keyPlanHome(t)
	for name, p := range map[string]Provider{
		"no plans": func() Provider { p := volcMixed(); p.Keys[0].Plan, p.Keys[1].Plan = "", ""; return p }(),
		"own plan": func() Provider {
			p := volcMixed()
			p.KeyPlan, p.Keys[0].Plan, p.Keys[1].Plan = "coding", "coding", "coding"
			return p
		}(),
		"gone":     func() Provider { p := volcMixed(); p.Keys[0].Plan, p.Keys[1].Plan = "lite", "pro"; return p }(),
		"one plan": {ID: "q", Name: "Q", Preset: "qwen", Chat: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1", Key: "a", Keys: []KeyAccount{{Key: "b", Plan: "api"}}},
	} {
		if p.PlansMixed() {
			t.Errorf("%s: mixed", name)
		}
		for _, k := range p.KeysOn() {
			if q := p.WithKey(k); q.Chat != p.Chat || q.Anthropic != p.Anthropic {
				t.Errorf("%s: key %s moved to %q", name, k.Key, q.Chat)
			}
			if !p.PlanServing("deepseek-v3-250324")(k) {
				t.Errorf("%s: key %s barred", name, k.Key)
			}
		}
		if got := planViews([]Provider{p}); len(got) != 1 || got[0].Key != p.Key || len(got[0].Keys) != len(p.Keys) {
			t.Errorf("%s: views %+v", name, got)
		}
	}
	// a saved provider that names no plan is written as before
	p := volcMixed()
	p.Keys[0].Plan, p.Keys[1].Plan = "", ""
	if err := Save(p); err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(Path())
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(b), `"plan"`) || strings.Contains(string(b), `"keyPlan"`) {
		t.Errorf("a plan written for keys that name none:\n%s", b)
	}
}

// SetKeyPlan keeps the provider's own plan as none, refuses a plan the
// preset doesn't have and a preset whose keys are on one plan.
func TestSetKeyPlan(t *testing.T) {
	keyPlanHome(t)
	p := volcMixed()
	p.Keys[0].Plan, p.Keys[1].Plan = "", ""
	if err := Save(p); err != nil {
		t.Fatal(err)
	}
	if err := SetKeyPlan("volc", keyID("k-api"), "api"); err != nil {
		t.Fatal(err)
	}
	if err := SetKeyPlan("volc", keyID("k-coding"), "agent"); err != nil {
		t.Fatal(err)
	}
	if err := SetKeyPlan("volc", keyID("k-agent"), "coding"); err != nil {
		t.Fatal(err)
	}
	got, err := Find("volc")
	if err != nil {
		t.Fatal(err)
	}
	if got.KeyPlan != "agent" || got.Keys[0].Plan != "api" || got.Keys[1].Plan != "" {
		t.Fatalf("plans %q %q %q", got.KeyPlan, got.Keys[0].Plan, got.Keys[1].Plan)
	}
	if l := got.KeyList(); l[0].Plan != "agent" || l[1].Plan != "api" {
		t.Errorf("KeyList %+v", l)
	}
	if err := SetKeyPlan("volc", keyID("k-api"), "nope"); err == nil {
		t.Error("a plan the preset doesn't have")
	}
	if err := Save(Provider{ID: "q", Name: "Q", Preset: "qwen", Chat: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1", Key: "a"}); err != nil {
		t.Fatal(err)
	}
	if err := SetKeyPlan("q", keyID("a"), "api"); err == nil {
		t.Error("a plan for a preset whose keys are on one")
	}
}

// A provider of keys on mixed plans lists each key's plan: the Coding
// Plan's models as the preset says, without asking (its /models is Ark's
// general catalog), pay as you go's from its own /models with its key.
func TestKeyPlanModelList(t *testing.T) {
	keyPlanHome(t)
	var mu sync.Mutex
	var asked []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		asked = append(asked, r.Header.Get("Authorization")+" "+r.URL.Path)
		mu.Unlock()
		_, _ = w.Write([]byte(`{"data":[{"id":"deepseek-v3-250324"},{"id":"kimi-k3"}]}`))
	}))
	defer srv.Close()
	p := Provider{ID: "volc", Name: "Volc", Preset: "volcengine", Chat: srv.URL + "/api/coding/v3", Responses: srv.URL + "/api/coding/v3", Anthropic: srv.URL + "/api/coding",
		Key: "k-coding", Keys: []KeyAccount{{Key: "k-api", Plan: "api"}}}
	ms, err := p.fetch(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if strings.Join(asked, ",") != "Bearer k-api /api/v3/models" {
		t.Fatalf("asked %v", asked)
	}
	have := map[string]bool{}
	for _, m := range ms {
		have[m.ID] = true
	}
	if !have["deepseek-v3-250324"] || !have["ark-code-latest"] || !have["kimi-k3"] {
		t.Fatalf("models %v", have)
	}
	if p.Serves(p.first(), "deepseek-v3-250324") || !p.Serves(p.Keys[0], "deepseek-v3-250324") {
		t.Error("pay as you go's model served by the Coding Plan key")
	}
	if !p.Serves(p.first(), "kimi-k3") || !p.Serves(p.Keys[0], "kimi-k3") {
		t.Error("a model both plans have")
	}
}

// Each plan a provider has keys of is its own view, with those keys alone,
// at that plan's endpoints: windows read per plan.
func TestKeyPlanViews(t *testing.T) {
	v := planViews([]Provider{volcMixed()})
	if len(v) != 3 {
		t.Fatalf("%d views", len(v))
	}
	for i, want := range []string{"k-coding /api/coding/v3", "k-api /api/v3", "k-agent /api/plan/v3"} {
		if got := v[i].Key + " " + strings.TrimPrefix(v[i].Chat, "https://ark.cn-beijing.volces.com"); got != want || len(v[i].Keys) != 0 {
			t.Errorf("view %d: %s (%d more keys), want %s", i, got, len(v[i].Keys), want)
		}
	}
}
