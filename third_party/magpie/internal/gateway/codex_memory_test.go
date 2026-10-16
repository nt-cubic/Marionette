package gateway

import (
	"encoding/json"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/usage"
)

// codexCall is a request Codex with magpie as its provider sends to
// /v1/responses: a turn of its own (source "") or a call of a kind of its.
func codexCall(t *testing.T, s *Server, model, source string) *httptest.ResponseRecorder {
	t.Helper()
	rec := httptest.NewRecorder()
	req := httptest.NewRequest("POST", "/v1/responses", strings.NewReader(`{"model":"`+model+`","stream":true,"input":"hi"}`))
	req.Header.Set("Authorization", "Bearer "+TokenFor("codex"))
	if source != "" {
		req.Header.Set("x-codex-turn-metadata", `{"thread_source":"`+source+`"}`)
	}
	s.Handler().ServeHTTP(rec, req)
	return rec
}

// Codex's memory calls come on models of its own by their bare names
// (gpt-5.6-terra, gpt-5.6-luna), whatever Codex is set to (#742,
// congee949: every one was a 404 "magpie knows no model"). They go to the
// model Codex's own turns go to — the last one answered, or the ledger's
// after a restart — and are recorded as the memory calls they are. Any
// other call on a model magpie doesn't know is still turned away.
func TestCodexMemoryCallsGoWhereItsTurnsGo(t *testing.T) {
	f := &fake{t: t}
	setup(t, provider.Chat, f)
	reply := func() string {
		return sse(
			`data: {"id":"c1","choices":[{"index":0,"delta":{"content":"ok"}}]}`,
			`data: {"id":"c1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1}}`,
			`data: [DONE]`)
	}
	s := New()

	// no turn of Codex's answered yet: nothing to go to
	if rec := codexCall(t, s, "gpt-5.6-terra", "memory_consolidation"); rec.Code != 404 {
		t.Fatalf("a memory call before any turn: %d %q", rec.Code, rec.Body.String())
	}

	f.reply = reply()
	if rec := codexCall(t, s, "fake/m1", ""); rec.Code != 200 {
		t.Fatalf("a turn: %d %q", rec.Code, rec.Body.String())
	}
	for _, c := range []struct{ model, source string }{
		{"gpt-5.6-terra", "memory_consolidation"},
		{"gpt-5.6-luna", "memgen"},
	} {
		f.reply = reply()
		calls := f.calls
		if rec := codexCall(t, s, c.model, c.source); rec.Code != 200 || f.calls != calls+1 {
			t.Fatalf("%s on %s: %d %q", c.source, c.model, rec.Code, rec.Body.String())
		}
		var sent struct {
			Model string `json:"model"`
		}
		json.Unmarshal(f.got, &sent)
		if sent.Model != "m1" {
			t.Errorf("%s: the vendor was asked for %q", c.source, sent.Model)
		}
	}
	var last usage.Record
	for _, u := range usage.Load(time.Time{}) {
		last = u
	}
	if usage.PurposeOf(last.Kind) != "kind:memory_consolidation" || last.Provider != "fake" || last.Failed() {
		t.Errorf("the ledger's memory call %+v", last)
	}

	// magpie restarted: the ledger has the turn
	f.reply = reply()
	if rec := codexCall(t, New(), "gpt-5.6-terra", "memory_consolidation"); rec.Code != 200 {
		t.Fatalf("after a restart: %d %q", rec.Code, rec.Body.String())
	}

	// any other call on a model magpie doesn't know is turned away as before
	if rec := codexCall(t, s, "gpt-5.6-terra", ""); rec.Code != 404 {
		t.Fatalf("a turn on an unknown model: %d", rec.Code)
	}
	if rec := codexCall(t, s, "gpt-5.6-terra", "review"); rec.Code != 404 {
		t.Fatalf("a review on an unknown model: %d", rec.Code)
	}
}
