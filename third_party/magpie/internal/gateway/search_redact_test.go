package gateway

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// A search query that leaves magpie for a search service that isn't a
// model's endpoint is masked as the settings say: an agent's search MCP
// call carries it as the agent's model wrote it, the values put back, and
// it went to the search API, a Kimi Code plan's or WorkBuddy's search as
// it was (lc on Discord: "mcp 搜索出去的脱敏了吗？"). The agent gets its own
// values back in what the search found.
func TestSearchQueryMasked(t *testing.T) {
	const phone = "13812345678"
	query := "who called from " + phone

	fresh(t)
	if err := settings.Save(settings.Settings{RedactPersonal: true}); err != nil {
		t.Fatal(err)
	}
	var mu sync.Mutex
	var asked []string
	api := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		q := r.URL.Query().Get("q")
		mu.Lock()
		asked = append(asked, q)
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		b, _ := json.Marshal(map[string]any{"results": []map[string]any{{"title": "Directory", "url": "https://example.com/", "content": "Results for " + q}}})
		w.Write(b)
	}))
	defer api.Close()
	if err := provider.SetSearchAPI(provider.SearchAPI{Vendor: "searxng", URL: api.URL}); err != nil {
		t.Fatal(err)
	}
	call := `{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"web_search","arguments":{"query":"` + query + `"}}}`
	m := readMCP(t, mcpPost(lanGuard(New().Handler()), "127.0.0.1:50000", call))
	mu.Lock()
	if len(asked) != 1 || strings.Contains(asked[0], phone) || !strings.Contains(asked[0], "{{PHONE_") {
		t.Fatalf("the search API was asked %q", asked)
	}
	mu.Unlock()
	if m.Result.IsError || len(m.Result.Content) != 1 || !strings.Contains(m.Result.Content[0].Text, "Results for "+query) {
		t.Fatalf("the agent got %+v", m)
	}

	// a Kimi Code plan's search service
	k := newKimiPlan(t)
	kimi := provider.Provider{ID: "kimi", Name: "Kimi Code", Preset: "kimi-code-cn", Key: "sk-kimi", Chat: k.URL + "/coding/v1"}
	if _, _, err := New().kimiSearch(context.Background(), kimi, query); err != nil {
		t.Fatal(err)
	}
	k.mu.Lock()
	if len(k.searched) != 1 || strings.Contains(k.searched[0], phone) || !strings.Contains(k.searched[0], "{{PHONE_") {
		t.Fatalf("Kimi was asked %q", k.searched)
	}
	k.mu.Unlock()
	// unless the user set the plan to go unmasked
	kimi.Unredacted = true
	if _, _, err := New().kimiSearch(context.Background(), kimi, query); err != nil {
		t.Fatal(err)
	}
	k.mu.Lock()
	if len(k.searched) != 2 || !strings.HasPrefix(k.searched[1], query+" |") {
		t.Fatalf("an unmasked Kimi was asked %q", k.searched)
	}
	k.mu.Unlock()

	// WorkBuddy's search
	site := &wbSearchSite{}
	p := wbSignedIn(t, "workbuddy", "workbuddy-desktop", site)
	if err := settings.Save(settings.Settings{RedactPersonal: true}); err != nil {
		t.Fatal(err)
	}
	if _, err := p.Fetch(context.Background()); err != nil {
		t.Fatal(err)
	}
	if _, _, err := New().wbSearch(context.Background(), p, query); err != nil {
		t.Fatal(err)
	}
	site.mu.Lock()
	defer site.mu.Unlock()
	if len(site.searched) != 1 {
		t.Fatalf("WorkBuddy searched %v", site.searched)
	}
	if q, _ := site.searched[0]["query"].(string); strings.Contains(q, phone) || !strings.Contains(q, "{{PHONE_") {
		t.Fatalf("WorkBuddy was asked %q", q)
	}
}
