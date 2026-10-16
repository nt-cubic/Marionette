package gui

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/sessions"
	"github.com/yetone/magpie/internal/usage"
)

// #2285: a Pi subagent's session file sits in a temp folder magpie never
// reads, so the sessions that spent the most (Usage › Sessions) knew its
// DeepSeek turns only from the gateway's records: no folder, no prompt,
// shown like a native session whose prompt was empty. The overview marks
// such a session as the gateway's, and a native one not, so the card can
// say which it is.
func TestTopSessionsMarkWhatOnlyTheGatewaySaw(t *testing.T) {
	h := sandboxHome(t)
	claude := filepath.Join(h, ".claude")
	t.Setenv("CLAUDE_CONFIG_DIR", claude)
	proj := filepath.Join(claude, "projects", "-work")
	if err := os.MkdirAll(proj, 0o755); err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	at := now.Add(-time.Hour).Format(time.RFC3339)
	line := fmt.Sprintf(`{"type":"user","message":{"role":"user","content":"hello"},"uuid":"u","timestamp":%q,"cwd":"/work","sessionId":"native"}`+"\n"+
		`{"type":"assistant","message":{"model":"claude-sonnet-5","id":"msg_1","role":"assistant","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":10,"output_tokens":2}},"uuid":"a","timestamp":%q,"cwd":"/work","sessionId":"native"}`+"\n", at, at)
	if err := os.WriteFile(filepath.Join(proj, "native.jsonl"), []byte(line), 0o600); err != nil {
		t.Fatal(err)
	}
	sessions.Reset()
	t.Cleanup(sessions.Reset)
	t.Cleanup(func() { statsMemo.Lock(); statsMemo.m = nil; statsMemo.Unlock() })
	statsMemo.Lock()
	statsMemo.m = nil
	statsMemo.Unlock()
	// what the gateway recorded of Pi asking for magpie/deepseek/deepseek-v4-pro
	usage.Append(usage.Record{Time: now.Add(-time.Minute), Agent: "pi", Session: "0199f1c2-subagent", Requested: "deepseek/deepseek-v4-pro", Provider: "deepseek", Model: "deepseek-v4-pro", Input: 2000, Output: 300, CacheRead: 60000, Status: 200})

	mux := http.NewServeMux()
	sessionRoutes(mux, folderOnly{})
	w := httptest.NewRecorder()
	mux.ServeHTTP(w, httptest.NewRequest("GET", "/api/sessions/overview?days=7", nil))
	if w.Code != 200 {
		t.Fatalf("overview: %d %s", w.Code, w.Body)
	}
	var ov struct {
		Top map[string][]map[string]any `json:"top"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &ov); err != nil {
		t.Fatal(err)
	}
	seen := map[string]any{}
	for _, s := range ov.Top["tokens"] {
		seen[fmt.Sprint(s["agent"], ":", s["id"])] = s["gateway"]
	}
	if len(seen) != 2 {
		t.Fatalf("top sessions: %s", w.Body)
	}
	if g := seen["pi:0199f1c2-subagent"]; g != true {
		t.Errorf("the gateway-only Pi session isn't marked the gateway's (gateway=%v): %s", g, w.Body)
	}
	if g, ok := seen["claude:native"]; !ok || g != nil {
		t.Errorf("the native session is marked the gateway's (gateway=%v): %s", g, w.Body)
	}
}
