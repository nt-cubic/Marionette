package gateway

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// grokShape422 is xAI's answer to an input item its API doesn't know (#350).
const grokShape422 = `{"error":"Failed to deserialize the JSON body into the target type: input[0]: unknown item type \"additional_tools\"; expected one of: message, reasoning, function_call"}`

// A member whose API can't read the request's shape hands it to the next
// one, asked once, and doesn't rest for it: the next request asks it first
// again (#350). Nobody left, the agent gets its error, asked no more.
func TestGroupFailsOverARequestShape(t *testing.T) {
	fresh(t)
	a := &scripted{replies: []reply{{422, "", grokShape422}}}
	b := &scripted{replies: []reply{{200, "", chatOK}}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	for i := 1; i <= 2; i++ {
		code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
		if code != 200 || !strings.Contains(body, "hello") || a.n != i || b.n != i {
			t.Fatalf("request %d: %d %s (a %d, b %d)", i, code, body, a.n, b.n)
		}
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	if len(r.Tries) != 2 || r.Tries[0].Status != 422 || r.Tries[0].Fail != failShape || r.Tries[0].Rest != nil {
		t.Fatalf("tries %+v", r.Tries)
	}

	if err := provider.SaveGroup(provider.Group{Name: "Solo", Members: []string{"a/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	a.n = 0
	if code, body := postAs(t, s, "", `{"model":"group/solo","messages":[{"role":"user","content":"hi"}]}`); code != 422 || !strings.Contains(body, "unknown item type") || a.n != 1 {
		t.Fatalf("alone: %d %s (a %d)", code, body, a.n)
	}

	for _, x := range []struct {
		status int
		body   string
		want   bool
	}{
		{422, grokShape422, true},
		{400, `{"error":{"message":"Unknown parameter: 'reasoning.summary'.","code":"unknown_parameter"}}`, true},
		{400, `{"error":{"message":"messages: field required"}}`, false},
		{400, `{"error":{"message":"This model's maximum context length is 128000 tokens"}}`, false},
		{500, grokShape422, false},
		{400, openCodeGoDepth400, true},
		{400, `OpenCode Go: JSON schema exceeds the maximum nesting depth of 10 levels`, true},
		{200, openCodeGoDepth400, false},
		{400, `{"error":{"message":"tools.0.input_schema: Field required"}}`, false},
		{400, `{"error":{"message":"Invalid schema for function 'x': 'object' is not a valid type"}}`, false},
	} {
		if got := shapeRefused(x.status, []byte(x.body)); got != x.want {
			t.Errorf("shapeRefused(%d, %s) = %v", x.status, x.body, got)
		}
	}
}

// openCodeGoDepth400 is OpenCode Go's answer on muse-spark-1.3-contributor
// to Claude Code's MCP tools (WardLu on #1621, #1870), which magpie showed
// as "400 OpenCode Go: JSON schema exceeds the maximum nesting depth of 10
// levels": the vendor's message in the OpenAI error shape it answers in.
const openCodeGoDepth400 = `{"error":{"message":"JSON schema exceeds the maximum nesting depth of 10 levels","type":"invalid_request_error"}}`

// A model whose tool schemas may nest only so deep is a member's limit, not
// a bad request: the group's next member is asked, and the one that
// refused doesn't rest (#1870). Before, the route ended on the 400 with the
// later members never asked.
func TestGroupFailsOverASchemaTooDeep(t *testing.T) {
	fresh(t)
	a := &scripted{replies: []reply{{400, "", openCodeGoDepth400}}}
	b := &scripted{replies: []reply{{200, "", chatOK}}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "hello") || a.n != 1 || b.n != 1 {
		t.Fatalf("%d %s (a %d, b %d)", code, body, a.n, b.n)
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	if len(r.Tries) != 2 || r.Tries[0].Status != 400 || r.Tries[0].Fail != failShape || r.Tries[0].Rest != nil {
		t.Fatalf("tries %+v", r.Tries)
	}
}

// commandCodeBare400 is Command Code's Provider API turning a request away
// with nothing but its trace id, as lqisgod's usage.jsonl has it (#2313).
const commandCodeBare400 = `{"message":"invalid request error trace_id: 251c3c29761c17fdc459480ed9c68932","type":"invalid_request_error"}`

// Command Code's bare 400 is its backend's, not the request's: the same
// bytes sent again answer 200 (CommandCodeAI/command-code#960). The
// group's next member is asked and Command Code doesn't rest; before, the
// route ended on it with the later members never asked (#2313).
func TestGroupFailsOverCommandCodesBare400(t *testing.T) {
	fresh(t)
	a := &scripted{replies: []reply{{400, "", commandCodeBare400}}}
	b := &scripted{replies: []reply{{200, "text/event-stream", chatStreamOK}}}
	scriptedOn(t, "commandcode", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"commandcode/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	code, body := postAs(t, s, "", `{"model":"group/g","stream":true,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "from b") || a.n != 1 || b.n != 1 {
		t.Fatalf("%d %s (a %d, b %d)", code, body, a.n, b.n)
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	if len(r.Tries) != 2 || r.Tries[0].Status != 400 || r.Tries[0].Fail != failShape || r.Tries[0].Rest != nil {
		t.Fatalf("tries %+v", r.Tries)
	}

	for _, x := range []struct {
		status int
		body   string
		want   bool
	}{
		{400, commandCodeBare400, true},
		{400, `Command Code: ` + commandCodeBare400, true},
		// as a client behind Command Code saw it, wrapped, and its 422
		// of the week before (CommandCodeAI/command-code#952)
		{400, `{"message":"{\"message\":\"invalid request error trace_id: a050673b3298a747a15895803095bcf0\",\"type\":\"invalid_request_error\"}\n","type":"invalid_request_error"}`, true},
		{422, `{"message":"{\"message\":\"invalid request error trace_id: f9965636e3da815215348ef4ddf2ae63\",\"type\":\"invalid_request_error\"}\n","type":"server_error"}`, true},
		// its overflow, and a parameter it names, are the request's
		{400, `{"message":"The input is longer than the model's context length trace_id: e811a6f10da868000cc12a043c07a73b","type":"invalid_request_error"}`, false},
		{400, `{"error":{"message":"Invalid option: expected one of \"low\"|\"medium\"|\"high\"|\"xhigh\"|\"max\"","type":"invalid_request_error","param":"reasoning_effort"}}`, false},
		// a message that says more after its trace id
		{400, `{"message":"invalid request error trace_id: 251c3c29761c17fdc459480ed9c68932: messages.3.content too long","type":"invalid_request_error"}`, false},
		{200, commandCodeBare400, false},
	} {
		if got := shapeRefused(x.status, []byte(x.body)); got != x.want {
			t.Errorf("shapeRefused(%d, %s) = %v", x.status, x.body, got)
		}
	}
}
