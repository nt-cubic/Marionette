package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/sessions"
)

func codexDetailsAPILine(t *testing.T, at time.Time, typ string, payload any) string {
	t.Helper()
	b, err := json.Marshal(struct {
		Timestamp string `json:"timestamp"`
		Type      string `json:"type"`
		Payload   any    `json:"payload"`
	}{at.Format(time.RFC3339Nano), typ, payload})
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

func writeCodexDetailsAPIRollout(t *testing.T, codex, id string, at time.Time, lines ...string) {
	t.Helper()
	path := filepath.Join(codex, "sessions", at.Format("2006/01/02"), "rollout-"+at.Format("2006-01-02T15-04-05")+"-"+id+".jsonl")
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(strings.Join(lines, "\n")+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
}

func TestCodexSubagentDetailsReachSessionAPIs(t *testing.T) {
	home := sandboxHome(t)
	codex := filepath.Join(home, ".codex")
	t.Setenv("CODEX_HOME", codex)
	// sessions.StatsFor counts the last days up to the real today, so the
	// rollouts are written now, not on a fixed date that is in the future
	// in one time zone and past the 7-day window a week later.
	now := holdUsageClock(t, time.Now().Truncate(time.Second))
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	sessions.Reset()
	forgetStats()
	t.Cleanup(func() { sessions.Reset(); forgetStats() })

	mainMeta := codexDetailsAPILine(t, now.Add(-time.Minute), "session_meta", map[string]any{
		"id": "main-a", "session_id": "shared", "source": "cli",
	})
	spawn := codexDetailsAPILine(t, now, "response_item", struct {
		Type      string `json:"type"`
		Name      string `json:"name"`
		CallID    string `json:"call_id"`
		Arguments string `json:"arguments"`
	}{"function_call", "spawn_agent", "call-child", `{"task_name":"Inspect the fixture"}`})
	activity := codexDetailsAPILine(t, now, "event_msg", struct {
		Type string `json:"type"`
		Item struct {
			Type          string `json:"type"`
			Kind          string `json:"kind"`
			ID            string `json:"id"`
			AgentThreadID string `json:"agent_thread_id"`
		} `json:"item"`
	}{Type: "item_completed", Item: struct {
		Type          string `json:"type"`
		Kind          string `json:"kind"`
		ID            string `json:"id"`
		AgentThreadID string `json:"agent_thread_id"`
	}{Type: "SubAgentActivity", Kind: "started", ID: "call-child", AgentThreadID: "child-a"}})
	writeCodexDetailsAPIRollout(t, codex, "main-a", now, mainMeta, spawn, activity)
	childMeta := codexDetailsAPILine(t, now.Add(-time.Minute), "session_meta", map[string]any{
		"id": "child-a", "session_id": "shared", "source": map[string]any{"subagent": map[string]any{"thread_spawn": map[string]any{"parent_thread_id": "main-a"}}},
		"agent_nickname": "Scout",
	})
	turn := codexDetailsAPILine(t, now.Add(-time.Second), "turn_context", map[string]any{"turn_id": "turn-child", "model": "fixture-model"})
	usage := codexDetailsAPILine(t, now, "token_usage_record", map[string]any{
		"response_id": "child-response", "session_id": "shared", "thread_id": "child-a", "turn_id": "turn-child",
		"usage": map[string]int{"input_tokens": 12, "output_tokens": 3, "total_tokens": 15},
	})
	writeCodexDetailsAPIRollout(t, codex, "child-a", now, childMeta, turn, usage)

	mux := http.NewServeMux()
	sessionRoutes(mux, folderOnly{})
	sessionManageRoutes(mux, folderOnly{})
	request := func(path string) *httptest.ResponseRecorder {
		t.Helper()
		w := httptest.NewRecorder()
		mux.ServeHTTP(w, httptest.NewRequest("GET", path, nil))
		if w.Code != http.StatusOK {
			t.Fatalf("%s: %d %s", path, w.Code, w.Body)
		}
		return w
	}

	var listed struct {
		Sessions []sessions.Session `json:"sessions"`
	}
	if err := json.Unmarshal(request("/api/sessions").Body.Bytes(), &listed); err != nil {
		t.Fatal(err)
	}
	assertDetails := func(s sessions.Session, where string) {
		t.Helper()
		if s.Title != "Inspect the fixture" || s.Nickname != "Scout" {
			t.Errorf("%s: title/nickname = %q/%q", where, s.Title, s.Nickname)
		}
	}
	var child sessions.Session
	for _, s := range listed.Sessions {
		if s.Agent == "codex" && s.ID == "child-a" {
			child = s
			break
		}
	}
	assertDetails(child, "list")

	var one sessions.Session
	if err := json.Unmarshal(request("/api/sessions/one?key=codex:child-a").Body.Bytes(), &one); err != nil {
		t.Fatal(err)
	}
	assertDetails(one, "one")

	var managed struct {
		Sessions []sessions.Managed `json:"sessions"`
	}
	if err := json.Unmarshal(request("/api/sessions/manage?agent=codex").Body.Bytes(), &managed); err != nil {
		t.Fatal(err)
	}
	for _, row := range managed.Sessions {
		if row.Agent == "codex" && row.ID == "child-a" {
			assertDetails(row.Session, "manage")
			child = row.Session
		}
	}

	var overview sessions.Overview
	if err := json.Unmarshal(request("/api/sessions/overview?days=7&agent=codex").Body.Bytes(), &overview); err != nil {
		t.Fatal(err)
	}
	for _, summary := range overview.Top["tokens"] {
		if summary.Agent == "codex" && summary.ID == "child-a" {
			if summary.Title != "Inspect the fixture" || summary.Nickname != "Scout" {
				t.Errorf("overview title/nickname = %q/%q", summary.Title, summary.Nickname)
			}
			return
		}
	}
	t.Fatal("overview token top omitted the active Codex child")
}
