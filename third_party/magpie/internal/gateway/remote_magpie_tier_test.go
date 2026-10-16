package gateway

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// Codex is offered Fast on another magpie's model where that magpie offers
// its own Codex Fast on it, and a turn in Fast mode goes on to that magpie
// with service_tier priority, which it sends its vendor as its own Codex's
// turn would be (PennyVibe, #1234: remote-magpie/codex/gpt-6.1-sol had no
// tiers while sub2api/gpt-6.1-sol had Fast). A model it offers no tier on
// is offered none here either. Here the gateway is both computers', as in
// TestRemoteMagpieNativeAPI: "office" is it, reached over HTTP, and its
// vendor is a relay the user added by its address.
func TestRemoteMagpieOffersItsTiers(t *testing.T) {
	var mu sync.Mutex
	var vendor, hops []string // service_tier as the vendor, and the remote magpie, were sent it
	tierOf := func(b []byte) string {
		var v struct {
			Tier *string `json:"service_tier"`
		}
		json.Unmarshal(b, &v)
		if v.Tier == nil {
			return "(none)"
		}
		return *v.Tier
	}
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		vendor = append(vendor, tierOf(b))
		mu.Unlock()
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(
			`data: {"type":"response.created","response":{"id":"resp_1","model":"gpt-6.1-sol"}}`,
			`data: {"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"m1","role":"assistant","content":[]}}`,
			`data: {"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"hello"}`,
			`data: {"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"m1","role":"assistant","content":[{"type":"output_text","text":"hello"}]}}`,
			`data: {"type":"response.completed","response":{"id":"resp_1","model":"gpt-6.1-sol","usage":{"input_tokens":5,"output_tokens":2}}}`))
	}))
	t.Cleanup(up.Close)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "k", Models: []string{"gpt-6.1-sol", "glm-5"}, Responses: up.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	h := New().Handler()
	remote := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodPost {
			b, _ := io.ReadAll(r.Body)
			r.Body = io.NopCloser(strings.NewReader(string(b)))
			mu.Lock()
			hops = append(hops, tierOf(b))
			mu.Unlock()
		}
		h.ServeHTTP(w, r)
	}))
	t.Cleanup(remote.Close)
	id, err := provider.Add(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset, Chat: strings.TrimPrefix(remote.URL, "http://")})
	if err != nil {
		t.Fatal(err)
	}
	office, err := provider.Find(id)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := office.Fetch(context.Background()); err != nil {
		t.Fatal(err)
	}

	// the catalog Codex here reads
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, httptest.NewRequest("GET", CodexCatalogPath, nil))
	var cat struct {
		Models []struct {
			Slug  string `json:"slug"`
			Tiers []struct {
				ID   string `json:"id"`
				Name string `json:"name"`
			} `json:"service_tiers"`
		} `json:"models"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &cat); err != nil {
		t.Fatalf("%v: %s", err, rec.Body)
	}
	offered := map[string]string{}
	for _, m := range cat.Models {
		s := ""
		for _, x := range m.Tiers {
			s += x.ID + ":" + x.Name + " "
		}
		offered[m.Slug] = s
	}
	for slug, want := range map[string]string{
		"relay/gpt-6.1-sol": "priority:Fast ", "office/relay/gpt-6.1-sol": "priority:Fast ",
		"relay/glm-5": "", "office/relay/glm-5": "",
	} {
		if got, ok := offered[slug]; !ok || got != want {
			t.Errorf("%s offered %q (listed %v), want %q", slug, got, ok, want)
		}
	}

	// a turn in Fast mode, as Codex sends it
	ask := strings.Replace(codexFastAsk("priority"), `"model":"m1"`, `"model":"office/relay/gpt-6.1-sol"`, 1)
	code, body := post(t, "/v1/responses", ask)
	if code != 200 {
		t.Fatalf("%d: %s", code, body)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(hops) != 1 || hops[0] != "priority" || len(vendor) != 1 || vendor[0] != "priority" {
		t.Errorf("tier sent to the remote magpie %v, by it to its vendor %v; want priority, priority", hops, vendor)
	}
}
