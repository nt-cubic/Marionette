package gui

import (
	"maps"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/settings"
)

// The tray panel's Allowances tab leaves out the subscriptions set hidden
// there (H20 on Discord), kept beside the Usage page's order: each is saved
// on its own, the other left as it was, and both outlast a save of the
// Settings page.
func TestUsageArrangePanelHidden(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	t.Setenv("APPDATA", filepath.Join(h, "AppData", "Roaming"))
	t.Setenv("LOCALAPPDATA", filepath.Join(h, "AppData", "Local"))
	post := func(path, body string) {
		t.Helper()
		rec := httptest.NewRecorder()
		Handler(nil, nil).ServeHTTP(rec, httptest.NewRequest("POST", path, strings.NewReader(body)))
		if rec.Code != http.StatusOK {
			t.Fatalf("%s: %d %s", path, rec.Code, rec.Body)
		}
	}
	want := func(order, hidden []string) {
		t.Helper()
		s := settings.Load()
		if !slices.Equal(s.UsageOrder, order) || !slices.Equal(s.PanelUsageHidden, hidden) {
			t.Fatalf("order %v hidden %v, want %v %v", s.UsageOrder, s.PanelUsageHidden, order, hidden)
		}
	}
	post("/api/usage/arrange", `{"order":["kimi","codex"]}`)
	want([]string{"kimi", "codex"}, nil)
	post("/api/usage/arrange", `{"panelHidden":["deepseek"," deepseek","claude"]}`)
	want([]string{"kimi", "codex"}, []string{"deepseek", "claude"})
	post("/api/usage/arrange", `{"order":["codex","kimi"]}`)
	want([]string{"codex", "kimi"}, []string{"deepseek", "claude"})
	post("/api/settings", `{"theme":"dark"}`)
	want([]string{"codex", "kimi"}, []string{"deepseek", "claude"})
	post("/api/usage/arrange", `{"panelHidden":[]}`)
	want([]string{"codex", "kimi"}, nil)
}

// The Usage page's Agents list leaves out the clients hidden there
// (ikunbots on X: curl, Go-http-client, python-httpx beside the agents in
// use): saved on its own, beside the panel's, and kept through a save of
// the Settings page.
func TestUsageArrangeAgentsHidden(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	t.Setenv("APPDATA", filepath.Join(h, "AppData", "Roaming"))
	t.Setenv("LOCALAPPDATA", filepath.Join(h, "AppData", "Local"))
	post := func(path, body string) {
		t.Helper()
		rec := httptest.NewRecorder()
		Handler(nil, nil).ServeHTTP(rec, httptest.NewRequest("POST", path, strings.NewReader(body)))
		if rec.Code != http.StatusOK {
			t.Fatalf("%s: %d %s", path, rec.Code, rec.Body)
		}
	}
	want := func(agents, panel []string) {
		t.Helper()
		s := settings.Load()
		if !slices.Equal(s.UsageAgentsHidden, agents) || !slices.Equal(s.PanelUsageHidden, panel) {
			t.Fatalf("agents hidden %v panel hidden %v, want %v %v", s.UsageAgentsHidden, s.PanelUsageHidden, agents, panel)
		}
	}
	post("/api/usage/arrange", `{"panelHidden":["deepseek"]}`)
	post("/api/usage/arrange", `{"agentsHidden":["curl","Go-http-client"," curl"]}`)
	want([]string{"curl", "Go-http-client"}, []string{"deepseek"})
	post("/api/settings", `{"theme":"dark"}`)
	want([]string{"curl", "Go-http-client"}, []string{"deepseek"})
	post("/api/usage/arrange", `{"agentsHidden":["Go-http-client"]}`)
	want([]string{"Go-http-client"}, []string{"deepseek"})
	post("/api/usage/arrange", `{"agentsHidden":[]}`)
	want(nil, []string{"deepseek"})
}

// A client magpie doesn't know is counted as one of its agents from the
// Usage page's Agents list (#1958, NorethSea), saved on its own and kept
// through a save of the Settings page; As "" counts it on its own again.
// An agent magpie knows isn't counted as another, nor anything as
// magpie, nor as a client that isn't an agent.
func TestUsageArrangeCountClient(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	t.Setenv("APPDATA", filepath.Join(h, "AppData", "Roaming"))
	t.Setenv("LOCALAPPDATA", filepath.Join(h, "AppData", "Local"))
	post := func(path, body string) int {
		t.Helper()
		rec := httptest.NewRecorder()
		Handler(nil, nil).ServeHTTP(rec, httptest.NewRequest("POST", path, strings.NewReader(body)))
		return rec.Code
	}
	want := func(m map[string]string) {
		t.Helper()
		if got := settings.Load().ClientAgents; !maps.Equal(got, m) {
			t.Fatalf("clientAgents %v, want %v", got, m)
		}
	}
	if c := post("/api/usage/arrange", `{"count":{"client":"hermes-codex-thread-bridge","as":"codex"}}`); c != http.StatusOK {
		t.Fatalf("count: %d", c)
	}
	want(map[string]string{"hermes-codex-thread-bridge": "codex"})
	post("/api/settings", `{"theme":"dark"}`)
	want(map[string]string{"hermes-codex-thread-bridge": "codex"})
	for _, bad := range []string{
		`{"count":{"client":"claude-code","as":"codex"}}`,
		`{"count":{"client":"magpie-router","as":"codex"}}`,
		`{"count":{"client":"my-bridge","as":"magpie"}}`,
		`{"count":{"client":"my-bridge","as":"Python-urllib"}}`,
		`{"count":{"client":"","as":"codex"}}`,
	} {
		if c := post("/api/usage/arrange", bad); c == http.StatusOK {
			t.Fatalf("%s was taken", bad)
		}
	}
	want(map[string]string{"hermes-codex-thread-bridge": "codex"})
	post("/api/usage/arrange", `{"count":{"client":"Hermes-Codex-Thread-Bridge","as":""}}`)
	want(nil)
}
