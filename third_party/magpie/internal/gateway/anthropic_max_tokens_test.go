package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/tidwall/gjson"
	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// A request that names no reply limit (Codex's Responses turns) is sent on
// Anthropic's API with the model's own: the user's Max output, else the
// vendor's list or models.dev, within the window, as the agents are told it.
// It was sent a flat 16384, which a long thinking and tool turn ran out of
// (#1792, tensornull: "Incomplete response returned, reason:
// max_output_tokens"). 16384 stays for a model whose limit isn't known, and
// a limit the client names goes as it is.
func TestAnthropicMaxTokensIsTheModels(t *testing.T) {
	fresh(t)
	var mu sync.Mutex
	var asked map[string]any
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var q map[string]any
		b, _ := io.ReadAll(r.Body)
		json.Unmarshal(b, &q)
		mu.Lock()
		asked = q
		mu.Unlock()
		if q["stream"] == true {
			w.Header().Set("Content-Type", "text/event-stream")
			io.WriteString(w, "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n"+
				"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n"+
				"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n"+
				"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n"+
				"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n"+
				"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")
			return
		}
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"m","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}`)
	}))
	t.Cleanup(up.Close)
	// models.dev's entry for a Claude model, and one listing a reply
	// limit past its window
	const known, wide = "claude-sonnet-4-5", "wide-1"
	os.MkdirAll(filepath.Dir(catalog.CachePath()), 0o755)
	if err := os.WriteFile(catalog.CachePath(), []byte(`{"anthropic":{"id":"anthropic","models":{
		"claude-sonnet-4-5":{"id":"claude-sonnet-4-5","limit":{"context":200000,"output":64000}},
		"wide-1":{"id":"wide-1","limit":{"context":100000,"output":500000}}}}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	if err := provider.Save(provider.Provider{ID: "a", Name: "A", Key: "k", Models: []string{"set", known, wide, "mystery-9"}, Anthropic: up.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetModelOutput("a/set", 65536); err != nil {
		t.Fatal(err)
	}
	s := New()
	maxTokens := func(path, body string) (int, map[string]any) {
		t.Helper()
		mu.Lock()
		asked = nil
		mu.Unlock()
		if code, raw := postJSON(t, s, path, body); code != 200 {
			t.Fatalf("%s %s: %d %s", path, body, code, raw)
		}
		mu.Lock()
		defer mu.Unlock()
		n, _ := asked["max_tokens"].(float64)
		return int(n), asked
	}
	for _, c := range []struct {
		name, path, body string
		want             int
	}{
		{"the user's Max output, Codex's turn", "/v1/responses", `{"model":"a/set","input":"hi"}`, 65536},
		{"the user's Max output, streamed", "/v1/responses", `{"model":"a/set","input":"hi","stream":true}`, 65536},
		{"the user's Max output, a Chat client", "/v1/chat/completions", `{"model":"a/set","messages":[{"role":"user","content":"hi"}]}`, 65536},
		{"models.dev's", "/v1/responses", `{"model":"a/` + known + `","input":"hi"}`, 64000},
		{"models.dev's, within the window", "/v1/responses", `{"model":"a/` + wide + `","input":"hi"}`, 100000},
		{"none known", "/v1/responses", `{"model":"a/mystery-9","input":"hi"}`, defaultMaxTokens},
		{"the client's own", "/v1/responses", `{"model":"a/set","input":"hi","max_output_tokens":1000}`, 1000},
	} {
		if got, _ := maxTokens(c.path, c.body); got != c.want {
			t.Errorf("%s: max_tokens %d, want %d", c.name, got, c.want)
		}
	}

	// thinking fits under the model's limit, as under a limit the client
	// named: a budget at or past max_tokens is refused with a 400
	if err := provider.SetModelOutput("a/set", 8192); err != nil {
		t.Fatal(err)
	}
	got, q := maxTokens("/v1/responses", `{"model":"a/set","input":"hi","reasoning":{"effort":"high"}}`)
	th, _ := q["thinking"].(map[string]any)
	budget, _ := th["budget_tokens"].(float64)
	if got != 8192 || th["type"] != "enabled" || budget <= 0 || int(budget) >= got {
		t.Fatalf("max_tokens %d, thinking %v", got, q["thinking"])
	}
}

// A Zed subscription's Claude is asked in Anthropic's request inside Zed's
// envelope, so a turn naming no limit takes the model's own there too.
func TestZedClaudeMaxTokensIsTheModels(t *testing.T) {
	fresh(t)
	var got []int64
	zedUpstream(t, "anthropic", func(w http.ResponseWriter, b []byte, _ string) {
		got = append(got, gjson.GetBytes(b, "provider_request.max_tokens").Int())
		zedLines(w,
			zedEvent(`{"type":"message_start","message":{"id":"m","role":"assistant","usage":{"input_tokens":1,"output_tokens":0}}}`),
			zedEvent(`{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}`),
			zedEvent(`{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}`),
			zedEvent(`{"type":"content_block_stop","index":0}`),
			zedEvent(`{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}`),
			zedEvent(`{"type":"message_stop"}`),
			zedEnded)
	})
	// a provider of the user's stands in for the subscription in the
	// catalog ("zed" itself is the built-in's id), saved after zedUpstream
	// moves the config folder
	if err := provider.Save(provider.Provider{ID: "zz", Name: "Zz", Key: "k", Models: []string{"claude-sonnet-4-5"}, Anthropic: "http://zed.invalid/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetModelOutput("zz/claude-sonnet-4-5", 40000); err != nil {
		t.Fatal(err)
	}
	s := New()
	for _, body := range []string{
		`{"model":"zz/claude-sonnet-4-5","input":"hi"}`,
		`{"model":"zz/claude-sonnet-4-5","input":"hi","max_output_tokens":1000}`,
	} {
		w := httptest.NewRecorder()
		var u Usage
		p := provider.Provider{ID: "zz", Account: &provider.Account{Agent: "zed", User: "octo"}}
		if status, msg := s.serveZed(w, httptest.NewRequest("POST", "/", strings.NewReader(body)), provider.Responses, p, "claude-sonnet-4-5", []byte(body), &u); status != 200 {
			t.Fatalf("%s: %d %q\n%s", body, status, msg, w.Body.String())
		}
	}
	if len(got) != 2 || got[0] != 40000 || got[1] != 1000 {
		t.Fatalf("max_tokens %v, want [40000 1000]", got)
	}
}
