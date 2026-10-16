package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/usageapi"
)

// The Usage page's numbers are read over the gateway too (HoGee_xxl on
// X): /v1/magpie/usage and /v1/magpie/usage/requests answer this machine,
// another one only with a gateway key, and a key held to a budget only
// with its own calls. A Claude Code subagent's call says which subagent
// made it and which one started that.
func TestUsageOverGateway(t *testing.T) {
	fresh(t)
	t.Setenv("MAGPIE_ADDR", "")
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":30,"completion_tokens":5}}`)
	}))
	defer up.Close()
	if err := provider.Save(provider.Provider{ID: "plan", Name: "Plan", Key: "upstream-secret", Chat: up.URL + "/v1", Models: []string{"m1"}}); err != nil {
		t.Fatal(err)
	}
	keys, secrets := newCaller(t, "Laptop", "Held")
	if _, err := access.Update("limit-key", access.Change{Key: keys[1].ID, Limit: &access.Limit{Period: "day", Tokens: 1_000_000}}); err != nil {
		t.Fatal(err)
	}
	h := lanGuard(New().Handler())
	do := func(method, path, from string, body io.Reader, hdr ...string) (int, string) {
		r := httptest.NewRequest(method, path, body)
		r.RemoteAddr = from
		for i := 0; i+1 < len(hdr); i += 2 {
			r.Header.Set(hdr[i], hdr[i+1])
		}
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		return w.Code, w.Body.String()
	}
	// the main conversation, then a subagent of it that another started, as
	// Claude Code 2.1.296 sends a nested one (captured headers)
	if c, b := do("POST", "/v1/chat/completions", "127.0.0.1:5000", strings.NewReader(chatReq), "Authorization", "Bearer "+secrets[0], "X-Claude-Code-Session-Id", "conv-1"); c != 200 {
		t.Fatal(c, b)
	}
	if c, b := do("POST", "/v1/chat/completions", "127.0.0.1:5000", strings.NewReader(chatReq), "Authorization", "Bearer "+secrets[0], "X-Claude-Code-Session-Id", "conv-1",
		"X-Claude-Code-Agent-Id", "a143287ac46b82a96", "X-Claude-Code-Parent-Agent-Id", "a99ca20c03d1813d8"); c != 200 {
		t.Fatal(c, b)
	}
	if c, b := do("POST", "/v1/chat/completions", "127.0.0.1:5000", strings.NewReader(chatReq), "Authorization", "Bearer "+secrets[1]); c != 200 {
		t.Fatal(c, b)
	}

	var sum usageapi.Usage
	c, b := do("GET", "/v1/magpie/usage?period=all", "127.0.0.1:5000", nil)
	if c != 200 || json.Unmarshal([]byte(b), &sum) != nil || sum.Calls != 3 || len(sum.CallerKeys) != 2 || sum.Path == "" {
		t.Fatal("loopback summary:", c, b)
	}
	var page usageapi.Ledger
	c, b = do("GET", "/v1/magpie/usage/requests?period=all", "127.0.0.1:5000", nil)
	if c != 200 || json.Unmarshal([]byte(b), &page) != nil || page.Total != 3 {
		t.Fatal("loopback requests:", c, b)
	}
	subs := 0
	for _, r := range page.Rows {
		if r.Subagent != "" {
			subs++
			if r.Subagent != "a143287ac46b82a96" || r.ParentAgent != "a99ca20c03d1813d8" || r.Session != "conv-1" {
				t.Fatalf("the subagent's row: %+v", r.Row)
			}
		} else if r.ParentAgent != "" {
			t.Fatalf("a parent with no subagent: %+v", r.Row)
		}
	}
	if subs != 1 {
		t.Fatal("subagent rows:", subs, b)
	}

	if c, _ := do("GET", "/v1/magpie/usage", "192.168.1.9:5000", nil); c != http.StatusUnauthorized {
		t.Fatal("another machine with no key got", c)
	}
	sum = usageapi.Usage{}
	c, b = do("GET", "/v1/magpie/usage?period=all", "192.168.1.9:5000", nil, "Authorization", "Bearer "+secrets[0])
	if c != 200 || json.Unmarshal([]byte(b), &sum) != nil || sum.Calls != 3 || sum.Path != "" {
		t.Fatal("another machine with a key:", c, b)
	}

	// the held key: no summary of everyone, and its own calls alone
	for _, from := range []string{"127.0.0.1:5000", "192.168.1.9:5000"} {
		if c, b := do("GET", "/v1/magpie/usage?period=all", from, nil, "x-api-key", secrets[1]); c != http.StatusForbidden || !strings.Contains(b, "/v1/magpie/usage/requests") {
			t.Fatal("held key's summary from", from, c, b)
		}
		page = usageapi.Ledger{}
		c, b := do("GET", "/v1/magpie/usage/requests?period=all&callerKey="+keys[0].ID, from, nil, "x-api-key", secrets[1])
		if c != 200 || json.Unmarshal([]byte(b), &page) != nil || page.Total != 1 || len(page.Rows) != 1 || page.Rows[0].CallerKeyID != keys[1].ID {
			t.Fatal("held key's requests from", from, c, b)
		}
		if len(page.CallerKeys) != 1 || strings.Contains(b, "Laptop") || strings.Contains(b, keys[0].ID) || strings.Contains(b, "conv-1") {
			t.Fatal("held key told of another key's calls:", b)
		}
	}
}
