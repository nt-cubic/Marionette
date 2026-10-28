package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// bigsteven on Discord: another computer's magpie (LAN share) serves each
// of its models from its own providers, and rests them itself. When one of
// them was out of quota there, its 429 rested the whole remote magpie here
// for the 3 hours it named, so a group whose member was another of its
// models, with quota left, went to its next member, or had none. Such a
// refusal rests only the model it was about.
func TestRemoteMagpieQuotaRestsTheModel(t *testing.T) {
	fresh(t)
	var asked []string
	remote := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Model string `json:"model"`
		}
		json.NewDecoder(r.Body).Decode(&req)
		asked = append(asked, req.Model)
		w.Header().Set("Content-Type", "application/json")
		if req.Model == "codex/gpt-5.5" {
			w.WriteHeader(http.StatusTooManyRequests)
			io.WriteString(w, `{"error":{"message":"You've hit your usage limit. Your quota resets in 3 hours.","type":"usage_limit_reached"}}`)
			return
		}
		if req.Model == "glm/glm-5" {
			w.WriteHeader(http.StatusTooManyRequests)
			io.WriteString(w, `{"error":{"message":"rate limit exceeded, slow down"}}`)
			return
		}
		io.WriteString(w, `{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":"from `+req.Model+`"},"finish_reason":"stop"}]}`)
	}))
	t.Cleanup(remote.Close)
	if err := provider.Save(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset, Key: "k",
		Chat: remote.URL + "/v1", Models: []string{"codex/gpt-5.5", "glm/glm-5", "kimi/kimi-k3"}}); err != nil {
		t.Fatal(err)
	}
	other := &keyed{}
	serveOn(t, "other", "ko", []string{"m"}, other)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"office/kimi/kimi-k3", "other/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	if code, body := postAs(t, s, "", `{"model":"office/codex/gpt-5.5","messages":[{"role":"user","content":"hi"}]}`); code != 429 {
		t.Fatalf("codex on the remote: %d %s", code, body)
	}
	if r, ok := restOf("office"); ok {
		t.Errorf("the whole remote magpie rests for one model's quota: %+v", r)
	}
	if _, ok := restOf("office/codex/gpt-5.5"); !ok {
		t.Errorf("codex/gpt-5.5 doesn't rest: %v", rests())
	}
	// and a rate limit on one of its models, the same: the group's next
	// member on that computer answers
	if err := provider.SaveGroup(provider.Group{Name: "R", Members: []string{"office/glm/glm-5", "office/kimi/kimi-k3"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	if code, body := postAs(t, s, "", `{"model":"group/r","messages":[{"role":"user","content":"hi"}]}`); code != 200 || !strings.Contains(body, "from kimi/kimi-k3") {
		t.Fatalf("glm on the remote: %d %s", code, body)
	}
	if r, ok := restOf("office"); ok {
		t.Errorf("the whole remote magpie rests for one model's rate limit: %+v", r)
	}
	if r, ok := restOf("office/glm/glm-5"); !ok || r.Why != failRate {
		t.Errorf("glm/glm-5 doesn't rest as rate limited: %+v %v", r, rests())
	}
	asked = nil
	code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "from kimi/kimi-k3") || len(other.tried) != 0 {
		t.Fatalf("the group: %d %s; the remote was asked %v, other %v", code, body, asked, other.tried)
	}
}

// One of a remote magpie's models rate limited with quota left sinks only
// its own rest (restID, as its rest is keyed): sinking the remote's whole
// key sends its other models, which were never limited, behind the next
// provider too, the same mistake resting the whole remote was.
func TestRemoteMagpieRateLimitSinksTheModel(t *testing.T) {
	fresh(t)
	forgetSinks(t)
	var mu sync.Mutex
	var asked []string
	remote := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Model string `json:"model"`
		}
		json.NewDecoder(r.Body).Decode(&req)
		mu.Lock()
		asked = append(asked, req.Model)
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		if req.Model == "glm/glm-5" {
			w.WriteHeader(http.StatusTooManyRequests)
			io.WriteString(w, `{"error":{"message":"rate limit exceeded, slow down"}}`)
			return
		}
		io.WriteString(w, `{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":"from `+req.Model+`"},"finish_reason":"stop"}]}`)
	}))
	t.Cleanup(remote.Close)
	if err := provider.Save(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset, Key: "k",
		Chat: remote.URL + "/v1", Models: []string{"glm/glm-5", "kimi/kimi-k3"}}); err != nil {
		t.Fatal(err)
	}
	other := &keyed{}
	serveOn(t, "other", "ko", []string{"m"}, other)
	// glm's rate limit, with quota left, sinks it (the group's Sink puts
	// what sank behind the others)
	if err := provider.SaveGroup(provider.Group{Name: "R", Members: []string{"office/glm/glm-5", "office/kimi/kimi-k3"}, Routing: provider.Ordered, Sink: true}); err != nil {
		t.Fatal(err)
	}
	s := New()
	if code, body := postAs(t, s, "", `{"model":"group/r","messages":[{"role":"user","content":"hi"}]}`); code != 200 || !strings.Contains(body, "from kimi/kimi-k3") {
		t.Fatalf("glm on the remote: %d %s", code, body)
	}
	// the watched group: the remote's kimi, never limited, keeps its place
	// ahead of a provider that never served this conversation
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"office/kimi/kimi-k3", "other/m"}, Routing: provider.Ordered, Sink: true}); err != nil {
		t.Fatal(err)
	}
	mu.Lock()
	asked = nil
	mu.Unlock()
	code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "from kimi/kimi-k3") || len(other.tried) != 0 {
		t.Fatalf("the group: %d %s; the remote was asked %v, other %v", code, body, asked, other.tried)
	}
}

// The same for a magpie added by hand, as a plain OpenAI-compatible
// endpoint: its reply names the provider there (providerHeader), so its
// model's 429 rests that model alone (remoteModelRest) and sinks it
// alone, and it is that model, not the endpoint's others, that the next
// Sink order sends to the back. Read at the model's own sink key, not only
// for the remote magpie preset, or it sinks nothing at all.
func TestMagpieAddedByHandRateLimitSinksTheModel(t *testing.T) {
	fresh(t)
	forgetSinks(t)
	var mu sync.Mutex
	var asked []string
	remote := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Model string `json:"model"`
		}
		json.NewDecoder(r.Body).Decode(&req)
		mu.Lock()
		asked = append(asked, req.Model)
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		if req.Model == "glm/glm-5" {
			w.Header().Set(providerHeader, "glm")
			w.WriteHeader(http.StatusTooManyRequests)
			io.WriteString(w, `{"error":{"message":"rate limit exceeded, slow down"}}`)
			return
		}
		io.WriteString(w, `{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":"from `+req.Model+`"},"finish_reason":"stop"}]}`)
	}))
	t.Cleanup(remote.Close)
	if err := provider.Save(provider.Provider{ID: "rm", Name: "Remote Magpie", Key: "k",
		Chat: remote.URL + "/v1", Models: []string{"glm/glm-5", "kimi/kimi-k3"}}); err != nil {
		t.Fatal(err)
	}
	other := &keyed{}
	serveOn(t, "other", "ko", []string{"m"}, other)
	s := New()
	group := func(name string, members ...string) string {
		t.Helper()
		if err := provider.SaveGroup(provider.Group{Name: name, Members: members, Routing: provider.Ordered, Sink: true}); err != nil {
			t.Fatal(err)
		}
		mu.Lock()
		asked = nil
		mu.Unlock()
		other.tried = nil
		code, body := postAs(t, s, "", `{"model":"group/`+strings.ToLower(name)+`","messages":[{"role":"user","content":"hi"}]}`)
		if code != 200 {
			t.Fatalf("group %s: %d %s", name, code, body)
		}
		return body
	}
	if body := group("R", "rm/glm/glm-5", "rm/kimi/kimi-k3"); !strings.Contains(body, "from kimi/kimi-k3") {
		t.Fatalf("glm there: %s", body)
	}
	// kimi there, never limited, keeps its place
	if body := group("K", "rm/kimi/kimi-k3", "other/m"); !strings.Contains(body, "from kimi/kimi-k3") || len(other.tried) != 0 {
		t.Errorf("kimi sank with glm: %s; other asked %v", body, other.tried)
	}
	// glm, limited there, went to the back: its rest over, it is still
	// behind other
	restingUntil.Lock()
	delete(restingUntil.m, "rm/glm/glm-5")
	delete(restingUntil.note, "rm/glm/glm-5")
	restingUntil.Unlock()
	if body := group("L", "rm/glm/glm-5", "other/m"); !strings.Contains(body, "from ko") || len(asked) != 0 {
		t.Errorf("glm didn't sink: %s; the remote was asked %v", body, asked)
	}
}

// UnrestAll lifts a key's own rest and each of its models', and with them
// each one's sink: a model that sank alone comes back to the front too.
func TestUnrestAllUnsinksTheModels(t *testing.T) {
	forgetSinks(t)
	now := time.Now()
	sink("office/glm/glm-5", now)
	sink("office", now)
	sink("officer/glm/glm-5", now)
	New().UnrestAll("office")
	for _, k := range []string{"office/glm/glm-5", "office"} {
		if _, ok := sunk.m[k]; ok {
			t.Errorf("%s still sunk after UnrestAll", k)
		}
	}
	if _, ok := sunk.m["officer/glm/glm-5"]; !ok {
		t.Errorf("another key's sink was lifted")
	}
}
