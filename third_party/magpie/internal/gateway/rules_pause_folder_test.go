package gateway

import (
	"encoding/json"
	"net/http/httptest"
	"runtime"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// #1606 (Ti800): a subscription's quota kept for the long task in one
// folder. A pause outside that folder leaves the member out of every
// request from elsewhere — another project, or an agent that doesn't say
// its folder — not even as a fallback, while the task's own requests
// still reach it; a pause in some folders leaves it out of theirs only.
func TestPauseByFolder(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("the fixtures' folders are Unix paths")
	}
	group := func(body []byte) string {
		var m map[string]any
		if err := json.Unmarshal(body, &m); err != nil {
			t.Fatal(err)
		}
		m["model"], m["stream"] = "group/r", false
		delete(m, "thinking")
		delete(m, "reasoning")
		b, _ := json.Marshal(m)
		return string(b)
	}
	elsewhere := func(name string) string {
		return strings.ReplaceAll(group(fixture(t, name)), "/Users/me/work/proj", "/Users/me/scratch")
	}
	post := func(s *Server, path, body, session string) (int, string, Route) {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest("POST", path, strings.NewReader(body))
		req.Header.Set("x-session-id", session)
		s.Handler().ServeHTTP(rec, req)
		return rec.Code, rec.Body.String(), lastRoute(s)
	}
	keep := provider.Rule{Use: "a/small", Pause: true, ExceptFolders: []string{"/Users/me/work/proj"}}

	// a/small is the subscription, kept for /Users/me/work/proj
	s, _, _ := ruled(t, keep)
	for _, tc := range []struct {
		name, path, body, folder string
		paused                   bool
	}{
		{"claude code in the task", "/v1/messages", group(fixture(t, "claude-code-2.1.296.json")), "", false},
		{"codex in the task", "/v1/responses", group(fixture(t, "codex-0.162.0.json")), "", false},
		{"pi in the task", "/v1/chat/completions", group(fixture(t, "pi-1.1.0.json")), "", false},
		{"claude code elsewhere", "/v1/messages", elsewhere("claude-code-2.1.296.json"), "/Users/me/scratch/a&b <c> 中文", true},
		{"codex elsewhere", "/v1/responses", elsewhere("codex-0.162.0.json"), "/Users/me/scratch/a&b <c> 中文", true},
		{"no folder said", "/v1/chat/completions", chat("hi", nil, 0, ""), "", true},
	} {
		code, out, r := post(s, tc.path, tc.body, tc.name)
		want := map[bool]string{false: "from ka", true: "from kb"}[tc.paused]
		if code != 200 || !strings.Contains(out, want) {
			t.Errorf("%s: %d %.300s", tc.name, code, out)
			continue
		}
		p := r.Group.Paused
		if !tc.paused {
			if len(p) != 0 {
				t.Errorf("%s: paused %+v", tc.name, p)
			}
			continue
		}
		if len(p) != 1 || p[0].Member != "a/small" || !p[0].ByFolder || p[0].Folder != tc.folder || strings.Join(p[0].When, " · ") != "not folder /Users/me/work/proj" {
			t.Errorf("%s: paused %+v", tc.name, p)
		}
	}

	// elsewhere, a/small isn't a fallback either
	s, a, b := ruled(t, keep)
	b.fail = 500
	if code, _, _ := post(s, "/v1/messages", elsewhere("claude-code-2.1.296.json"), "f"); code == 200 || a.n() != 0 {
		t.Errorf("paused member asked as a fallback: %d, a asked %d", code, a.n())
	}

	// a pause in some folders: theirs only, never a request that says none
	s, _, _ = ruled(t, provider.Rule{Use: "a/small", Pause: true, Folders: []string{"/Users/me/scratch"}})
	if code, out, _ := post(s, "/v1/messages", elsewhere("claude-code-2.1.296.json"), "g1"); code != 200 || !strings.Contains(out, "from kb") {
		t.Errorf("in the paused folder: %d %.300s", code, out)
	}
	for i, body := range []string{group(fixture(t, "pi-1.1.0.json")), chat("hi", nil, 0, "")} {
		if code, out, r := post(s, "/v1/chat/completions", body, "g2"+string(rune('a'+i))); code != 200 || !strings.Contains(out, "from ka") || len(r.Group.Paused) != 0 {
			t.Errorf("outside the paused folder %d: %d %.300s", i, code, out)
		}
	}
}
