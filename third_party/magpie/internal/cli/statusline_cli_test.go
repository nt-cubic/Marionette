package cli

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// What Claude Code hands a statusLine command on stdin (code.claude.com
// docs, "Full JSON schema"), cut to the fields around the one read.
const claudeStatusJSON = `{
  "cwd": "/Users/me/work",
  "session_id": "6f1d2c3a-9b8e-4a7f-8c1d-2e3f4a5b6c7d",
  "session_name": "my-session",
  "transcript_path": "/Users/me/.claude/projects/work/6f1d2c3a.jsonl",
  "model": {"id": "claude-opus-5-5", "display_name": "Opus"},
  "workspace": {"current_dir": "/Users/me/work", "project_dir": "/Users/me/work", "added_dirs": []},
  "version": "2.1.296",
  "output_style": {"name": "default"},
  "cost": {"total_cost_usd": 0.01234, "total_duration_ms": 45000, "total_api_duration_ms": 2300, "total_lines_added": 156, "total_lines_removed": 23},
  "context_window": {"total_input_tokens": 15500, "total_output_tokens": 1200, "context_window_size": 200000, "used_percentage": 8, "remaining_percentage": 92},
  "exceeds_200k_tokens": false
}`

// statuslineGateway is a gateway answering kept quotas only (a vendor read,
// without kept=1, fails the test) and the session's route.
func statuslineGateway(t *testing.T, now time.Time) *httptest.Server {
	t.Helper()
	at := func(d time.Duration) *time.Time { x := now.Add(d); return &x }
	quotas := []provider.Quota{
		{Provider: "claude", Name: "Claude Code", Kind: "subscription", User: "a@example.com", LastServedAt: at(-time.Minute), Last: true,
			Windows: []provider.QuotaSpan{{Name: "5 hours", Used: 62, Remaining: 38, ResetsAt: at(2 * time.Hour)}, {Name: "7 days", Used: 41, Remaining: 59, ResetsAt: at(72 * time.Hour)}}},
		{Provider: "claude", Name: "Claude Code", Kind: "subscription", User: "b@example.com", Alias: "B", LastServedAt: at(-time.Hour),
			Windows: []provider.QuotaSpan{{Name: "5 hours", Used: 8, Remaining: 92, ResetsAt: at(-time.Minute)}, {Name: "7 days", Used: 30, Remaining: 70}}},
		{Provider: "codex", Name: "Codex", Kind: "subscription", User: "c@example.com",
			Windows: []provider.QuotaSpan{{Name: "5 hours", Used: 10, Remaining: 90}}},
		{Provider: "deepseek", Name: "DeepSeek", Kind: "balance", Balance: "¥12.00", LastServedAt: at(-time.Minute), Windows: []provider.QuotaSpan{}},
	}
	mux := http.NewServeMux()
	mux.HandleFunc("GET /v1/magpie/quotas", func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Query().Get("kept") != "1" {
			t.Errorf("statusline asked %s: that reads the vendors, not what magpie kept", r.URL)
			http.Error(w, "vendor read", 500)
			return
		}
		json.NewEncoder(w).Encode(map[string]any{"object": "list", "data": quotas})
	})
	mux.HandleFunc("GET /v1/magpie/route", func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Query().Get("session") != "6f1d2c3a-9b8e-4a7f-8c1d-2e3f4a5b6c7d" {
			json.NewEncoder(w).Encode(map[string]any{"route": nil})
			return
		}
		json.NewEncoder(w).Encode(map[string]any{"session": r.URL.Query().Get("session"), "seq": 3,
			"route": map[string]any{"asked": "smart", "group": "smart", "model": "claude/claude-opus-5-5", "tries": []any{}, "done": true, "status": 200}})
	})
	s := httptest.NewServer(mux)
	t.Cleanup(s.Close)
	return s
}

func TestStatuslineTellsRouteAndEveryAccount(t *testing.T) {
	now := time.Now()
	s := statuslineGateway(t, now)
	got := statusline(strings.NewReader(claudeStatusJSON), nil, s.URL, now)
	// the session's member, then the two accounts that served lately (the
	// codex one that didn't and the balance left out); B's 5 hours started
	// again since it was read
	want := "→ claude/claude-opus-5-5 | claude a@example.com* 5h 62% 7d 41% | claude B 5h 0% 7d 30%"
	if got != want {
		t.Fatalf("statusline =\n  %q\nwant\n  %q", got, want)
	}
	if got := statusline(strings.NewReader(claudeStatusJSON), []string{"codex"}, s.URL, now); got != "→ claude/claude-opus-5-5 | codex c@example.com 5h 10%" {
		t.Fatalf("statusline codex = %q", got)
	}
	// no session on stdin: the accounts alone
	if got := statusline(strings.NewReader(""), []string{"b@example.com"}, s.URL, now); got != "claude B 5h 0% 7d 30%" {
		t.Fatalf("statusline without a session = %q", got)
	}
}

// A gateway that isn't running is an empty line, at once.
func TestStatuslineGatewayOff(t *testing.T) {
	s := httptest.NewServer(http.NotFoundHandler())
	base := s.URL
	s.Close()
	start := time.Now()
	if got := statusline(strings.NewReader(claudeStatusJSON), nil, base, time.Now()); got != "" {
		t.Fatalf("statusline with the gateway off = %q, want nothing", got)
	}
	if d := time.Since(start); d > 30*time.Second {
		t.Fatalf("statusline took %s with the gateway off", d)
	}
}
