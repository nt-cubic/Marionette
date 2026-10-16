package gui

import (
	"encoding/json"
	"fmt"
	"math"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/sessions"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/usage"
)

type codexTestCall struct {
	response, model, effort  string
	at                       time.Time
	input, output, reasoning int
}

func writeCodexAPITestThread(t *testing.T, codex, id, session, parent string, calls []codexTestCall) {
	t.Helper()
	start := calls[0].at
	path := filepath.Join(codex, "sessions", start.Format("2006/01/02"), "rollout-"+start.Format("2006-01-02T15-04-05")+"-"+id+".jsonl")
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	source, err := json.Marshal("cli")
	if parent != "" {
		source, err = json.Marshal(map[string]any{"subagent": map[string]any{"thread_spawn": map[string]string{"parent_thread_id": parent}}})
	}
	if err != nil {
		t.Fatal(err)
	}
	lines := []string{fmt.Sprintf(`{"timestamp":%q,"type":"session_meta","payload":{"id":%q,"session_id":%q,"source":%s}}`, calls[0].at.Add(-time.Hour).Format(time.RFC3339Nano), id, session, source)}
	for i, call := range calls {
		turn := fmt.Sprintf("turn-%s-%d", id, i)
		lines = append(lines,
			fmt.Sprintf(`{"timestamp":%q,"type":"turn_context","payload":{"turn_id":%q,"model":%q,"effort":%q}}`, call.at.Add(-time.Second).Format(time.RFC3339Nano), turn, call.model, call.effort),
			fmt.Sprintf(`{"timestamp":%q,"type":"token_usage_record","payload":{"response_id":%q,"session_id":%q,"thread_id":%q,"turn_id":%q,"usage":{"input_tokens":%d,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":%d,"reasoning_output_tokens":%d,"total_tokens":%d}}}`, call.at.Format(time.RFC3339Nano), call.response, session, id, turn, call.input, call.output, call.reasoning, call.input+call.output),
		)
	}
	if err := os.WriteFile(path, []byte(strings.Join(lines, "\n")+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
}

func codexSessionByID(t *testing.T, list []sessions.Session, agent, id string) sessions.Session {
	t.Helper()
	for _, s := range list {
		if s.Agent == agent && s.ID == id {
			return s
		}
	}
	t.Fatalf("session %s:%s missing from %+v", agent, id, list)
	return sessions.Session{}
}

func codexEffortByModel(rows []sessions.ModelEffort, model, effort string) (sessions.ModelEffort, bool) {
	for _, row := range rows {
		if row.Model == model && row.Effort == effort {
			return row, true
		}
	}
	return sessions.ModelEffort{}, false
}

func TestCodexSessionAPIAddsOwnModelEffort(t *testing.T) {
	home := sandboxHome(t)
	codex := filepath.Join(home, ".codex")
	t.Setenv("CODEX_HOME", codex)
	now := holdUsageClock(t, time.Now())
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	if err := os.MkdirAll(filepath.Dir(catalog.CachePath()), 0o700); err != nil {
		t.Fatal(err)
	}
	prices := `{"openai":{"id":"openai","models":{"wire-model":{"id":"wire-model","cost":{"input":2,"output":8,"cache_read":0.5,"cache_write":2.5}},"native-model":{"id":"native-model","cost":{"input":2,"output":8,"cache_read":0.5,"cache_write":2.5}},"child-model":{"id":"child-model","cost":{"input":2,"output":8,"cache_read":0.5,"cache_write":2.5}}}}}`
	if err := os.WriteFile(catalog.CachePath(), []byte(prices), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := settings.Save(settings.Settings{ModelPrices: map[string]settings.ModelPrice{
		"*/child-model": settings.StatedPrice(catalog.Price{Input: 2, Output: 8, CacheRead: 0.5, CacheWrite: 2.5}),
	}}); err != nil {
		t.Fatal(err)
	}
	sessions.Reset()
	forgetStats()
	t.Cleanup(func() { sessions.Reset(); forgetStats() })

	call := func(response, model, effort string, dayOffset, input, output, reasoning int) codexTestCall {
		return codexTestCall{response: response, model: model, effort: effort, at: now.AddDate(0, 0, -dayOffset).Add(-time.Duration(input) * time.Second), input: input, output: output, reasoning: reasoning}
	}
	writeCodexAPITestThread(t, codex, "main-a", "shared-session", "", []codexTestCall{
		call("main-low", "main-model", "low", 0, 10, 2, 1),
		call("main-high", "main-model", "high", 0, 20, 5, 3),
		call("main-other", "alternate-model", "low", 0, 5, 1, 0),
	})
	oldChildCall := call("old-child-local", "child-model", "low", 9, 700, 70, 20)
	writeCodexAPITestThread(t, codex, "child-b", "shared-session", "main-a", []codexTestCall{
		oldChildCall,
		call("paired-response", "native-model", "low", 0, 80, 8, 4),
		call("child-local", "child-model", "low", 0, 40, 4, 2),
	})
	writeCodexAPITestThread(t, codex, "nested-c", "shared-session", "child-b", []codexTestCall{
		call("nested-local", "nested-model", "medium", 0, 50, 5, 3),
	})
	writeCodexAPITestThread(t, codex, "child-d", "shared-session", "main-a", []codexTestCall{
		call("other-child-local", "other-child-model", "high", 0, 60, 6, 4),
	})
	oldCall := call("old-local", "old-model", "low", 9, 70, 7, 5)
	writeCodexAPITestThread(t, codex, "old-thread", "shared-session", "", []codexTestCall{oldCall})

	usage.Append(usage.Record{Time: now.Add(-time.Minute), Agent: "codex", Provider: "openai", Model: "wire-model", Effort: "high", Input: 500, Output: 50, CacheRead: 100, CacheWrite: 20, Reasoning: 30, Status: 200, Session: "main-a", NativeSession: "shared-child", ResponseID: "paired-response", Kind: "collab_spawn"})
	usage.Append(usage.Record{Time: now.Add(-2 * time.Minute), Agent: "codex", Provider: "openai", Model: "remote-model", Effort: "low", Input: 25, Output: 5, Status: 200, Session: "gateway-only"})
	usage.Append(usage.Record{Time: now.Add(-3 * time.Minute), Agent: "claude", Provider: "openai", Model: "claude-model", Input: 15, Output: 3, Status: 200, Session: "claude-only"})

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

	listResponse := request("/api/sessions")
	var listed struct {
		Sessions []sessions.Session `json:"sessions"`
	}
	if err := json.Unmarshal(listResponse.Body.Bytes(), &listed); err != nil {
		t.Fatal(err)
	}
	main := codexSessionByID(t, listed.Sessions, "codex", "main-a")
	if main.Role != "main" || main.RootSession != "main-a" || main.ParentSession != "" {
		t.Fatalf("main identity: %+v", main)
	}
	mainInput, mainOutput := 0, 0
	for _, row := range main.ModelEffort {
		mainInput += row.Input
		mainOutput += row.Output
	}
	if len(main.ModelEffort) != 3 || mainInput != 35 || mainOutput != 8 {
		t.Errorf("main's own model-effort groups/totals = %+v (%d in, %d out), want 3 groups, 35 in, 8 out", main.ModelEffort, mainInput, mainOutput)
	}
	if low, ok := codexEffortByModel(main.ModelEffort, "main-model", "low"); !ok || low.Input != 10 || low.Output != 2 || low.Reasoning != 1 {
		t.Errorf("main low effort = %+v, found=%t", low, ok)
	}
	if high, ok := codexEffortByModel(main.ModelEffort, "main-model", "high"); !ok || high.Input != 20 || high.Output != 5 || high.Reasoning != 3 {
		t.Errorf("main high effort = %+v, found=%t", high, ok)
	}
	if other, ok := codexEffortByModel(main.ModelEffort, "alternate-model", "low"); !ok || other.Input != 5 || other.Output != 1 {
		t.Errorf("main alternate model = %+v, found=%t", other, ok)
	}

	child := codexSessionByID(t, listed.Sessions, "codex", "child-b")
	if child.Role != "subagent" || child.ParentSession != "main-a" || child.RootSession != "main-a" {
		t.Fatalf("child identity: %+v", child)
	}
	if len(child.ModelEffort) != 2 {
		t.Errorf("child's own model-effort groups = %+v, want wire and local groups", child.ModelEffort)
	}
	if row, ok := codexEffortByModel(child.ModelEffort, "wire-model", "high"); !ok || row.Input != 500 || row.Output != 50 || row.CacheRead != 100 || row.CacheWrite != 20 || row.Reasoning != 30 || !row.Priced || math.Abs(row.Cost-0.0015) > 1e-12 {
		t.Errorf("paired gateway winner = %+v, found=%t", row, ok)
	}
	if row, ok := codexEffortByModel(child.ModelEffort, "child-model", "low"); !ok || row.Input != 740 || row.Output != 74 || row.Reasoning != 22 {
		t.Errorf("child local-only use = %+v, found=%t", row, ok)
	}
	if _, ok := codexEffortByModel(child.ModelEffort, "native-model", "low"); ok {
		t.Errorf("matched local response was counted alongside the wire winner: %+v", child.ModelEffort)
	}
	if got := codexSessionByID(t, listed.Sessions, "codex", "nested-c"); got.Role != "subagent" || got.ParentSession != "child-b" || got.RootSession != "main-a" || len(got.ModelEffort) != 1 || got.ModelEffort[0].Model != "nested-model" {
		t.Errorf("nested child did not keep its own identity and use: %+v", got)
	}
	if got := codexSessionByID(t, listed.Sessions, "codex", "child-d"); got.ParentSession != "main-a" || got.RootSession != "main-a" || len(got.ModelEffort) != 1 || got.ModelEffort[0].Model != "other-child-model" {
		t.Errorf("independent child did not keep its own use: %+v", got)
	}
	remote := codexSessionByID(t, listed.Sessions, "codex", "gateway-only")
	if remote.Role != "unknown" || remote.ParentSession != "" || remote.RootSession != "" || len(remote.ModelEffort) != 0 {
		t.Errorf("gateway-only identity was guessed: %+v", remote)
	}
	if !strings.Contains(listResponse.Body.String(), `"agent":"claude"`) {
		t.Fatal("Claude gateway session missing from the mixed list")
	}
	var rawList struct {
		Sessions []json.RawMessage `json:"sessions"`
	}
	if err := json.Unmarshal(listResponse.Body.Bytes(), &rawList); err != nil {
		t.Fatal(err)
	}
	for _, raw := range rawList.Sessions {
		var fields map[string]json.RawMessage
		if err := json.Unmarshal(raw, &fields); err != nil {
			t.Fatal(err)
		}
		if string(fields["agent"]) != `"claude"` {
			continue
		}
		for _, key := range []string{"role", "parent_session", "root_session", "model_effort"} {
			if _, ok := fields[key]; ok {
				t.Errorf("non-Codex session has %s: %s", key, raw)
			}
		}
	}

	before, ok := sessions.Get("codex:child-b")
	if !ok {
		t.Fatal("child session missing before /one")
	}
	oneResponse := request("/api/sessions/one?key=codex:child-b")
	var one sessions.Session
	if err := json.Unmarshal(oneResponse.Body.Bytes(), &one); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(before.Models, one.Models) {
		t.Fatalf("/one changed the existing models list: got %+v want %+v", one.Models, before.Models)
	}
	if !reflect.DeepEqual(child.ModelEffort, one.ModelEffort) || one.Role != child.Role || one.ParentSession != child.ParentSession || one.RootSession != child.RootSession {
		t.Fatalf("/one disagrees with list: child=%+v one=%+v", child, one)
	}
	if row, ok := codexEffortByModel(one.ModelEffort, "child-model", "low"); !ok || row.Input != 740 || row.Output != 74 {
		t.Errorf("/one child all-time use = %+v, found=%t", row, ok)
	}
	remoteOne := request("/api/sessions/one?key=codex:gateway-only")
	if !strings.Contains(remoteOne.Body.String(), `"role":"unknown"`) || strings.Contains(remoteOne.Body.String(), `"model_effort"`) {
		t.Fatalf("gateway-only /one identity: %s", remoteOne.Body)
	}

	manageResponse := request("/api/sessions/manage?agent=codex")
	var managed struct {
		Sessions []sessions.Managed `json:"sessions"`
	}
	if err := json.Unmarshal(manageResponse.Body.Bytes(), &managed); err != nil {
		t.Fatal(err)
	}
	managedSessions := make([]sessions.Session, len(managed.Sessions))
	for i := range managed.Sessions {
		managedSessions[i] = managed.Sessions[i].Session
	}
	managedChild := codexSessionByID(t, managedSessions, "codex", "child-b")
	if !reflect.DeepEqual(child.ModelEffort, managedChild.ModelEffort) || managedChild.Role != child.Role {
		t.Fatalf("manage disagrees with list: child=%+v managed=%+v", child, managedChild)
	}
	if row, ok := codexEffortByModel(managedChild.ModelEffort, "child-model", "low"); !ok || row.Input != 740 || row.Output != 74 {
		t.Errorf("manage child all-time use = %+v, found=%t", row, ok)
	}
	if got := codexSessionByID(t, managedSessions, "codex", "gateway-only"); got.Role != "unknown" || len(got.ModelEffort) != 0 {
		t.Errorf("gateway-only manage row: %+v", got)
	}

	statsBefore := statsFor(7)
	overview7Response := request("/api/sessions/overview?days=7&agent=codex")
	var overview7 sessions.Overview
	if err := json.Unmarshal(overview7Response.Body.Bytes(), &overview7); err != nil {
		t.Fatal(err)
	}
	for _, top := range overview7.Top {
		for _, summary := range top {
			if summary.ID == "old-thread" {
				t.Fatalf("seven-day overview included old thread: %+v", summary)
			}
		}
	}
	for _, key := range []string{"tokens", "cost", "active"} {
		top, ok := overview7.Top[key]
		if !ok {
			t.Fatalf("overview has no %s top", key)
		}
		var childSummary *sessions.Summary
		for i := range top {
			if top[i].Agent == "codex" && top[i].ID == "child-b" {
				childSummary = &top[i]
				break
			}
		}
		if childSummary == nil {
			t.Fatalf("child-b is missing from overview's %s top: %+v", key, top)
		}
		if row, ok := codexEffortByModel(childSummary.ModelEffort, "child-model", "low"); !ok || row.Input != 40 || row.Output != 4 {
			t.Errorf("seven-day overview %s child use = %+v, found=%t", key, row, ok)
		}
		if row, ok := codexEffortByModel(childSummary.ModelEffort, "wire-model", "high"); !ok || row.Input != 500 || !row.Priced {
			t.Errorf("seven-day overview %s gateway use = %+v, found=%t", key, row, ok)
		}
	}
	for _, summary := range statsBefore.Sessions {
		if summary.Agent == "codex" && len(summary.ModelEffort) != 0 {
			t.Fatalf("overview mutated cached stats summary: %+v", summary)
		}
	}

	allResponse := request("/api/sessions/overview?days=0&agent=codex")
	var overviewAll sessions.Overview
	if err := json.Unmarshal(allResponse.Body.Bytes(), &overviewAll); err != nil {
		t.Fatal(err)
	}
	foundOld := false
	for _, summary := range overviewAll.Top["tokens"] {
		if summary.ID == "old-thread" {
			foundOld = true
			if summary.Role != "main" || len(summary.ModelEffort) != 1 || summary.ModelEffort[0].Model != "old-model" {
				t.Errorf("all-time old summary: %+v", summary)
			}
		}
	}
	if !foundOld {
		t.Fatal("all-time overview did not include old Codex thread")
	}
	for _, key := range []string{"tokens", "cost", "active"} {
		top, ok := overviewAll.Top[key]
		if !ok {
			t.Fatalf("all-time overview has no %s top", key)
		}
		var childSummary *sessions.Summary
		for i := range top {
			if top[i].Agent == "codex" && top[i].ID == "child-b" {
				childSummary = &top[i]
				break
			}
		}
		if childSummary == nil {
			t.Fatalf("child-b is missing from all-time overview's %s top: %+v", key, top)
		}
		if row, ok := codexEffortByModel(childSummary.ModelEffort, "child-model", "low"); !ok || row.Input != 740 || row.Output != 74 {
			t.Errorf("all-time overview %s child use = %+v, found=%t", key, row, ok)
		}
	}

	usageMux := http.NewServeMux()
	usageRoutes(usageMux, folderOnly{})
	guiUsage := httptest.NewRecorder()
	usageMux.ServeHTTP(guiUsage, httptest.NewRequest("GET", "/api/usage/requests?period=all", nil))
	if guiUsage.Code != http.StatusOK {
		t.Fatalf("GUI usage requests: %d %s", guiUsage.Code, guiUsage.Body)
	}
	gwUsage := httptest.NewRecorder()
	gwRequest := httptest.NewRequest("GET", "/v1/magpie/usage/requests?period=all", nil)
	gwRequest.RemoteAddr = "127.0.0.1:5000"
	gateway.New().Handler().ServeHTTP(gwUsage, gwRequest)
	if gwUsage.Code != http.StatusOK {
		t.Fatalf("gateway usage requests: %d %s", gwUsage.Code, gwUsage.Body)
	}
	for name, body := range map[string][]byte{"gui": guiUsage.Body.Bytes(), "gateway": gwUsage.Body.Bytes()} {
		var page ledgerJSON
		if err := json.Unmarshal(body, &page); err != nil {
			t.Fatalf("%s usage requests: %v", name, err)
		}
		var matched []ledgerRow
		for _, row := range page.Rows {
			if row.ResponseID == "paired-response" {
				matched = append(matched, row)
			}
		}
		if len(matched) != 1 {
			t.Fatalf("%s returned %d copies of the response: %s", name, len(matched), body)
		}
		row := matched[0]
		if row.SessionRole != "subagent" || row.Session != "main-a" || row.Subagent != "child-b" || row.ParentAgent != "" || row.NativeSession != "shared-child" || row.Model != "wire-model" || row.Effort != "high" || row.Input != 500 || row.Output != 50 || !row.Priced || math.Abs(row.Cost-0.0015) > 1e-12 {
			t.Errorf("%s canonical response row: %+v", name, row)
		}
	}
}
