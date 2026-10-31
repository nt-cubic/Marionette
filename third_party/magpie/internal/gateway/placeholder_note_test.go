package gateway

import (
	"bytes"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"regexp"
	"strings"
	"sync"
	"testing"

	"github.com/tidwall/gjson"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/redact"
	"github.com/yetone/magpie/internal/settings"
)

// The note goes to the vendor: it names neither magpie, nor a vendor, nor
// a client (LESSONS: text magpie adds to a prompt), and its example is no
// placeholder masking writes, so it is never put back as one.
func TestPlaceholderNoteIsNeutral(t *testing.T) {
	low := strings.ToLower(placeholderNote)
	for _, w := range []string{"magpie", "codex", "claude", "anthropic", "openai", "chatgpt", "gpt", "gemini", "google", "deepseek", "kimi", "qwen", "grok", "vendor", "provider", "gateway", "redact", "privacy", "proxy"} {
		if strings.Contains(low, w) {
			t.Errorf("the note names %q: %s", w, placeholderNote)
		}
	}
	ex := regexp.MustCompile(`\{\{[^}]*\}\}`).FindString(placeholderNote)
	if ex == "" {
		t.Fatal("the note shows no placeholder's form")
	}
	if regexp.MustCompile(`\{\{[A-Z][A-Z0-9_]*_[a-z2-7]{8}\}\}`).MatchString(ex) {
		t.Errorf("the note's example %s is a placeholder masking could write", ex)
	}
	if got := redact.Restore(placeholderNote, false); got != placeholderNote {
		t.Errorf("the note is changed when values are put back: %s", got)
	}
}

// Each API's body is told where its placeholders first appear: in the
// instructions when they or the tools hold one, else on the earliest
// message that does, after what it has, the bytes before it kept as the
// client wrote them; Responses' instructions are never touched.
func TestPlaceholderNotePlaces(t *testing.T) {
	const ph = "{{EMAIL_abcdefgh}}"
	cases := []struct {
		name  string
		from  provider.Protocol
		body  string
		where string // gjson path of the note's text
		keep  string // gjson path whose raw stays as it was
	}{
		{"anthropic system string", provider.Anthropic, `{"model":"m","system":"Mail ` + ph + `.","messages":[{"role":"user","content":"hi"}]}`, "system", "messages"},
		{"anthropic system blocks", provider.Anthropic, `{"model":"m","system":[{"type":"text","text":"a"},{"type":"text","text":"b ` + ph + `","cache_control":{"type":"ephemeral"}}],"messages":[]}`, "system.2.text", "system.1"},
		{"anthropic tools", provider.Anthropic, `{"model":"m","tools":[{"name":"t","description":"` + ph + `"}],"messages":[{"role":"user","content":"x"}]}`, "system", "messages"},
		{"anthropic message", provider.Anthropic, `{"model":"m","system":"S","messages":[{"role":"user","content":"hi"},{"role":"assistant","content":"ok"},{"role":"user","content":"mail ` + ph + `"}]}`, "messages.2.content.1.text", "messages.1"},
		{"anthropic tool result", provider.Anthropic, `{"model":"m","system":"S","messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"` + ph + `"}]}]}`, "messages.0.content.1.text", "system"},
		{"chat system", provider.Chat, `{"model":"m","messages":[{"role":"user","content":"u"},{"role":"system","content":"S ` + ph + `"}]}`, "messages.1.content", "messages.0"},
		{"chat developer parts", provider.Chat, `{"model":"m","messages":[{"role":"developer","content":[{"type":"text","text":"S ` + ph + `"}]},{"role":"user","content":"u"}]}`, "messages.0.content.1.text", "messages.1"},
		{"chat tools no system", provider.Chat, `{"model":"m","tools":[{"type":"function","function":{"name":"f","description":"` + ph + `"}}],"messages":[{"role":"user","content":"u"}]}`, "messages.0.content", "tools"},
		{"chat tool output", provider.Chat, `{"model":"m","messages":[{"role":"system","content":"S"},{"role":"user","content":"u"},{"role":"tool","tool_call_id":"c","content":"` + ph + `"}]}`, "messages.2.content", "messages.0"},
		{"responses instructions", provider.Responses, `{"model":"m","instructions":"You are a coding agent. ` + ph + `","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"u"}]}]}`, "input.0.content.0.text", "input.1"},
		{"responses item", provider.Responses, `{"model":"m","instructions":"I","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"u"}]},{"type":"function_call","call_id":"a","name":"f","arguments":"{}"},{"type":"function_call","call_id":"b","name":"f","arguments":"{}"},{"type":"function_call_output","call_id":"a","output":"` + ph + `"},{"type":"function_call_output","call_id":"b","output":"x"},{"type":"message","role":"user","content":[{"type":"input_text","text":"next"}]}]}`, "input.5.content.0.text", "input.4"},
		{"responses text", provider.Responses, `{"model":"m","instructions":"I","input":"hello ` + ph + `"}`, "input.0.content.0.text", "instructions"},
		{"gemini system", provider.Gemini, `{"systemInstruction":{"parts":[{"text":"S ` + ph + `"}]},"contents":[{"role":"user","parts":[{"text":"u"}]}]}`, "systemInstruction.parts.1.text", "contents"},
		{"gemini snake", provider.Gemini, `{"system_instruction":{"parts":[{"text":"S ` + ph + `"}]},"contents":[]}`, "system_instruction.parts.1.text", "contents"},
		{"gemini content", provider.Gemini, `{"systemInstruction":{"parts":[{"text":"S"}]},"contents":[{"role":"user","parts":[{"text":"u"}]},{"role":"model","parts":[{"text":"ok"}]},{"role":"user","parts":[{"text":"` + ph + `"}]}]}`, "contents.2.parts.1.text", "systemInstruction"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			out := withPlaceholderNote(c.from, []byte(c.body))
			if !json.Valid(out) {
				t.Fatalf("not JSON: %s", out)
			}
			if got := gjson.GetBytes(out, c.where).String(); !strings.HasSuffix(got, placeholderNote) {
				t.Fatalf("%s = %q in %s", c.where, got, out)
			}
			if strings.Count(string(out), placeholderNote) != 1 {
				t.Fatalf("said more than once: %s", out)
			}
			if before := gjson.Get(c.body, c.keep).Raw; before != "" && gjson.GetBytes(out, c.keep).Raw != before {
				t.Fatalf("%s changed: %s", c.keep, out)
			}
			if c.from == provider.Responses && gjson.GetBytes(out, "instructions").String() != gjson.Get(c.body, "instructions").String() {
				t.Fatalf("instructions changed: %s", out)
			}
			// the same body is told the same way every time
			if again := withPlaceholderNote(c.from, []byte(c.body)); !bytes.Equal(again, out) {
				t.Fatalf("not stable:\n%s\n%s", out, again)
			}
		})
	}
	// a string input stays the user's message, after the note
	out := withPlaceholderNote(provider.Responses, []byte(`{"model":"m","instructions":"I","input":"hello `+ph+`"}`))
	if gjson.GetBytes(out, "input.1.role").String() != "user" || gjson.GetBytes(out, "input.1.content").String() != "hello "+ph {
		t.Fatalf("input text lost: %s", out)
	}
	// with no placeholder, or none it can place, it is left as it was
	for _, b := range []string{`not json`, `[1]`, `{"model":"m","input":[{"type":"message","role":"user","content":"hi"}]}`, `{"model":"m"}`} {
		if out := withPlaceholderNote(provider.Responses, []byte(b)); string(out) != b {
			t.Errorf("%s became %s", b, out)
		}
	}
	plain := `{"model":"m","system":"S","messages":[{"role":"user","content":"Values like {{NAME_ab12cd34}}"}]}`
	if out := withPlaceholderNote(provider.Anthropic, []byte(plain)); string(out) != plain {
		t.Errorf("told with no placeholder masking writes: %s", out)
	}
}

// A Claude subscription (run through the Claude Code binary) and Codex's
// ChatGPT sign-in are never told; nor a Responses turn on a stored one.
func TestPlaceholderNoteNotOnClientPaths(t *testing.T) {
	plain := []byte(`{"input":[]}`)
	for _, agent := range []string{"claude", "codex"} {
		p := provider.Provider{ID: agent, Account: &provider.Account{Agent: agent}}
		for _, from := range []provider.Protocol{provider.Anthropic, provider.Chat, provider.Responses, provider.Gemini} {
			if notesPlaceholders(p, from, plain) {
				t.Errorf("%s on %s is told", agent, from)
			}
		}
	}
	key := provider.Provider{ID: "k", Key: "k"}
	if !notesPlaceholders(key, provider.Responses, plain) || !notesPlaceholders(key, provider.Anthropic, plain) {
		t.Error("a key's provider isn't told")
	}
	if notesPlaceholders(key, provider.Responses, []byte(`{"previous_response_id":"resp_1","input":[]}`)) {
		t.Error("a turn on a stored response is told")
	}
	if !notesPlaceholders(provider.Provider{ID: "c", Account: &provider.Account{Agent: "copilot"}}, provider.Chat, plain) {
		t.Error("another sign-in isn't told")
	}
}

// Through the gateway: a request masking changed is told, on every later
// turn the same way; one it didn't change isn't.
func TestMaskedRequestTold(t *testing.T) {
	const key = "sk-proj-abcdEFGH1234ijklMNOP5678qrst"
	var mu sync.Mutex
	var got []byte
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		got = b
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"msg_1","type":"message","role":"assistant","model":"m1","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":1}}`)
	}))
	defer up.Close()
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := provider.Save(provider.Provider{ID: "fake", Name: "Fake", Key: "k", Models: []string{"m1"}, Anthropic: up.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := settings.Save(settings.Settings{Redact: true}); err != nil {
		t.Fatal(err)
	}
	sent := func(req string) []byte {
		t.Helper()
		if code, body := post(t, "/v1/messages", req); code != 200 {
			t.Fatalf("%d %s", code, body)
		}
		mu.Lock()
		defer mu.Unlock()
		return got
	}
	turn1 := `{"model":"fake/m1","max_tokens":10,"system":[{"type":"text","text":"Be brief."}],"messages":[{"role":"user","content":"my key is ` + key + `"}]}`
	b1 := sent(turn1)
	if strings.Contains(string(b1), key) || !strings.Contains(string(b1), "{{API_KEY_") {
		t.Fatalf("not masked: %s", b1)
	}
	if gjson.GetBytes(b1, "messages.0.content.1.text").String() != noteBlock || gjson.GetBytes(b1, "system").Raw != `[{"type":"text","text":"Be brief."}]` {
		t.Fatalf("not told on the message that holds it: %s", b1)
	}
	// the next turn carries the value again, as the agent had it back
	turn2 := `{"model":"fake/m1","max_tokens":10,"system":[{"type":"text","text":"Be brief."}],"messages":[{"role":"user","content":"my key is ` + key + `"},{"role":"assistant","content":"ok"},{"role":"user","content":"go on"}]}`
	b2 := sent(turn2)
	if gjson.GetBytes(b2, "system").Raw != gjson.GetBytes(b1, "system").Raw || gjson.GetBytes(b2, "messages.0").Raw != gjson.GetBytes(b1, "messages.0").Raw {
		t.Fatalf("the prefix changed between turns:\n%s\n%s", b1, b2)
	}
	// nothing masked, nothing said
	b3 := sent(`{"model":"fake/m1","max_tokens":10,"system":"Be brief.","messages":[{"role":"user","content":"hello"}]}`)
	if strings.Contains(string(b3), placeholderNote) || gjson.GetBytes(b3, "system").String() != "Be brief." {
		t.Fatalf("told with nothing masked: %s", b3)
	}
}
