package gateway

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// Gemini's OpenAI-compatible API, in Linka's frames (Discord): a whole
// function call, its thought signature and the usage in one chunk, then
// the finish_reason in the next.
const (
	compatCall = `data: {"choices":[{"delta":{"role":"assistant","tool_calls":[{"extra_content":{"google":{"thought_signature":"c2ln"}},"function":{"arguments":"{\"code\":\"run()\"}","name":"codemode"},"id":"call_1","type":"function"}]},"index":0}],"model":"models/gemini-4-argon","object":"chat.completion.chunk","usage":{"completion_tokens":1110,"prompt_tokens":2049,"total_tokens":165667}}` + "\n\n"
	compatStop = `data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"model":"models/gemini-4-argon","object":"chat.completion.chunk","usage":{"completion_tokens":1110,"prompt_tokens":2049,"total_tokens":165667}}` + "\n\n"
	compatDone = "data: [DONE]\n\n"
)

// compatServer serves reply as a Chat upstream on this machine, where a
// Gemini model's is taken to be Gemini's OpenAI-compatible API
// (geminiCompat), and asks it for model at path with body.
func compatServer(t *testing.T, model, reply, path, body string) (int, string) {
	t.Helper()
	fresh(t)
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.Copy(io.Discard, r.Body)
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, reply)
	}))
	t.Cleanup(up.Close)
	if err := provider.Save(provider.Provider{ID: "up", Name: "UP", Key: "k", Models: []string{model}, Chat: up.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	return post(t, path, strings.ReplaceAll(body, "MODEL", "up/"+model))
}

// compatBare is compatCall without the usage: the generation wasn't
// counted as done.
var compatBare = strings.Replace(compatCall, `,"usage":{"completion_tokens":1110,"prompt_tokens":2049,"total_tokens":165667}`, "", 1)

// The upstream hung up between the call and its finish_reason. Gemini's
// OpenAI-compatible API counts the usage only once the generation is
// done, so a call chunk carrying it lost only the finish_reason: the
// client relayed to gets it, "tool_calls", where Pi threw the call away
// as "Stream ended without finish_reason". Without the usage the reply
// was cut, and the client is told so, as a Gemini stream's is (#1644).
func TestGeminiCompatRelayEndedShort(t *testing.T) {
	const ask = `{"model":"MODEL","stream":true,"stream_options":{"include_usage":true},"messages":[{"role":"user","content":"hi"}]}`
	const cut = `data: {"error":{"message":"UP: the reply ended before it was complete","type":"api_error"}}`

	code, body := compatServer(t, "gemini-4-argon", compatCall, "/v1/chat/completions", ask)
	if code != 200 || !strings.Contains(body, `"codemode"`) || strings.Contains(body, "ended before") {
		t.Fatalf("status %d: %s", code, body)
	}
	if !strings.Contains(body, `"finish_reason":"tool_calls"`) || !strings.HasSuffix(strings.TrimSpace(body), compatDone[:len(compatDone)-2]) {
		t.Errorf("a done Gemini reply that lost its finish_reason didn't end with it: %s", body)
	}

	// text alone, done, ends with "stop"
	text := `data: {"choices":[{"delta":{"role":"assistant","content":"hi"},"index":0}],"model":"models/gemini-4-argon","object":"chat.completion.chunk","usage":{"completion_tokens":1,"prompt_tokens":2049,"total_tokens":2050}}` + "\n\n"
	if _, body := compatServer(t, "gemini-4-argon", text, "/v1/chat/completions", ask); !strings.Contains(body, `"finish_reason":"stop"`) || strings.Contains(body, "ended before") {
		t.Errorf("done text: %s", body)
	}

	code, body = compatServer(t, "gemini-4-argon", compatBare, "/v1/chat/completions", ask)
	if code != 200 || !strings.Contains(body, `"codemode"`) {
		t.Fatalf("status %d: %s", code, body)
	}
	if !strings.HasSuffix(strings.TrimSpace(body), cut) {
		t.Errorf("a cut Gemini reply ended as if whole: %s", body)
	}
	if strings.Contains(body, `"finish_reason":"`) {
		t.Errorf("a cut reply given a finish_reason: %s", body)
	}

	// whole, with [DONE] or without it, it goes as it came, its one
	// finish_reason "tool_calls" as it made a call (chatTidy, 喜欢一个人
	// on Discord), never a second one added
	for what, reply := range map[string]string{
		"whole":            compatCall + compatStop + compatDone,
		"whole, no [DONE]": compatCall + compatStop,
	} {
		code, body := compatServer(t, "gemini-4-argon", reply, "/v1/chat/completions", ask)
		if code != 200 || strings.Contains(body, "ended before") || strings.Count(body, `"finish_reason":"`) != 1 || !strings.Contains(body, `"finish_reason":"tool_calls"`) {
			t.Errorf("%s: %d %s", what, code, body)
		}
	}

	// another vendor's Chat stream may end whole with neither: it goes as
	// it came, as it did
	for _, reply := range []string{compatCall, compatBare} {
		other := strings.ReplaceAll(reply, "models/gemini-4-argon", "kimi-k3")
		code, body = compatServer(t, "kimi-k3", other, "/v1/chat/completions", ask)
		if code != 200 || strings.Contains(body, "ended before") || strings.Contains(body, `"finish_reason":"`) {
			t.Errorf("another vendor's stream: %d %s", code, body)
		}
	}
}

// Translated, a call whose generation was done ends the turn with the
// call, for Claude Code to run; a cut one ends with the error, not with a
// turn that ended with the call never run.
func TestGeminiCompatTranslatedEndedShort(t *testing.T) {
	const ask = `{"model":"MODEL","max_tokens":32000,"stream":true,"messages":[{"role":"user","content":"hi"}]}`
	_, body := compatServer(t, "gemini-4-argon", compatCall, "/v1/messages", ask)
	if !strings.Contains(body, `"codemode"`) {
		t.Fatalf("the call didn't reach the client: %s", body)
	}
	if _, stops, errs := messageStops(body); errs != 0 || stops != 1 || !strings.Contains(body, `"stop_reason":"tool_use"`) {
		t.Errorf("a done Gemini call didn't end its turn as one (%d stops, %d errors): %s", stops, errs, body)
	}

	_, body = compatServer(t, "gemini-4-argon", compatBare, "/v1/messages", ask)
	if !strings.Contains(body, `"codemode"`) {
		t.Fatalf("the call didn't reach the client: %s", body)
	}
	if _, stops, errs := messageStops(body); errs != 1 || stops != 0 {
		t.Errorf("a cut Gemini reply ended as a turn (%d stops, %d errors): %s", stops, errs, body)
	}

	_, body = compatServer(t, "gemini-4-argon", compatCall+compatStop+compatDone, "/v1/messages", ask)
	if _, stops, errs := messageStops(body); errs != 0 || stops != 1 {
		t.Errorf("a whole Gemini reply (%d stops, %d errors): %s", stops, errs, body)
	}

	// another vendor's Chat stream ends as it did
	other := strings.ReplaceAll(compatBare, "models/gemini-4-argon", "kimi-k3")
	_, body = compatServer(t, "kimi-k3", other, "/v1/messages", ask)
	if _, _, errs := messageStops(body); errs != 0 {
		t.Errorf("another vendor's stream: %s", body)
	}

	// unstreamed: a done call is the answer, a cut reply a 502
	unstreamed := strings.Replace(ask, `"stream":true`, `"stream":false`, 1)
	code, body := compatServer(t, "gemini-4-argon", compatCall, "/v1/messages", unstreamed)
	if code != 200 || !strings.Contains(body, `"tool_use"`) || !strings.Contains(body, `"stop_reason":"tool_use"`) {
		t.Errorf("unstreamed done call: %d %s", code, body)
	}
	code, body = compatServer(t, "gemini-4-argon", compatBare, "/v1/messages", unstreamed)
	if code != 502 || !strings.Contains(body, "ended before") {
		t.Errorf("unstreamed cut: %d %s", code, body)
	}
}

// A Gemini reply that ended done but inside a line still gets its
// finish_reason as an event of its own, not as more of that line (#2446).
func TestGeminiCompatFinishAfterPartialLine(t *testing.T) {
	const ask = `{"model":"MODEL","stream":true,"stream_options":{"include_usage":true},"messages":[{"role":"user","content":"hi"}]}`
	_, body := compatServer(t, "gemini-4-argon", compatCall+`data: {"choi`, "/v1/chat/completions", ask)
	i := strings.Index(body, `"finish_reason":"tool_calls"`)
	if i < 0 {
		t.Fatalf("no finish_reason filled in: %s", body)
	}
	j := strings.LastIndex(body[:i], "data: ")
	if j < 0 || !strings.HasSuffix(body[:j], "\n\n") {
		t.Errorf("the finish_reason began inside the line still pending: %q", body)
	}
}
