package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
)

// A plugin model whose config says temperature: false — as factory 0.1.31's
// config models say it of its OpenAI route (plugins#154, sweanng424: the
// vendor turned temperature and top_p away with a 400) — is asked without
// them, however the client speaks: relayed as it came, or translated. One
// that says nothing of it keeps what the client sent (#2492).
func TestPluginModelRefusingTemperature(t *testing.T) {
	t.Setenv("FAKE_TEMP", "1")
	var mu sync.Mutex
	var seen []map[string]any
	pid := besideFake(t, "opencode-zen-free", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		var q map[string]any
		json.Unmarshal(b, &q)
		mu.Lock()
		seen = append(seen, q)
		mu.Unlock()
		w.Header().Set("Content-Type", "text/event-stream")
		b1, _ := json.Marshal(map[string]any{"id": "c1", "object": "chat.completion.chunk", "choices": []any{map[string]any{"index": 0, "delta": map[string]any{"content": "ok"}, "finish_reason": nil}}})
		io.WriteString(w, "data: "+string(b1)+"\n\n")
		io.WriteString(w, `data: {"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}`+"\n\ndata: [DONE]\n\n")
	}))
	h := New().Handler()
	ask := func(path, body string) (int, string) {
		req := httptest.NewRequest("POST", path, strings.NewReader(body))
		req.Header.Set("Authorization", "Bearer magpie")
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, req)
		return rec.Code, rec.Body.String()
	}
	last := func() map[string]any {
		t.Helper()
		mu.Lock()
		defer mu.Unlock()
		if len(seen) == 0 {
			t.Fatal("the plugin's fetch was never asked")
		}
		return seen[len(seen)-1]
	}
	has := func(q map[string]any, field string) bool {
		_, ok := q[field]
		return ok
	}
	// relayed as it came: a streamed chat request goes through as the
	// client sent it, temperature and all, the model's name swapped
	code, body := ask("/v1/chat/completions", `{"model":"`+pid+`/fake-cold","stream":true,"temperature":0.7,"top_p":0.9,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 {
		t.Fatalf("fake-cold relayed: %d %s", code, body)
	}
	if q := last(); has(q, "temperature") || has(q, "top_p") {
		t.Errorf("the plugin's fetch got a model that refuses them asked with temperature %v, top_p %v", q["temperature"], q["top_p"])
	}
	// translated from Claude's API, which has no model list of its own to
	// read the refusal from: the built chat request carries neither
	code, body = ask("/v1/messages", `{"model":"`+pid+`/fake-cold","max_tokens":100,"temperature":0.7,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 {
		t.Fatalf("fake-cold translated: %d %s", code, body)
	}
	if q := last(); has(q, "temperature") {
		t.Errorf("the translated request asked temperature %v of a model that refuses it", q["temperature"])
	}
	// one that says nothing keeps what the client sent
	code, body = ask("/v1/chat/completions", `{"model":"`+pid+`/fake-1","stream":true,"temperature":0.7,"top_p":0.9,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 {
		t.Fatalf("fake-1 relayed: %d %s", code, body)
	}
	if q := last(); !has(q, "temperature") || !has(q, "top_p") {
		t.Errorf("a model that says nothing of temperature was asked without it (temperature %v, top_p %v)", q["temperature"], q["top_p"])
	}
}
