package gateway

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// nativeSealedReply is a Responses stream whose output is one compaction item,
// sealed by the vendor as enc.
func nativeSealedReply(enc string) string {
	item := `{"type":"compaction","encrypted_content":"` + enc + `"}`
	return sse(
		`data: {"type":"response.created","response":{"id":"resp_c1","status":"in_progress","output":[]}}`,
		`data: {"type":"response.output_item.done","output_index":0,"item":`+item+`}`,
		`data: {"type":"response.completed","response":{"id":"resp_c1","status":"completed","output":[`+item+`],"usage":{"input_tokens":9,"output_tokens":2,"total_tokens":11}}}`)
}

func nativeOn(t *testing.T, id string) {
	t.Helper()
	if err := provider.SetNativeCompaction(id, true); err != nil {
		t.Fatal(err)
	}
}

// Codex offers web_search on every turn, which has a turn to a provider
// that doesn't search translated: a compaction is relayed all the same
const compactTurn = `{"model":"%s","stream":true,"instructions":"You are Codex","tools":[{"type":"web_search"}],"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"fix the bug"}]},{"type":"compaction_trigger"}]}`

func nativeAsk(t *testing.T, model, body string) (int, string) {
	t.Helper()
	return codexPost(t, strings.Replace(body, "%s", model, 1))
}

// #1099: a provider set to compact natively is asked Codex's
// compaction_trigger as Codex sent it, and the compaction it seals goes
// back to Codex unchanged; the next turn carries it to the same provider
// as it is. One not set to keeps magpie's summary (magpie1:).
func TestNativeCompactionPassesTheTrigger(t *testing.T) {
	f := &fake{t: t, reply: nativeSealedReply("gAAAAB-sealed-by-relay")}
	setup(t, provider.Responses, f)
	chatgpt(t, func(w http.ResponseWriter, r *http.Request) {
		t.Error("a relay's compaction reached OpenAI")
		w.WriteHeader(400)
	})

	// off: magpie summarises, the trigger never reaches the relay
	f.reply = sse(
		`data: {"type":"response.output_item.done","output_index":0,"item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"SUMMARY"}]}}`,
		`data: {"type":"response.completed","response":{"id":"resp_s","status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}`)
	code, body := nativeAsk(t, "fake/m1", compactTurn)
	if code != 200 || strings.Contains(string(f.got), "compaction_trigger") || !strings.Contains(body, magpieCompaction) {
		t.Fatalf("off: %d %s\nrelay got %s", code, body, f.got)
	}

	// on
	nativeOn(t, "fake")
	f.reply = nativeSealedReply("gAAAAB-sealed-by-relay")
	code, body = nativeAsk(t, "fake/m1", compactTurn)
	if code != 200 {
		t.Fatalf("on: %d %s", code, body)
	}
	if !strings.Contains(string(f.got), `{"type":"compaction_trigger"}`) || strings.Contains(string(f.got), "CONTEXT CHECKPOINT COMPACTION") {
		t.Errorf("the relay wasn't asked Codex's own compaction: %s", f.got)
	}
	if f.path != "/v1/responses" {
		t.Errorf("asked at %s", f.path)
	}
	if !strings.Contains(body, `"encrypted_content":"gAAAAB-sealed-by-relay"`) || strings.Contains(body, magpieCompaction) {
		t.Errorf("Codex got %s", body)
	}

	// the next turn takes the sealed compaction to the relay as it is
	f.reply = sse(`data: {"type":"response.completed","response":{"id":"resp_n","status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}`)
	code, body = codexPost(t, `{"model":"fake/m1","stream":true,"tools":[{"type":"web_search"}],"input":[{"type":"compaction","encrypted_content":"gAAAAB-sealed-by-relay"},{"type":"message","role":"user","content":[{"type":"input_text","text":"go on"}]}]}`)
	if code != 200 || !strings.Contains(string(f.got), `"encrypted_content":"gAAAAB-sealed-by-relay"`) {
		t.Errorf("next turn: %d %s\nrelay got %s", code, body, f.got)
	}
}

// A relay that refuses compaction_trigger has its own error go back to
// Codex, not a summary of magpie's in its place.
func TestNativeCompactionRefusalGoesBack(t *testing.T) {
	f := &fake{t: t}
	f.refuse = func(body []byte) (int, string) {
		if strings.Contains(string(body), "compaction_trigger") {
			return 400, `{"error":{"message":"Invalid value: 'compaction_trigger'. Supported values are: 'message', 'reasoning'.","type":"invalid_request_error","param":"input[1].type"}}`
		}
		return 0, ""
	}
	setup(t, provider.Responses, f)
	chatgpt(t, func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(400) })
	nativeOn(t, "fake")
	code, body := nativeAsk(t, "fake/m1", compactTurn)
	if code != 400 || !strings.Contains(body, "Invalid value: 'compaction_trigger'") || strings.Contains(body, magpieCompaction) {
		t.Fatalf("%d %s", code, body)
	}
	if f.calls != 1 {
		t.Errorf("asked %d times", f.calls)
	}
}

// A compaction one key sealed never goes to another provider, which would
// refuse it, nor goes without it: the request is answered an error saying
// why. The writer, asked again, still reads it.
func TestNativeCompactionStaysWithItsWriter(t *testing.T) {
	f := &fake{t: t, reply: nativeSealedReply("gAAAAB-sealed-by-a")}
	setup(t, provider.Responses, f)
	chatgpt(t, func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(400) })
	g := &fake{t: t, reply: sse(`data: {"type":"response.completed","response":{"id":"resp_b","status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}`)}
	up := httptest.NewServer(g)
	t.Cleanup(up.Close)
	if err := provider.Save(provider.Provider{ID: "other", Name: "Other", Key: "k2", Models: []string{"m1"}, Responses: up.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	nativeOn(t, "fake")
	nativeOn(t, "other")
	if code, body := nativeAsk(t, "fake/m1", compactTurn); code != 200 || !strings.Contains(body, "gAAAAB-sealed-by-a") {
		t.Fatalf("compact: %d %s", code, body)
	}
	next := `{"model":"%s","stream":true,"input":[{"type":"compaction","encrypted_content":"gAAAAB-sealed-by-a"},{"type":"message","role":"user","content":[{"type":"input_text","text":"go on"}]}]}`
	code, body := nativeAsk(t, "other/m1", next)
	if code != 400 || !strings.Contains(body, "compacted natively") || !strings.Contains(body, "fake#") {
		t.Fatalf("another provider: %d %s", code, body)
	}
	if g.calls != 0 {
		t.Errorf("the other provider was sent it: %s", g.got)
	}
	f.reply = g.reply
	if code, body := nativeAsk(t, "fake/m1", next); code != 200 || !strings.Contains(string(f.got), "gAAAAB-sealed-by-a") {
		t.Errorf("the writer: %d %s", code, body)
	}
}

// GitHub Copilot's GPT models answer on Responses, so a Copilot account
// set to compact natively is asked Codex's compaction as it was sent; its
// Claude models, on chat completions, keep magpie's summary.
func TestNativeCompactionCopilot(t *testing.T) {
	var mu sync.Mutex
	var asked []string
	copilotFake(t, "gho_compact1099", func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		defer mu.Unlock()
		switch r.URL.Path {
		case "/models":
			io.WriteString(w, `{"data":[
			  {"id":"gpt-5.3-codex","name":"GPT-5.3-Codex","model_picker_enabled":true,"policy":{"state":"enabled"},"supported_endpoints":["/responses"],"capabilities":{"type":"chat"}},
			  {"id":"claude-sonnet-4.5","name":"Claude Sonnet 4.5","model_picker_enabled":true,"policy":{"state":"enabled"},"supported_endpoints":["/chat/completions"],"capabilities":{"type":"chat"}}]}`)
		case "/responses":
			asked = append(asked, "/responses "+string(b))
			w.Header().Set("Content-Type", "text/event-stream")
			io.WriteString(w, nativeSealedReply("gAAAAB-sealed-by-copilot"))
		case "/chat/completions":
			asked = append(asked, "/chat/completions "+string(b))
			w.Header().Set("Content-Type", "text/event-stream")
			io.WriteString(w, "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"SUMMARY\"}}]}\n\ndata: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n")
		default:
			w.WriteHeader(404)
		}
	})
	chatgpt(t, func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(400) })
	p, err := provider.Find("copilot")
	if err != nil || !p.CarriesCompaction() {
		t.Fatalf("copilot: %v %+v", err, p)
	}
	nativeOn(t, "copilot")
	if p, _ := provider.Find("copilot"); !p.NativeCompaction {
		t.Fatal("a Copilot account didn't keep native compaction on")
	}
	code, body := nativeAsk(t, "copilot/gpt-5.3-codex", compactTurn)
	if code != 200 || !strings.Contains(body, `"encrypted_content":"gAAAAB-sealed-by-copilot"`) {
		t.Fatalf("gpt: %d %s", code, body)
	}
	mu.Lock()
	if len(asked) != 1 || !strings.HasPrefix(asked[0], "/responses ") || !strings.Contains(asked[0], `{"type":"compaction_trigger"}`) {
		t.Errorf("Copilot was asked %q", asked)
	}
	asked = nil
	mu.Unlock()

	code, body = nativeAsk(t, "copilot/claude-sonnet-4.5", compactTurn)
	if code != 200 || !strings.Contains(body, magpieCompaction) {
		t.Fatalf("claude: %d %s", code, body)
	}
	mu.Lock()
	defer mu.Unlock()
	for _, a := range asked {
		if strings.Contains(a, "compaction_trigger") {
			t.Errorf("Claude on Copilot was asked the trigger: %s", a)
		}
	}
}

// #2271 (JasonBuildAI): a native compaction its provider couldn't answer,
// sealed by the fallback it went to, whose own native compaction is off,
// is that fallback's to read: it is recorded as the writer, the next turn
// carrying it goes there as it is, and a provider that can't read it is
// never sent it, nor asked without it, but answered why.
func TestNativeCompactionFallbackSealStaysWithItsWriter(t *testing.T) {
	noRests()
	t.Cleanup(noRests)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	chatgpt(t, func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(400) })
	done := sse(`data: {"type":"response.completed","response":{"id":"resp_n","status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}`)
	fa := &fake{t: t, reply: done}
	fa.refuse = func(body []byte) (int, string) {
		if strings.Contains(string(body), "compaction_trigger") {
			return 500, `{"error":{"message":"upstream down"}}`
		}
		return 0, ""
	}
	fb := &fake{t: t, reply: nativeSealedReply("gAAAAB-sealed-by-fb")}
	other := &fake{t: t, reply: done}
	for _, x := range []struct {
		p provider.Provider
		f *fake
	}{
		{provider.Provider{ID: "fa", Name: "FA", Key: "k", Models: []string{"m1"}, Fallback: []string{"fb/m1"}}, fa},
		{provider.Provider{ID: "fb", Name: "FB", Key: "k", Models: []string{"m1"}}, fb},
		{provider.Provider{ID: "other", Name: "Other", Key: "k", Models: []string{"m1"}}, other},
	} {
		up := httptest.NewServer(x.f)
		t.Cleanup(up.Close)
		x.p.Responses = up.URL + "/v1"
		if err := provider.Save(x.p); err != nil {
			t.Fatal(err)
		}
	}
	nativeOn(t, "fa") // fb and other are not

	code, body := nativeAsk(t, "fa/m1", compactTurn)
	if code != 200 || !strings.Contains(body, "gAAAAB-sealed-by-fb") || !strings.Contains(string(fb.got), "compaction_trigger") {
		t.Fatalf("compact: %d %s\nfb got %s", code, body, fb.got)
	}

	// as Codex sends it, offering web_search, which has a turn to a
	// provider that doesn't search translated
	next := `{"model":"%s","stream":true,"tools":[{"type":"web_search"}],"input":[{"type":"compaction","encrypted_content":"gAAAAB-sealed-by-fb"},{"type":"message","role":"user","content":[{"type":"input_text","text":"go on"}]}]}`
	code, body = nativeAsk(t, "other/m1", next)
	if code != 400 || !strings.Contains(body, "compacted natively") || !strings.Contains(body, "fb#") {
		t.Fatalf("another provider: %d %s\nother got %s", code, body, other.got)
	}
	if strings.Contains(body, "native compaction is on") {
		t.Errorf("says native compaction is on for its writer, whose isn't: %s", body)
	}
	if other.calls != 0 {
		t.Errorf("the other provider was asked: %s", other.got)
	}

	fb.reply = done
	fa.got, fb.got = nil, nil
	code, body = nativeAsk(t, "fa/m1", next)
	if code != 200 || !strings.Contains(string(fb.got), `"encrypted_content":"gAAAAB-sealed-by-fb"`) {
		t.Errorf("the writer: %d %s\nfb got %s", code, body, fb.got)
	}
	if strings.Contains(string(fa.got), "gAAAAB-sealed-by-fb") {
		t.Errorf("fa, which can't read it, was sent it: %s", fa.got)
	}
}
