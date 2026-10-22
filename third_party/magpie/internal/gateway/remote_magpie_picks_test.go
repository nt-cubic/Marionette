package gateway

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// A magpie added as a Remote magpie lists here what its user picked there,
// and no more (GokoShiki on Discord: 24 picked on the remote, 51 here, the
// extra ones OpenRouter's). The remote's list for another magpie carried
// every OpenRouter decision model, picked or not, so each one came here as
// a model of its own. A model a plugin serves there (the Grok
// subscription's) and an API provider's (DeepSeek's) come here as picked.
func TestRemoteMagpieListsThePicks(t *testing.T) {
	// the remote magpie, under its own HOME
	besideFake(t, "grok", http.NotFoundHandler())
	var chat, decide []catalog.Model
	for i := range 40 {
		chat = append(chat, catalog.Model{ID: fmt.Sprintf("vendor/chat-%d", i)})
	}
	for i := range 27 {
		decide = append(decide, catalog.Model{ID: fmt.Sprintf("vendor/dec-%d", i)})
	}
	const or = "https://openrouter.ai/api/v1"
	for _, p := range []provider.Provider{
		{ID: "openrouter", Name: "OpenRouter", Preset: "openrouter", Key: "k", Chat: or, Models: []string{"vendor/chat-1", "vendor/chat-2", "vendor/dec-3"}},
		{ID: "deepseek", Name: "DeepSeek", Key: "k", Chat: "http://127.0.0.1:1/v1", Anthropic: "http://127.0.0.1:1/anthropic", Models: []string{"deepseek-chat"}},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	for id, ms := range map[string][]catalog.Model{"openrouter": chat, "openrouter.decisions": decide} {
		if err := catalog.SaveLive(id, or, ms); err != nil {
			t.Fatal(err)
		}
	}
	if err := catalog.SaveLive("deepseek", "http://127.0.0.1:1/v1", []catalog.Model{{ID: "deepseek-chat"}, {ID: "deepseek-reasoner"}}); err != nil {
		t.Fatal(err)
	}
	// what the remote's Gateway page shows: each provider's picks
	var picked []string
	for _, p := range provider.All() {
		for _, m := range p.Exposed() {
			picked = append(picked, p.ID+"/"+m.ID)
		}
	}
	slices.Sort(picked)
	for _, want := range []string{"deepseek/deepseek-chat", "grok/fake-1", "openrouter/vendor/dec-3"} {
		if !slices.Contains(picked, want) {
			t.Fatalf("the remote's picks %v lack %s", picked, want)
		}
	}

	// the list it gives another magpie, asked as one asks
	req := httptest.NewRequest("GET", "/v1/models", nil)
	req.Header.Set("User-Agent", "magpie")
	for _, h := range []string{provider.DrawersHeader, provider.VideomakersHeader, provider.PricesHeader, provider.DecidersHeader} {
		req.Header.Set(h, "1")
	}
	rec := httptest.NewRecorder()
	New().Handler().ServeHTTP(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("remote /v1/models: %d %s", rec.Code, rec.Body)
	}
	body := rec.Body.Bytes()

	// this magpie, under a HOME of its own, adds the remote
	fresh(t)
	remote := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/models" {
			http.NotFound(w, r)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		w.Write(body)
	}))
	t.Cleanup(remote.Close)
	id, err := provider.Add(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset, Key: "k", Chat: remote.URL})
	if err != nil {
		t.Fatal(err)
	}
	office, _ := provider.Find(id)
	if _, err := office.Fetch(context.Background()); err != nil {
		t.Fatal(err)
	}
	office, _ = provider.Find(id)
	var here []string
	for _, m := range office.Exposed() {
		here = append(here, m.ID)
	}
	slices.Sort(here)
	if !slices.Equal(here, picked) {
		var v struct{ Data []json.RawMessage }
		json.Unmarshal(body, &v)
		t.Fatalf("the remote picked %d: %v\nthis magpie lists %d: %v\n(the remote's list had %d entries)", len(picked), picked, len(here), here, len(v.Data))
	}
}
