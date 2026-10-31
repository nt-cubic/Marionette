package gui

import (
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/sessions"
)

// A stats read again behind that finds the same stats keeps their gen, so
// the page's next ask with its tag is answered the same without the stats
// encoded again; stats that changed are sent whole, whatever tag was kept
// for the old ones (#2278).
func TestStatsTagKeptWhileTheSame(t *testing.T) {
	h := sandboxHome(t)
	proj := filepath.Join(h, ".claude", "projects", "-work")
	if err := os.MkdirAll(proj, 0o755); err != nil {
		t.Fatal(err)
	}
	write := func(id, at string) {
		b := `{"parentUuid":null,"isSidechain":false,"type":"user","message":{"role":"user","content":"hi"},"uuid":"u","timestamp":"` + at + `","cwd":"/work","sessionId":"` + id + `"}` + "\n" +
			`{"parentUuid":"u","isSidechain":false,"message":{"model":"claude-sonnet-5","id":"msg_` + id + `","type":"message","role":"assistant","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":10,"output_tokens":2}},"requestId":"req_1","type":"assistant","uuid":"a","timestamp":"` + at + `","cwd":"/work","sessionId":"` + id + `"}` + "\n"
		if err := os.WriteFile(filepath.Join(proj, id+".jsonl"), []byte(b), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write("00000001-1111-2222-3333-444444444444", "2026-09-20T10:00:00.000Z")
	t.Setenv("CLAUDE_CONFIG_DIR", filepath.Join(h, ".claude"))
	sessions.Reset()
	forgetStats()
	t.Cleanup(func() { sessions.Reset(); forgetStats() })

	mux := http.NewServeMux()
	sessionRoutes(mux, folderOnly{})
	get := func(had string) *httptest.ResponseRecorder {
		r := httptest.NewRequest("GET", "/api/sessions/stats?days=0", nil)
		if had != "" {
			r.Header.Set("X-Magpie-Had", had)
		}
		w := httptest.NewRecorder()
		mux.ServeHTTP(w, r)
		if w.Code != 200 {
			t.Fatalf("%d %s", w.Code, w.Body)
		}
		return w
	}
	kept := func() (uint64, bool) {
		statsMemo.Lock()
		defer statsMemo.Unlock()
		k := statsMemo.m[0]
		return k.gen, k.busy
	}
	// the kept stats made old, so the next ask reads them again behind
	age := func() {
		statsMemo.Lock()
		statsMemo.m[0].at = time.Now().Add(-time.Minute)
		statsMemo.Unlock()
	}
	settled := func() uint64 {
		for range 500 {
			if gen, busy := kept(); !busy {
				return gen
			}
			time.Sleep(10 * time.Millisecond)
		}
		t.Fatal("the read behind never ended")
		return 0
	}

	first := get("")
	tag := first.Header().Get("X-Magpie-Tag")
	gen := settled()
	age()
	if w := get(tag); w.Header().Get("X-Magpie-Same") != "1" {
		t.Fatalf("asked with its own tag: same %q", w.Header().Get("X-Magpie-Same"))
	}
	if got := settled(); got != gen {
		t.Errorf("read again behind with nothing changed, gen %d, want %d kept", got, gen)
	}
	if w := get(tag); w.Header().Get("X-Magpie-Same") != "1" || w.Header().Get("X-Magpie-Tag") != tag {
		t.Errorf("after the read behind, asked with its own tag: same %q, tag %q", w.Header().Get("X-Magpie-Same"), w.Header().Get("X-Magpie-Tag"))
	}

	write("00000002-1111-2222-3333-444444444444", "2026-09-21T10:00:00.000Z")
	sessions.Reset()
	age()
	get(tag) // starts the read behind; answered from the kept stats
	if got := settled(); got == gen {
		t.Fatalf("a new session read behind kept gen %d", got)
	}
	w := get(tag)
	if w.Header().Get("X-Magpie-Same") != "" || w.Body.Len() == 0 || w.Header().Get("X-Magpie-Tag") == tag {
		t.Errorf("after the stats changed, asked with the old tag: same %q, %d bytes, tag %q", w.Header().Get("X-Magpie-Same"), w.Body.Len(), w.Header().Get("X-Magpie-Tag"))
	}
}
