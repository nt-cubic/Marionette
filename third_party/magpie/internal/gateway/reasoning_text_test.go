package gateway

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// openaiStrict is a Responses upstream that checks reasoning as OpenAI does
// (#1411): an item with its text in it (content parts) is turned away with
// the 400 the ChatGPT backend answered one with (2026-10-09), naming the
// item. With takes set it takes the text, as a relay for a DeepSeek model
// does.
type openaiStrict struct {
	takes  bool
	mu     sync.Mutex
	inputs [][]map[string]any
}

func (u *openaiStrict) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	b, _ := io.ReadAll(r.Body)
	if strings.HasSuffix(r.URL.Path, "/models") {
		io.WriteString(w, `{"models":[{"slug":"gpt-6.1-sol","visibility":"list"}]}`)
		return
	}
	var q struct {
		Input []map[string]any `json:"input"`
	}
	json.Unmarshal(b, &q)
	u.mu.Lock()
	u.inputs = append(u.inputs, q.Input)
	u.mu.Unlock()
	for i, it := range q.Input {
		if c, _ := it["content"].([]any); !u.takes && it["type"] == "reasoning" && len(c) > 0 {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusBadRequest)
			fmt.Fprintf(w, `{"error":{"message":"Invalid 'input[%d].content': array too long. Expected an array with maximum length 0, but got an array with length %d instead.","type":"invalid_request_error","param":"input[%d].content","code":"array_above_max_length"}}`, i, len(c), i)
			return
		}
	}
	w.Header().Set("Content-Type", "text/event-stream")
	io.WriteString(w, sse(
		`data: {"type":"response.created","response":{"id":"resp_1","status":"in_progress"}}`,
		`data: {"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"m1","role":"assistant","content":[{"type":"output_text","text":"DONE"}]}}`,
		`data: {"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[],"usage":{"input_tokens":5,"output_tokens":2}}}`))
}

func (u *openaiStrict) asked() [][]map[string]any {
	u.mu.Lock()
	defer u.mu.Unlock()
	return append([][]map[string]any(nil), u.inputs...)
}

// deepseekTurn is a Codex conversation a routing group served on DeepSeek
// first: its reasoning item as #1411 shows it saved (the text and the
// ciphertext redacted as the reporter did), between the turn's call and
// its answer, and reasoning the destination sealed itself after it.
func deepseekTurn(model string) string {
	return `{"model":"` + model + `","stream":true,"store":false,"include":["reasoning.encrypted_content"],"input":[` +
		`{"type":"message","role":"user","content":[{"type":"input_text","text":"count the lines"}]},` +
		`{"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":"[redacted]"}],"encrypted_content":"[redacted]"},` +
		`{"type":"function_call","call_id":"call_1","name":"shell","arguments":"{\"cmd\":\"wc -l main.go\"}"},` +
		`{"type":"function_call_output","call_id":"call_1","output":"42 main.go"},` +
		`{"type":"message","role":"assistant","content":[{"type":"output_text","text":"42 lines"}]},` +
		`{"type":"reasoning","id":"rs_own","summary":[],"encrypted_content":"sealed-by-destination"},` +
		`{"type":"message","role":"user","content":[{"type":"input_text","text":"and now?"}]}]}`
}

// reasoningTexts is how many reasoning items in input carry their text.
func reasoningTexts(input []map[string]any) int {
	n := 0
	for _, it := range input {
		if c, _ := it["content"].([]any); it["type"] == "reasoning" && len(c) > 0 {
			n++
		}
	}
	return n
}

// #1411: a routing group's turn served on DeepSeek left Codex a reasoning
// item with its text and a seal; the next turn, routed to a relay in front
// of OpenAI (AgentRouter's gpt-6-luna), was turned away with "Invalid
// 'input[n].content': array too long", and so was every turn after. It is
// asked again without that item, everything else kept, and the relay isn't
// sent such items again.
func TestReasoningTextLeftOutWhereRefused(t *testing.T) {
	fresh(t)
	u := &openaiStrict{}
	srv := httptest.NewServer(u)
	t.Cleanup(srv.Close)
	if err := provider.Save(provider.Provider{ID: "ar", Name: "AgentRouter", Key: "k", Models: []string{"gpt-6-luna"}, Responses: srv.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "medium", Name: "Medium", Members: []string{"ar/gpt-6-luna"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	rec := codexPostTo(s, deepseekTurn("group/medium"))
	if rec.Code != 200 || !strings.Contains(rec.Body.String(), "DONE") && !strings.Contains(rec.Body.String(), "RE9ORQ") {
		t.Fatalf("%d %s", rec.Code, rec.Body)
	}
	got := u.asked()
	if len(got) != 2 || reasoningTexts(got[0]) != 1 {
		t.Fatalf("asked the relay %d times, want again once after its 400", len(got))
	}
	if reasoningTexts(got[1]) != 0 || strings.Join(reasoningIDs(got[1]), ",") != "rs_own" {
		t.Errorf("asked again with reasoning %v", got[1])
	}
	before, after := kinds(got[0]), kinds(got[1])
	for _, k := range []string{"message", "function_call", "function_call_output"} {
		if after[k] == 0 || after[k] != before[k] {
			t.Errorf("%s: %d asked again, %d first", k, after[k], before[k])
		}
	}

	// the next turn goes without it at once
	rec = codexPostTo(s, deepseekTurn("group/medium"))
	if got = u.asked(); rec.Code != 200 || len(got) != 3 || reasoningTexts(got[2]) != 0 {
		t.Fatalf("again: %d, %d asks", rec.Code, len(got))
	}
}

// A relay that takes the text (one for a DeepSeek model) is sent the item
// as Codex sent it, and asked once.
func TestReasoningTextKeptWhereTaken(t *testing.T) {
	fresh(t)
	u := &openaiStrict{takes: true}
	srv := httptest.NewServer(u)
	t.Cleanup(srv.Close)
	if err := provider.Save(provider.Provider{ID: "ar", Name: "AgentRouter", Key: "k", Models: []string{"gpt-6-luna"}, Responses: srv.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	rec := codexPostTo(New(), deepseekTurn("ar/gpt-6-luna"))
	if rec.Code != 200 {
		t.Fatalf("%d %s", rec.Code, rec.Body)
	}
	if got := u.asked(); len(got) != 1 || reasoningTexts(got[0]) != 1 {
		t.Fatalf("asked %d times: %v", len(got), got)
	}
}

// The group's next member is Codex's GPT on a ChatGPT sign-in, whose
// backend takes a reasoning item's content only empty: the item never goes
// there, and the backend is asked once.
func TestReasoningTextNotSentToCodexAccount(t *testing.T) {
	fresh(t)
	home := os.Getenv("HOME")
	claims := func(m map[string]any) string {
		b, _ := json.Marshal(m)
		return "h." + base64.RawURLEncoding.EncodeToString(b) + ".s"
	}
	os.MkdirAll(filepath.Join(home, ".codex"), 0o755)
	os.WriteFile(filepath.Join(home, ".codex", "auth.json"), mustJSON(map[string]any{"auth_mode": "chatgpt", "tokens": map[string]any{
		"id_token":      claims(map[string]any{"email": "me@example.com"}),
		"access_token":  claims(map[string]any{"exp": time.Now().Add(time.Hour).Unix()}),
		"refresh_token": "r", "account_id": "acct-1"}}), 0o600)
	provider.ForgetAccounts()
	t.Cleanup(provider.ForgetAccounts)
	u := &openaiStrict{}
	srv := httptest.NewServer(u)
	t.Cleanup(srv.Close)
	was := provider.CodexBase
	provider.CodexBase = srv.URL + "/backend-api/codex"
	t.Cleanup(func() { provider.CodexBase = was })
	if err := provider.SaveGroup(provider.Group{ID: "medium", Name: "Medium", Members: []string{"codex/gpt-6.1-sol"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	rec := httptest.NewRecorder()
	req := httptest.NewRequest("POST", "/v1/responses", strings.NewReader(deepseekTurn("group/medium")))
	req.Header.Set("session_id", "thread-1411")
	New().Handler().ServeHTTP(rec, req)
	if rec.Code != 200 {
		t.Fatalf("%d %s", rec.Code, rec.Body)
	}
	got := u.asked()
	if len(got) != 1 || reasoningTexts(got[0]) != 0 || kinds(got[0])["reasoning"] != 1 || kinds(got[0])["function_call_output"] != 1 {
		t.Fatalf("the ChatGPT backend was asked %d times: %v", len(got), got)
	}
}
