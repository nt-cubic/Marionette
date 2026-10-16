package gateway

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A Claude Desktop session on another magpie's Claude model keeps Claude
// Code's own background requests on the model they name, as served by that
// magpie: its auto mode's classifier asks for claude-sonnet-5 by Claude's
// id, which the Remote magpie lists as office/claude/claude-sonnet-5, and
// went to the session's office/claude/claude-opus-5-5 instead (#2187,
// SuInk: 9 sonnet calls answered by opus, 480K tokens of 1h cache written).
// Desktop's title still follows the session, and a model the remote
// doesn't serve still goes to the session's.
func TestClaudeDesktopClassifierOnRemoteMagpie(t *testing.T) {
	reply := sse(
		`event: message_start`+"\n"+`data: {"type":"message_start","message":{"id":"m","type":"message","role":"assistant","model":"m1","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}`,
		`event: message_stop`+"\n"+`data: {"type":"message_stop"}`)
	f := &fake{reply: reply}
	up := setup(t, provider.Anthropic, f)
	// the other computer's Claude model, under a provider "work": its
	// subscription is "claude" there, which a test can't sign in to
	if err := provider.Save(provider.Provider{ID: "work", Name: "Work", Key: "k", Anthropic: up.URL,
		Models: []string{"claude-opus-5-5", "claude-sonnet-5", "claude-haiku-5-5"}}); err != nil {
		t.Fatal(err)
	}
	h := New().Handler()
	var mu sync.Mutex
	var hops []string // the models the other magpie was asked for
	remote := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			h.ServeHTTP(w, r) // its model list, as that magpie gives it
			return
		}
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		hops = append(hops, gjsonModel(b))
		mu.Unlock()
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, reply)
	}))
	t.Cleanup(remote.Close)
	id, err := provider.Add(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset, Chat: strings.TrimPrefix(remote.URL, "http://")})
	if err != nil {
		t.Fatal(err)
	}
	office, err := provider.Find(id)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := office.Fetch(context.Background()); err != nil {
		t.Fatal(err)
	}
	// here, only the other magpie serves Claude
	if err := provider.Delete("work"); err != nil {
		t.Fatal(err)
	}
	if e, ok := entryOf("office/work/claude-sonnet-5"); !ok || e.Model != "work/claude-sonnet-5" {
		t.Fatalf("office's sonnet: %+v %v", e, ok)
	}
	if !unserved("claude-sonnet-5") {
		t.Fatal("claude-sonnet-5 is served here by its bare id; the test needs it not to be")
	}
	desktopPicked.Lock()
	desktopPicked.model, desktopPicked.from = "", ""
	desktopPicked.Unlock()

	const cli = "claude-cli/2.1.295 (external, claude-desktop)"
	const electron = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Claude/1.6608.0 Chrome/142.0.7444.235 Electron/39.2.7 Safari/537.36"
	send := func(ua, body string) string {
		t.Helper()
		mu.Lock()
		hops = nil
		mu.Unlock()
		req := httptest.NewRequest("POST", "/v1/messages", strings.NewReader(body))
		req.Header.Set("x-api-key", Token+"-claude-desktop")
		req.Header.Set("User-Agent", ua)
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, req)
		if rec.Code != 200 {
			t.Fatalf("%d %s", rec.Code, rec.Body)
		}
		mu.Lock()
		defer mu.Unlock()
		if len(hops) != 1 {
			t.Fatalf("the other magpie was asked %v", hops)
		}
		return hops[0]
	}
	// the session is on the other magpie's opus, by the id Desktop was
	// shown it by
	opus, _ := entryOf("office/work/claude-opus-5-5")
	if got := send(cli, `{"model":"`+DesktopID(opus)+`","max_tokens":64000,"stream":true,"tools":[{"name":"Bash","input_schema":{"type":"object"}}],"messages":[{"role":"user","content":"hi"}]}`); got != "work/claude-opus-5-5" {
		t.Fatalf("turn: sent %q", got)
	}
	// Claude Code's safety and notification classifiers name their sonnet
	for _, body := range []string{
		`{"model":"claude-sonnet-5","max_tokens":64,"stream":true,"system":[{"type":"text","text":"x-anthropic-billing-header: cc_version=2.1.295; cc_entrypoint=claude-desktop;"},{"type":"text","text":"You are a security monitor for autonomous AI coding agents."}],"messages":[{"role":"user","content":"<transcript>…</transcript>"}]}`,
		`{"model":"claude-sonnet-5","max_tokens":1024,"stream":true,"system":[{"type":"text","text":"A user kicked off a Claude Code agent to do a coding task and walked away."}],"messages":[{"role":"user","content":"…"}]}`,
	} {
		if got := send(cli, body); got != "work/claude-sonnet-5" {
			t.Fatalf("classifier: the other magpie was asked for %q, want work/claude-sonnet-5", got)
		}
	}
	// Desktop's title follows the session
	title := `{"model":"claude-sonnet-5","max_tokens":200,"stream":true,"system":"You write short session titles. Reply with only the tagged fields the prompt asks for.","messages":[{"role":"user","content":"hi"}]}`
	if got := send(electron, title); got != "work/claude-opus-5-5" {
		t.Fatalf("title: sent %q", got)
	}
	// a Claude model the other magpie doesn't serve goes to the session's
	if got := send(cli, `{"model":"claude-haiku-4-5","max_tokens":64,"stream":true,"messages":[{"role":"user","content":"hi"}]}`); got != "work/claude-opus-5-5" {
		t.Fatalf("unserved: sent %q", got)
	}
}

func entryOf(id string) (provider.Entry, bool) {
	for _, e := range provider.Catalog() {
		if e.ID == id {
			return e, true
		}
	}
	return provider.Entry{}, false
}
