package gateway

import (
	"context"
	"encoding/json"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// masaka on Discord: clicking a part of the context window card shows the
// text the request sent there — the system prompt, each tool's definition,
// each turn — masked as the vendor got it, even where the request went to
// a model the user let have it unmasked. The route, which goes in the
// routing history on disk, still holds no text.
func TestPromptTextIsWhatWasSent(t *testing.T) {
	const key = "sk-proj-abcdEFGH1234ijklMNOP5678qrst"
	for _, unmasked := range []bool{false, true} {
		f := &fake{ctype: "application/json", reply: `{"id":"msg_1","type":"message","role":"assistant","model":"m1","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":300,"output_tokens":1}}`}
		setup(t, provider.Anthropic, f)
		if unmasked {
			p, err := provider.Find("fake")
			if err != nil {
				t.Fatal(err)
			}
			p.Unredacted = true
			if err := provider.Save(*p); err != nil {
				t.Fatal(err)
			}
		}
		if err := settings.Save(settings.Settings{Redact: true}); err != nil {
			t.Fatal(err)
		}
		body := `{"model":"fake/m1","max_tokens":10,
			"system":"You are the release agent. Ship carefully; the deploy key is ` + key + `.",
			"tools":[{"name":"Deploy","description":"Deploys the build to production.","input_schema":{"type":"object","properties":{"env":{"type":"string"}}}}],
			"messages":[
				{"role":"user","content":"ship the build"},
				{"role":"assistant","content":[{"type":"text","text":"Deploying."},{"type":"tool_use","id":"c1","name":"Deploy","input":{"env":"prod"}}]},
				{"role":"user","content":[{"type":"tool_result","tool_use_id":"c1","content":"deployed to prod in 42s"}]}]}`
		s := New()
		rec := httptest.NewRecorder()
		s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/messages", strings.NewReader(body)))
		if rec.Code != 200 {
			t.Fatalf("%d %s", rec.Code, rec.Body)
		}
		if strings.Contains(string(f.got), key) != unmasked {
			t.Fatalf("unmasked %v sent %s", unmasked, f.got)
		}
		rs := s.Trace(context.Background(), 0, 0).Routes
		if len(rs) != 1 || rs[0].Prompt == nil {
			t.Fatalf("%+v", rs)
		}
		id := rs[0].ID
		text := func(kind string) string {
			pt, ok := s.PromptText(id, kind, true)
			if !ok {
				t.Fatalf("%s: not kept", kind)
			}
			b, _ := json.Marshal(pt)
			return string(b)
		}
		sys := text(PartSystem)
		if !strings.Contains(sys, "Ship carefully; the deploy key is") || strings.Contains(sys, key) {
			t.Fatalf("unmasked %v system %s", unmasked, sys)
		}
		if tools := text(PartTools); !strings.Contains(tools, `Deploys the build to production.`) || !strings.Contains(tools, `\"env\"`) {
			t.Fatalf("tools %s", tools)
		}
		if res := text(PartResults); !strings.Contains(res, "deployed to prod in 42s") {
			t.Fatalf("results %s", res)
		}
		chat, _ := s.PromptText(id, PartChat, true)
		var roles []string
		for _, e := range chat.Items {
			for _, pc := range e.Pieces {
				roles = append(roles, e.Name+":"+pc.Role+":"+pc.Name)
			}
		}
		if got := strings.Join(roles, " "); got != "1:user: 1:assistant: 1:call:Deploy" {
			t.Fatalf("chat %s", got)
		}
		// listed without text when no item is asked for
		if l, _ := s.PromptText(id, PartSystem, false); len(l.Items) != 1 || l.Items[0].Pieces != nil {
			t.Fatalf("list %+v", l)
		}
		// the route, as the history keeps it, holds names only
		b, _ := json.Marshal(rs[0])
		if strings.Contains(string(b), "Ship carefully") || strings.Contains(string(b), "Deploys the build") {
			t.Fatalf("route carries text: %s", b)
		}
	}
}

// Only the last promptBodies requests' bodies are kept, a body over
// promptBodyMax never, and together no more than promptBodiesMax.
func TestPromptTextKeepsOnlyTheLastBodies(t *testing.T) {
	var tr trace
	body := []byte(`{"system":"s","messages":[{"role":"user","content":"hi"}]}`)
	for id := int64(1); id <= promptBodies+2; id++ {
		tr.keepBody(id, provider.Anthropic, body)
	}
	for id := int64(1); id <= promptBodies+2; id++ {
		if _, ok := tr.keptBodyOf(id); ok != (id > 2) {
			t.Fatalf("body %d kept %v", id, ok)
		}
	}
	tr.keepBody(100, provider.Anthropic, make([]byte, promptBodyMax+1))
	if _, ok := tr.keptBodyOf(100); ok {
		t.Fatal("kept a body over promptBodyMax")
	}
	big := make([]byte, promptBodiesMax/3)
	for id := int64(200); id < 204; id++ {
		tr.keepBody(id, provider.Anthropic, big)
	}
	total := 0
	for _, b := range tr.bodies {
		total += len(b.body)
	}
	if _, ok := tr.keptBodyOf(203); !ok || total > promptBodiesMax {
		t.Fatalf("kept %d bytes", total)
	}
	if _, ok := tr.keptBodyOf(200); ok {
		t.Fatal("kept the oldest past promptBodiesMax")
	}
}

// A chat completions assistant turn is its reasoning, what it said and
// each call, a piece each; a call's arguments, a JSON string, read laid out.
func TestPromptTextChatPieces(t *testing.T) {
	body := []byte(`{"model":"m","messages":[
		{"role":"system","content":"be brief"},
		{"role":"user","content":"read it"},
		{"role":"assistant","reasoning_content":"I should read it.","content":"Reading.","tool_calls":[{"id":"c1","type":"function","function":{"name":"read","arguments":"{\"path\":\"a.go\"}"}}]},
		{"role":"tool","tool_call_id":"c1","content":"package a"}]}`)
	b, ok := promptPieces(provider.Chat, body)
	if !ok {
		t.Fatal("not read")
	}
	var got []string
	for _, pc := range b.pieces[PartChat+"\x00"+itemKey("turn", "1")] {
		pc = pc.read()
		got = append(got, pc.Role+"="+pc.Text)
	}
	want := "user=read it|thinking=I should read it.|assistant=Reading.|call={\n  \"path\": \"a.go\"\n}"
	if strings.Join(got, "|") != want {
		t.Fatalf("got %q", strings.Join(got, "|"))
	}
	// the sizes are what they were before the text was kept
	if p := promptOf(provider.Chat, body); partOf(p, PartChat).Tokens != func() int {
		n := 0
		for _, it := range b.parts[PartChat] {
			n += it.Tokens
		}
		return n
	}() {
		t.Fatalf("sizes differ")
	}
}
