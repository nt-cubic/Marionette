package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// The Cline plugin's provider has ClinePass's Upstream tick too (ARNO on
// Discord): saved on it, a DeepSeek model's request reaches the plugin
// pinned to DeepSeek's own API, on Chat and Anthropic Messages alike, and
// its other models' requests go as sent.
func TestClinePluginPinsDeepSeek(t *testing.T) {
	t.Setenv("FAKE_DEEPSEEK", "1")
	var mu sync.Mutex
	var sent []map[string]any
	pid := besideFake(t, "cline", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var q map[string]any
		b, _ := io.ReadAll(r.Body)
		json.Unmarshal(b, &q)
		mu.Lock()
		sent = append(sent, q)
		mu.Unlock()
		if q["stream"] == true {
			w.Header().Set("Content-Type", "text/event-stream")
			io.WriteString(w, sse(`data: {"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"hi"},"finish_reason":"stop"}]}`, `data: [DONE]`))
			return
		}
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"c1","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1}}`)
	}))
	p, err := provider.Find(pid)
	if err != nil {
		t.Fatal(err)
	}
	if !p.ClinePinnable() || p.ClinePin("deepseek-v4-pro") != "" {
		t.Fatalf("the Cline plugin's provider: pinnable %v, pinned unasked %q", p.ClinePinnable(), p.ClinePin("deepseek-v4-pro"))
	}
	ask := func() []map[string]any {
		mu.Lock()
		sent = nil
		mu.Unlock()
		for _, req := range []struct{ path, body string }{
			{"/v1/chat/completions", `{"model":"` + pid + `/deepseek-v4-pro","messages":[{"role":"user","content":"hi"}]}`},
			{"/v1/messages", `{"model":"` + pid + `/deepseek-v4-pro","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}`},
			{"/v1/chat/completions", `{"model":"` + pid + `/fake-1","messages":[{"role":"user","content":"hi"}]}`},
		} {
			postProto(t, New(), req.path, req.body)
		}
		mu.Lock()
		defer mu.Unlock()
		if len(sent) != 3 {
			t.Fatalf("the plugin sent %d requests", len(sent))
		}
		return append([]map[string]any(nil), sent...)
	}
	for i, q := range ask() {
		if q["providerOptions"] != nil {
			t.Fatalf("request %d pinned with the tick off: %v", i, q["providerOptions"])
		}
	}

	// the tick, saved as the editor saves it, is kept on the account
	p.PinUpstream = true
	if err := provider.Save(*p); err != nil {
		t.Fatal(err)
	}
	if p, err = provider.Find(pid); err != nil || !p.PinUpstream {
		t.Fatalf("the tick wasn't kept: %+v, %v", p, err)
	}
	got := ask()
	for i, q := range got[:2] {
		po, _ := json.Marshal(q["providerOptions"])
		if string(po) != `{"gateway":{"only":["deepseek"]}}` {
			t.Fatalf("request %d (DeepSeek) reached the plugin with providerOptions %s", i, po)
		}
	}
	if got[2]["providerOptions"] != nil {
		t.Fatalf("fake-1 pinned: %v", got[2]["providerOptions"])
	}
}
