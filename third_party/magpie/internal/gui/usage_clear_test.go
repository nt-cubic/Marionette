package gui

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/usage"
)

// The Usage page's clear on a model's row, and the one for all usage
// under the lists (CrazyMouse_TT on X), POST /api/usage/clear: the
// model's calls, or every call, leave the page's figures.
func TestUsageClearRoute(t *testing.T) {
	sandboxHome(t)
	at := time.Now().Add(-time.Hour)
	usage.Append(usage.Record{Time: at, Agent: "codex", Provider: "ollama", Model: "qwen3", Input: 10, Status: 200})
	usage.Append(usage.Record{Time: at, Agent: "codex", Provider: "ollama", Model: "llama4", Input: 5, Status: 200})
	mux := http.NewServeMux()
	usageRoutes(mux, folderOnly{})
	post := func(body string, code int) {
		t.Helper()
		w := httptest.NewRecorder()
		mux.ServeHTTP(w, httptest.NewRequest("POST", "/api/usage/clear", strings.NewReader(body)))
		if w.Code != code {
			t.Fatalf("POST %s: %d %s", body, w.Code, w.Body)
		}
	}
	post(`{"provider":"ollama","model":"qwen3"}`, 200)
	if s := usage.Summarize(usage.All); s.Calls != 1 || len(s.Models) != 1 || s.Models[0].Model != "llama4" {
		t.Fatalf("after clearing qwen3: %+v", s.Models)
	}
	post(`{"provider":"ollama"}`, http.StatusBadRequest)
	post(`{}`, 200)
	if s := usage.Summarize(usage.All); s.Calls != 0 {
		t.Fatalf("after clearing everything: %d calls", s.Calls)
	}
	if got := settings.Load().UsageCleared; len(got) != 1 {
		t.Fatalf("settings: %+v", got)
	}
}
