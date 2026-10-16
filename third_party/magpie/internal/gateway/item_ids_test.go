package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// museReasoningID is the reasoning item id Muse Spark on OpenCode handed
// Codex, as echo_ts's error quotes it: two ids joined with a colon.
const museReasoningID = "rs_6aca2fc7533c979d5f0e4f7a:rs_01a125caa544754e9a3c70080100b009"

// museConversation is echo_ts's conversation after the switch: a user
// turn, Muse's reasoning at input[1] with its id and its sealed content,
// a call Muse made (its item id joined the same way) and its output, and
// Muse's answer.
const museConversation = `{"type":"message","role":"user","content":[{"type":"input_text","text":"fix the login bug"}]},
  {"type":"reasoning","id":"` + museReasoningID + `","summary":[],"encrypted_content":"muse-sealed"},
  {"type":"function_call","id":"fc_6aca2fc7533c979d5f0e4f7b:fc_01a125caa544754e9a3c70080100b00a","call_id":"call_muse_1","name":"shell","arguments":"{\"cmd\":\"ls\"}"},
  {"type":"function_call_output","call_id":"call_muse_1","output":"main.go"},
  {"type":"message","id":"msg_6aca2fc7533c979d5f0e4f7c:msg_01a125caa544754e9a3c70080100b00b","role":"assistant","content":[{"type":"output_text","text":"I fixed main.go"}]}`

// openaiItemID is the id OpenAI takes on an input item.
var openaiItemID = regexp.MustCompile(`^[A-Za-z0-9_-]+$`)

// idChecker plays OpenAI's /responses as Copilot passes it on: an input
// item whose id has another character is refused with OpenAI's 400, word
// for word as echo_ts got it (Copilot's name is magpie's, before it).
// With check off it takes any id, as OpenCode does its own.
type idChecker struct {
	mu     sync.Mutex
	check  bool
	inputs [][]map[string]any
}

func (u *idChecker) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	body, _ := io.ReadAll(r.Body)
	var q struct {
		Input []map[string]any `json:"input"`
	}
	json.Unmarshal(body, &q)
	u.mu.Lock()
	u.inputs = append(u.inputs, q.Input)
	u.mu.Unlock()
	for i, it := range q.Input {
		id, ok := it["id"].(string)
		if u.check && ok && !openaiItemID.MatchString(id) {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusBadRequest)
			io.WriteString(w, `{"error":{"message":"Invalid 'input[`+mustString(int64(i))+`].id': '`+id+`'. Expected an ID that contains letters, numbers, underscores, or dashes, but this value contained additional characters.","type":"invalid_request_error","param":"input[`+mustString(int64(i))+`].id","code":"invalid_value"}}`)
			return
		}
	}
	w.Header().Set("Content-Type", "text/event-stream")
	io.WriteString(w, sse(
		`data: {"type":"response.created","response":{"id":"resp_1","status":"in_progress"}}`,
		`data: {"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"DONE"}]}}`,
		`data: {"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[],"usage":{"input_tokens":5,"output_tokens":1}}}`))
}

func (u *idChecker) asked() [][]map[string]any {
	u.mu.Lock()
	defer u.mu.Unlock()
	return append([][]map[string]any(nil), u.inputs...)
}

// checkSent fails unless input is museConversation as an id checker takes
// it: Muse's reasoning gone, every other item there with an id of
// [A-Za-z0-9_-], and the call still paired with its output.
func checkSent(t *testing.T, input []map[string]any) {
	t.Helper()
	var types []string
	for _, it := range input {
		types = append(types, it["type"].(string))
		if id, ok := it["id"].(string); ok && !openaiItemID.MatchString(id) {
			t.Errorf("item id %q sent", id)
		}
		if it["type"] == "reasoning" {
			t.Errorf("Muse's reasoning sent: %v", it)
		}
	}
	if got := strings.Join(types, ","); !strings.HasPrefix(got, "message,function_call,function_call_output,message") {
		t.Errorf("items sent: %s", got)
	}
	b, _ := json.Marshal(input)
	for _, want := range []string{`"fc_6aca2fc7533c979d5f0e4f7b_fc_01a125caa544754e9a3c70080100b00a"`, `"call_muse_1"`, "I fixed main.go", "fix the login bug"} {
		if !strings.Contains(string(b), want) {
			t.Errorf("input lacks %s: %s", want, b)
		}
	}
}

// echo_ts, in Codex: a conversation on Muse Spark (OpenCode), switched to
// GPT on GitHub Copilot. Muse's items, ids joined with a colon, went to
// Copilot as Codex sent them, and every turn was turned away "Invalid
// 'input[1].id'". Copilot checks ids as OpenAI does, so they go as it
// takes them from the first ask.
func TestCopilotTakesAnotherVendorsItemIDs(t *testing.T) {
	fresh(t)
	cfg := os.Getenv("XDG_CONFIG_HOME")
	os.MkdirAll(filepath.Join(cfg, "github-copilot"), 0o755)
	os.WriteFile(filepath.Join(cfg, "github-copilot", "apps.json"), mustJSON(map[string]any{
		"github.com:Iv1.x": map[string]any{"user": "echo", "oauth_token": "gho_echo"},
	}), 0o600)
	up := &idChecker{check: true}
	api := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/models":
			io.WriteString(w, `{"data":[{"id":"gpt-6.1","name":"GPT-6.1","model_picker_enabled":true,"policy":{"state":"enabled"},"supported_endpoints":["/responses"],"capabilities":{"type":"chat","supports":{"reasoning_effort":["low","medium","high"]}}}]}`)
		case "/responses":
			up.ServeHTTP(w, r)
		default:
			w.WriteHeader(404)
		}
	}))
	defer api.Close()
	gh := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.HasSuffix(r.URL.Path, "/token") {
			json.NewEncoder(w).Encode(map[string]any{"token": "sess", "expires_at": time.Now().Add(time.Hour).Unix(), "endpoints": map[string]string{"api": api.URL}})
			return
		}
		io.WriteString(w, `{"copilot_plan":"individual"}`)
	}))
	defer gh.Close()
	oldTok, oldUser := provider.CopilotTokenURL, provider.CopilotUserURL
	provider.CopilotTokenURL, provider.CopilotUserURL = gh.URL+"/token", gh.URL+"/user"
	defer func() { provider.CopilotTokenURL, provider.CopilotUserURL = oldTok, oldUser }()

	s := New()
	rec := httptest.NewRecorder()
	req := httptest.NewRequest("POST", "/v1/responses", strings.NewReader(`{"model":"copilot/gpt-6.1","stream":true,"store":false,"include":["reasoning.encrypted_content"],
	  "input":[`+museConversation+`,{"type":"message","role":"user","content":[{"type":"input_text","text":"now add a test"}]}]}`))
	s.Handler().ServeHTTP(rec, req)
	if rec.Code != 200 || !strings.Contains(rec.Body.String(), "DONE") {
		t.Fatalf("%d %s", rec.Code, rec.Body.String())
	}
	asked := up.asked()
	if len(asked) != 1 {
		t.Fatalf("asked Copilot %d times", len(asked))
	}
	checkSent(t, asked[0])
}

// A relay in front of OpenAI, which magpie can't tell from any other
// upstream, refuses the ids as OpenAI does: asked once more with ids it
// takes, and from then on asked that way at once.
func TestRelayRefusingItemIDsAskedAgain(t *testing.T) {
	fresh(t)
	up := &idChecker{check: true}
	srv := httptest.NewServer(up)
	t.Cleanup(srv.Close)
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "k", Models: []string{"gpt-6.1"}, Responses: srv.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	s := New()
	for turn := range 2 {
		rec := codexPostTo(s, `{"model":"relay/gpt-6.1","stream":true,"store":false,"input":[`+museConversation+`]}`)
		if rec.Code != 200 || !strings.Contains(rec.Body.String(), "DONE") {
			t.Fatalf("turn %d: %d %s", turn, rec.Code, rec.Body.String())
		}
	}
	asked := up.asked()
	if len(asked) != 3 {
		t.Fatalf("asked the relay %d times, want 2 for the first turn and 1 for the second", len(asked))
	}
	checkSent(t, asked[1])
	checkSent(t, asked[2])
}

// OpenCode, where Muse made them, gets Muse's items as Codex sent them:
// its reasoning, id and seal, is its own to read back.
func TestMuseKeepsItsOwnItemIDs(t *testing.T) {
	fresh(t)
	up := &idChecker{}
	srv := httptest.NewServer(up)
	t.Cleanup(srv.Close)
	if err := provider.Save(provider.Provider{ID: "zen", Name: "OpenCode", Key: "k", Models: []string{"muse-spark-1.3-contributor"}, Responses: srv.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	rec := codexPostTo(New(), `{"model":"zen/muse-spark-1.3-contributor","stream":true,"store":false,"input":[`+museConversation+`]}`)
	if rec.Code != 200 {
		t.Fatalf("%d %s", rec.Code, rec.Body.String())
	}
	asked := up.asked()
	if len(asked) != 1 {
		t.Fatalf("asked %d times", len(asked))
	}
	b, _ := json.Marshal(asked[0])
	if !strings.Contains(string(b), `"`+museReasoningID+`"`) || !strings.Contains(string(b), "muse-sealed") {
		t.Errorf("Muse's reasoning not sent as it was: %s", b)
	}
}

// Back on one of Codex's own models, on the ChatGPT backend, which checks
// ids as OpenAI does.
func TestCodexOwnModelTakesAnotherVendorsItemIDs(t *testing.T) {
	setup(t, provider.Chat, &fake{t: t})
	up := &idChecker{check: true}
	chatgpt(t, up.ServeHTTP)
	code, body := codexPost(t, `{"model":"gpt-6-luna","stream":true,"store":false,"input":[`+museConversation+`]}`)
	if code != 200 || !strings.Contains(body, "DONE") {
		t.Fatalf("%d %s", code, body)
	}
	asked := up.asked()
	if len(asked) != 1 {
		t.Fatalf("asked the backend %d times", len(asked))
	}
	checkSent(t, asked[0])
}
