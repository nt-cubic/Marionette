package gateway

import (
	"encoding/json"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// #1438 (yhong91): /v1/models published grok/grok-4.7 with a 256000
// window, the Grok backend's own, beside max_output_tokens 500000, which
// models.dev gives x-ai's grok-4.7. Every model list the gateway serves now
// keeps the reply limit within the window beside it, as the agents' files
// have since #338; an unknown window or output keeps its shape, and a
// request is still lowered only to the model's own limit.
func TestModelListsKeepOutputWithinTheWindow(t *testing.T) {
	f := &fake{ctype: "application/json", reply: `{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}`}
	up := setup(t, provider.Chat, f)
	// models.dev's entry, as the reporter's cache has it
	os.MkdirAll(filepath.Dir(catalog.CachePath()), 0o755)
	if err := os.WriteFile(catalog.CachePath(), []byte(`{"xai":{"id":"xai","models":{"grok-4.7":{"id":"grok-4.7","limit":{"context":500000,"output":500000}}}}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	if err := provider.Save(provider.Provider{ID: "gx", Name: "GX", Key: "k", Chat: up.URL + "/v1",
		Models: []string{"grok-4.7", "own-within", "own-nowindow", "own-nooutput"}}); err != nil {
		t.Fatal(err)
	}
	// the backend's own list: grok-4.7's window, as cli-chat-proxy.grok.com
	// gives it, with no output of its own
	if err := catalog.SaveLive("gx", up.URL+"/v1", []catalog.Model{
		{ID: "grok-4.7", Context: 256000},
		{ID: "own-within", Context: 128000, Output: 64000},
		{ID: "own-nowindow", Output: 500000},
		{ID: "own-nooutput", Context: 256000},
	}); err != nil {
		t.Fatal(err)
	}
	// the fixture is the report's: models.dev's output over the backend's
	// window in the catalog itself
	e, ok := provider.ServedEntryOf("gx/grok-4.7")
	if !ok || e.Context != 256000 || e.Output != 500000 {
		t.Fatalf("fixture: window %d, output %d; want models.dev's 500000 over the backend's 256000", e.Context, e.Output)
	}

	list := func(path, key, ua string) map[string]map[string]any {
		t.Helper()
		req := httptest.NewRequest("GET", path, nil)
		if key != "" {
			req.Header.Set("Authorization", "Bearer "+key)
		}
		if ua != "" {
			req.Header.Set("User-Agent", ua)
		}
		rec := httptest.NewRecorder()
		New().Handler().ServeHTTP(rec, req)
		var out struct {
			Data []map[string]any `json:"data"`
		}
		if err := json.Unmarshal(rec.Body.Bytes(), &out); err != nil {
			t.Fatalf("%s: %s", path, rec.Body)
		}
		got := map[string]map[string]any{}
		for _, d := range out.Data {
			got[d["id"].(string)] = d
		}
		return got
	}
	num := func(v any) any {
		if n, ok := v.(float64); ok {
			return int(n)
		}
		return v
	}

	// /v1/models, as every client reads it
	got := list("/v1/models", "", "")
	for id, want := range map[string][2]any{
		"gx/grok-4.7":     {256000, 256000},
		"gx/own-within":   {128000, 64000},
		"gx/own-nowindow": {nil, 500000},
		"gx/own-nooutput": {256000, nil},
	} {
		m := got[id]
		if m == nil {
			t.Fatalf("%s not listed: %v", id, got)
		}
		if w, o := num(m["context_window"]), num(m["max_output_tokens"]); w != want[0] || o != want[1] {
			t.Errorf("/v1/models %s: context_window %v, max_output_tokens %v; want %v, %v", id, w, o, want[0], want[1])
		}
	}

	// Cursor Private Inference's capabilities
	c, _ := list("/v1/models", TokenFor("cursor-local"), "")["gx/grok-4.7"]["capabilities"].(map[string]any)
	if num(c["context_length"]) != 256000 || num(c["max_output_tokens"]) != 256000 {
		t.Errorf("Cursor's capabilities: %v", c)
	}

	// Muse Code's limit
	muse := list("/muse-code/models", "", "muse-build/1.4.2 (non-interactive; macos-aarch64; build 0)")
	if meta, _ := muse["gx/grok-4.7"]["metadata"].(map[string]any)["muse-code"].(map[string]any); meta == nil {
		t.Errorf("Muse's list: %v", muse)
	} else if l, _ := meta["limit"].(map[string]any); num(l["context"]) != 256000 || num(l["output"]) != 256000 {
		t.Errorf("Muse's limit: %v", meta["limit"])
	}

	// a group's member is asked for no more than the model's own limit, as
	// before (withMaxOutput): what is published is the pair, not a new cap
	// on what is asked
	if err := provider.SaveGroup(provider.Group{ID: "gg", Name: "GG", Members: []string{"gx/grok-4.7"}}); err != nil {
		t.Fatal(err)
	}
	if code, body := post(t, "/v1/chat/completions", `{"model":"`+provider.GroupPrefix+`gg","max_tokens":300000,"messages":[{"role":"user","content":"hi"}]}`); code != 200 {
		t.Fatalf("%d %s", code, body)
	}
	var asked map[string]any
	if err := json.Unmarshal(f.got, &asked); err != nil || num(asked["max_tokens"]) != 300000 {
		t.Errorf("asked upstream: %s", f.got)
	}
}
