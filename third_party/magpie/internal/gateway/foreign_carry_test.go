package gateway

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// codexUnverifiable is Codex's refusal of reasoning another vendor or
// account sealed, as #4798's reporters quote it (zane668, ZanderH-code).
const codexUnverifiable = "The encrypted content KqMXgAAAAABpGZWl could not be verified. Reason: Encrypted content could not be decrypted or parsed."

// carrySealer is a Responses upstream that seals its reasoning as
// "sealed-by-<name>#<its call>", each item its own, reads only its own, and answers each request it
// takes with that reasoning and a call, as Grok and Codex answer a tool
// loop. It keeps every request's input.
type carrySealer struct {
	name    string
	refusal string // its words for reasoning it can't read, as a 400
	mu      sync.Mutex
	down    int // answering this status, for the conversation to move on
	n       int
	inputs  [][]map[string]any
}

func (s *carrySealer) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	b, _ := io.ReadAll(r.Body)
	var q struct {
		Input []map[string]any `json:"input"`
	}
	json.Unmarshal(b, &q)
	foreign := false
	for _, it := range q.Input {
		if enc, _ := it["encrypted_content"].(string); it["type"] == "reasoning" && !strings.HasPrefix(enc, "sealed-by-"+s.name+"#") {
			foreign = true
		}
	}
	s.mu.Lock()
	s.inputs = append(s.inputs, q.Input)
	down := s.down
	s.n++
	n := s.n
	s.mu.Unlock()
	w.Header().Set("Content-Type", "application/json")
	switch {
	case down != 0:
		w.WriteHeader(down)
		io.WriteString(w, `{"error":{"message":"Grok Build usage balance exhausted","type":"insufficient_quota"}}`)
		return
	case foreign:
		w.WriteHeader(400)
		msg, _ := json.Marshal(s.refusal)
		io.WriteString(w, `{"error":{"message":`+string(msg)+`,"type":"invalid_request_error","param":null}}`)
		return
	}
	w.Header().Set("Content-Type", "text/event-stream")
	call := fmt.Sprintf("call_%s_%d", s.name, n)
	io.WriteString(w, sse(
		`data: {"type":"response.created","response":{"id":"r`+call+`","model":"m"}}`,
		`data: {"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_`+call+`","summary":[],"encrypted_content":"sealed-by-`+s.name+`#`+call+`"}}`,
		`data: {"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_`+call+`","call_id":"`+call+`","name":"shell","arguments":"","status":"in_progress"}}`,
		`data: {"type":"response.function_call_arguments.delta","item_id":"fc_`+call+`","output_index":1,"delta":"{\"cmd\":\"cat a.txt\"}"}`,
		`data: {"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc_`+call+`","call_id":"`+call+`","name":"shell","arguments":"{\"cmd\":\"cat a.txt\"}","status":"completed"}}`,
		`data: {"type":"response.completed","response":{"id":"r`+call+`","usage":{"input_tokens":7,"output_tokens":1}}}`))
}

// taken is what each request since the last call carried: its reasoning's
// sealer, in order, and the call ids and arguments of its calls and results.
func (s *carrySealer) taken() (seals [][]string, calls []string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, in := range s.inputs {
		var got []string
		for _, it := range in {
			switch it["type"] {
			case "reasoning":
				by, _, _ := strings.Cut(strings.TrimPrefix(it["encrypted_content"].(string), "sealed-by-"), "#")
				got = append(got, by)
			case "function_call":
				calls = append(calls, it["call_id"].(string)+" "+it["arguments"].(string))
			case "function_call_output":
				calls = append(calls, it["call_id"].(string)+" -> "+fmt.Sprint(it["output"]))
			}
		}
		seals = append(seals, got)
	}
	s.inputs = nil
	return seals, calls
}

// carryClient is a client of one API in a tool loop: it keeps the calls it
// was given, ids as given, and sends them back with their results.
type carryClient struct {
	from   provider.Protocol
	stream bool
	calls  []string // the ids it was given, in order
}

func (c *carryClient) ask(t *testing.T, s *Server) (int, string) {
	t.Helper()
	const user = "Sum the numbers in a.txt"
	var path, body string
	st := fmt.Sprint(c.stream)
	switch c.from {
	case provider.Chat:
		msgs := `{"role":"user","content":"` + user + `"}`
		for _, id := range c.calls {
			msgs += `,{"role":"assistant","content":"Reading it.","tool_calls":[{"id":"` + id + `","type":"function","function":{"name":"shell","arguments":"{\"cmd\":\"cat a.txt\"}"}}]},{"role":"tool","tool_call_id":"` + id + `","content":"1 2 3"}`
		}
		path, body = "/v1/chat/completions", `{"model":"group/grok-codex","stream":`+st+`,"reasoning_effort":"high","tools":[{"type":"function","function":{"name":"shell","parameters":{"type":"object","properties":{"cmd":{"type":"string"}}}}}],"messages":[`+msgs+`]}`
	case provider.Anthropic:
		msgs := `{"role":"user","content":"` + user + `"}`
		for _, id := range c.calls {
			msgs += `,{"role":"assistant","content":[{"type":"text","text":"Reading it."},{"type":"tool_use","id":"` + id + `","name":"shell","input":{"cmd":"cat a.txt"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"` + id + `","content":"1 2 3"}]}`
		}
		path, body = "/v1/messages", `{"model":"group/grok-codex","stream":`+st+`,"max_tokens":32000,"thinking":{"type":"adaptive"},"tools":[{"name":"shell","input_schema":{"type":"object","properties":{"cmd":{"type":"string"}}}}],"messages":[`+msgs+`]}`
	case provider.Gemini:
		msgs := `{"role":"user","parts":[{"text":"` + user + `"}]}`
		for _, id := range c.calls {
			msgs += `,{"role":"model","parts":[{"text":"Reading it."},{"functionCall":{"id":"` + id + `","name":"shell","args":{"cmd":"cat a.txt"}}}]},{"role":"user","parts":[{"functionResponse":{"id":"` + id + `","name":"shell","response":{"output":"1 2 3"}}}]}`
		}
		path, body = "/v1beta/models/group/grok-codex:generateContent", `{"tools":[{"functionDeclarations":[{"name":"shell","parameters":{"type":"object","properties":{"cmd":{"type":"string"}}}}]}],"contents":[`+msgs+`]}`
	}
	rec := httptest.NewRecorder()
	s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", path, strings.NewReader(body)))
	if rec.Code == 200 {
		c.calls = append(c.calls, c.givenID(t, rec.Body.Bytes()))
	}
	return rec.Code, rec.Body.String()
}

// givenID is the id of the one call the reply gave the client.
func (c *carryClient) givenID(t *testing.T, body []byte) string {
	t.Helper()
	if c.from != provider.Gemini {
		return askedCallID(t, c.from, body, c.stream)
	}
	var m struct {
		Candidates []struct {
			Content struct {
				Parts []struct {
					FunctionCall *struct{ ID string } `json:"functionCall"`
				}
			}
		}
	}
	json.Unmarshal(body, &m)
	var ids []string
	for _, cand := range m.Candidates {
		for _, p := range cand.Content.Parts {
			if p.FunctionCall != nil {
				ids = append(ids, p.FunctionCall.ID)
			}
		}
	}
	if len(ids) != 1 {
		t.Fatalf("gemini client was given calls %v: %s", ids, body)
	}
	return ids[0]
}

// #4798 (zane668, ZanderH-code): a Chat, Messages or Gemini client's
// conversation moved from Grok to Codex, and back. The reasoning one of
// them sealed rides in its calls' ids (reasoning_carry.go) and reached
// the other, which refused it with a 400; nothing asked again, and every
// later turn carried the same item, so the session ended there. Now the
// refusal is asked again without what the calls carry, the turn is
// answered, the next turn there leaves the refused item out up front
// while the upstream's own goes along, and the calls, their arguments
// and their results go as the client wrote them, under their own ids.
func TestForeignCarriedReasoningAskedAgainWithout(t *testing.T) {
	for _, cl := range []carryClient{{from: provider.Chat}, {from: provider.Chat, stream: true}, {from: provider.Anthropic}, {from: provider.Anthropic, stream: true}, {from: provider.Gemini}} {
		t.Run(fmt.Sprintf("%s stream=%v", cl.from, cl.stream), func(t *testing.T) {
			fresh(t)
			grok := &carrySealer{name: "grok", refusal: xaiUndecryptable}
			codex := &carrySealer{name: "codex", refusal: codexUnverifiable}
			for _, u := range []struct {
				id   string
				name string
				h    http.Handler
			}{{"grok1", "Grok (SuperGrok)", grok}, {"codex1", "Codex", codex}} {
				up := httptest.NewServer(u.h)
				t.Cleanup(up.Close)
				if err := provider.Save(provider.Provider{ID: u.id, Name: u.name, Key: "k-" + u.id, Models: []string{"m"}, Responses: up.URL + "/v1"}); err != nil {
					t.Fatal(err)
				}
			}
			if err := provider.SaveGroup(provider.Group{Name: "grok-codex", Members: []string{"grok1/m", "codex1/m"}, Routing: provider.Ordered}); err != nil {
				t.Fatal(err)
			}
			s := New()
			c := &cl

			// Grok answers, and the call it gave carries its reasoning
			if code, body := c.ask(t, s); code != 200 {
				t.Fatalf("first turn: %d %s", code, body)
			}
			if _, items := unreasonedID(c.calls[0]); len(items) != 1 {
				t.Fatalf("the call's id %q carries %d items", c.calls[0], len(items))
			}
			grok.taken()

			// Grok's balance runs out: Codex takes the turn, refuses
			// Grok's reasoning once, and answers without it
			grok.mu.Lock()
			grok.down = http.StatusPaymentRequired
			grok.mu.Unlock()
			if code, body := c.ask(t, s); code != 200 {
				t.Fatalf("after the switch: %d %s", code, body)
			}
			seals, calls := codex.taken()
			if fmt.Sprint(seals) != "[[grok] []]" {
				t.Fatalf("Codex was sent reasoning %v, want Grok's, then none", seals)
			}
			for _, cl := range calls {
				if want := []string{"call_grok_1 {\"cmd\":\"cat a.txt\"}", "call_grok_1 -> 1 2 3"}; !strings.HasPrefix(cl, "call_grok_1") || cl != want[0] && cl != want[1] {
					t.Fatalf("Codex was sent the call as %q", cl)
				}
			}
			if len(calls) != 4 {
				t.Fatalf("Codex was sent calls %q", calls)
			}

			// the next turn there goes once, with Codex's own reasoning
			// and none of Grok's
			if code, body := c.ask(t, s); code != 200 {
				t.Fatalf("the turn after: %d %s", code, body)
			}
			if seals, _ := codex.taken(); fmt.Sprint(seals) != "[[codex]]" {
				t.Fatalf("Codex was then sent reasoning %v, want its own alone", seals)
			}

			// and the other way: Grok, back, is refused Codex's once and
			// asked again without what the calls carry — its own earlier
			// reasoning too, as a Responses client's request goes without
			// all of it (withoutReasoning) — and its next turn has its
			// newest reasoning, none of what it refused
			grok.mu.Lock()
			grok.down = 0
			grok.mu.Unlock()
			grok.taken()
			sticks.Lock()
			sticks.m = map[string]stick{}
			sticks.Unlock()
			restingUntil.Lock()
			restingUntil.m = map[string]time.Time{}
			restingUntil.Unlock()
			if code, body := c.ask(t, s); code != 200 || !strings.Contains(c.calls[len(c.calls)-1], "call_grok_") {
				t.Fatalf("back on Grok: %d %s (calls %q)", code, body, c.calls)
			}
			if seals, _ := grok.taken(); fmt.Sprint(seals) != "[[grok codex codex] []]" {
				t.Fatalf("Grok was sent reasoning %v, want its own and Codex's, then none", seals)
			}
			if code, body := c.ask(t, s); code != 200 {
				t.Fatalf("Grok's next turn: %d %s", code, body)
			}
			if seals, _ := grok.taken(); fmt.Sprint(seals) != "[[grok]]" {
				t.Fatalf("Grok was then sent reasoning %v, want its newest alone", seals)
			}
		})
	}
}
