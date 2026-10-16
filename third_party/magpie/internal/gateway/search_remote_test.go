package gateway

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// A Remote magpie searches the web for the models it serves, and its list
// says how (Player on Discord: Claude Code's WebSearch on remoteMagpie's
// glm-5.3 went to this magpie's searcher, another model, and Settings'
// Searcher didn't offer the remote's models). Claude Code's web search
// goes on to it as it was offered; the models it searches natively for
// are ones Settings may name, and a search run for another model is
// marked, so the other magpie only lets the model itself search for it.
func TestRemoteMagpieSearches(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	setHome(t, t.TempDir()) // no signed-in agent searches
	var mu sync.Mutex
	var vendor []string // the bodies the searching vendor was sent
	zp := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodGet {
			io.WriteString(w, `{"data":[{"id":"glm-5.3"}]}`)
			return
		}
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		vendor = append(vendor, string(b))
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"m","type":"message","role":"assistant","model":"glm-5.3","content":[`+
			`{"type":"server_tool_use","id":"s1","name":"web_search","input":{"query":"go"}},`+
			`{"type":"web_search_tool_result","tool_use_id":"s1","content":[{"type":"web_search_result","title":"Go","url":"https://go.dev/"}]},`+
			`{"type":"text","text":"Go 1.27.1 is out."}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":4}}`)
	}))
	t.Cleanup(zp.Close)
	plain := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.WriteString(w, `{"data":[{"id":"m1"}]}`)
	}))
	t.Cleanup(plain.Close)
	hosts := searchHosts[provider.Anthropic]
	searchHosts[provider.Anthropic] = append(slices.Clone(hosts), provider.HostOf(zp.URL))
	t.Cleanup(func() { searchHosts[provider.Anthropic] = hosts })
	for _, p := range []provider.Provider{
		{ID: "zhipu", Name: "Zhipu", Key: "k", Anthropic: zp.URL},
		{ID: "plain", Name: "Plain", Key: "k", Chat: plain.URL + "/v1"},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
		if _, err := p.Fetch(context.Background()); err != nil {
			t.Fatal(err)
		}
	}
	h := New().Handler()
	var hops []string // what the remote magpie was sent, and how
	remote := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodPost {
			b, _ := io.ReadAll(r.Body)
			r.Body = io.NopCloser(strings.NewReader(string(b)))
			mu.Lock()
			hops = append(hops, r.Header.Get(SearchingHeader)+" "+string(b))
			mu.Unlock()
		}
		h.ServeHTTP(w, r)
	}))
	t.Cleanup(remote.Close)
	id, err := provider.Add(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset, Chat: strings.TrimPrefix(remote.URL, "http://")})
	if err != nil {
		t.Fatal(err)
	}
	office, _ := provider.Find(id)
	if _, err := office.Fetch(context.Background()); err != nil {
		t.Fatal(err)
	}

	// its list says how it searches for each model: by the vendor itself,
	// or with its own searcher
	live, _, _ := catalog.Live(id)
	how := map[string]string{}
	for _, m := range live {
		how[m.ID] = m.WebSearch
	}
	if how["zhipu/glm-5.3"] != searchNative || how["plain/m1"] != searchMagpie {
		t.Fatalf("the remote's list says it searches %v", how)
	}

	// Claude Code's WebSearch on a model of it goes there as it is, and on
	// to the vendor, which searches
	body := `{"model":"` + id + `/zhipu/glm-5.3","max_tokens":1000,"messages":[{"role":"user","content":"What is the latest Go?"}],
		"tools":[{"name":"Read","description":"read","input_schema":{"type":"object"}},{"type":"web_search_20250305","name":"web_search","max_uses":8}]}`
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodPost, "/v1/messages", strings.NewReader(body))
	req.Header.Set("User-Agent", "claude-cli/2.1.0 (external, cli)")
	h.ServeHTTP(rec, req)
	if rec.Code != 200 || !strings.Contains(rec.Body.String(), "Go 1.27.1") {
		t.Fatalf("%d %s", rec.Code, rec.Body.String())
	}
	mu.Lock()
	sent, asked := slices.Clone(hops), slices.Clone(vendor)
	hops, vendor = nil, nil
	mu.Unlock()
	if len(sent) != 1 || !strings.Contains(sent[0], `"web_search_20250305"`) || strings.HasPrefix(sent[0], "1") {
		t.Fatalf("the remote magpie was sent %q", sent)
	}
	if len(asked) != 1 || !strings.Contains(asked[0], `"web_search_20250305"`) {
		t.Fatalf("the vendor was sent %q", asked)
	}

	// Settings may name it to search with, with the model it searches by
	// itself for, and not the one it would search for with another
	var choice *SearcherChoice
	for _, c := range Searchers() {
		if c.Provider.ID == id {
			choice = &c
		}
	}
	if choice == nil {
		t.Fatalf("the remote magpie isn't a searcher: %v", Searchers())
	}
	if choice.Small != "zhipu/glm-5.3" || slices.ContainsFunc(choice.Models, func(m catalog.Model) bool { return m.ID == "plain/m1" }) {
		t.Fatalf("it searches with %s of %v", choice.Small, choice.Models)
	}
	st := settings.Load()
	st.Searcher = id + "/zhipu/glm-5.3"
	if err := settings.Save(st); err != nil {
		t.Fatal(err)
	}
	if p, m, ok := searcher(); !ok || p.ID != id || m != "zhipu/glm-5.3" {
		t.Fatalf("searcher = %s %s %v", p.ID, m, ok)
	}

	// a search for plain's model goes to it, marked as a search, and the
	// vendor searches
	said, _, err := New().modelSearch(context.Background(), "the latest Go")
	if err != nil || !strings.Contains(said, "Go 1.27.1") {
		t.Fatalf("search said %q, %v", said, err)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(hops) != 1 || !strings.HasPrefix(hops[0], "1 ") {
		t.Fatalf("the remote magpie was sent %q", hops)
	}
}

// A request another magpie marks as a search is one the model searches
// for alone: this magpie runs no searcher of its own for it.
func TestSearchingFromAnotherMagpie(t *testing.T) {
	var seen bool
	h := withCaller(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		seen = searching(r.Context()) && r.Header.Get(SearchingHeader) == ""
	}))
	for _, c := range []struct {
		ua   string
		want bool
	}{{"magpie/0.1.983", true}, {"curl/8", false}} {
		r := httptest.NewRequest(http.MethodPost, "/v1/messages", nil)
		r.Header.Set("User-Agent", c.ua)
		r.Header.Set(SearchingHeader, "1")
		seen = false
		h.ServeHTTP(httptest.NewRecorder(), r)
		if seen != c.want {
			t.Errorf("%s: searching = %v", c.ua, seen)
		}
	}
}
