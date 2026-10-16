package gateway

import (
	"encoding/json"
	"net/http/httptest"
	"strings"
	"testing"
)

// postMessages sends an Anthropic Messages request to the gateway, as dsh's
// DeepSeek provider and Claude Code do.
func postMessages(t *testing.T, body string) (int, string) {
	t.Helper()
	rec := httptest.NewRecorder()
	req := httptest.NewRequest("POST", "/v1/messages", strings.NewReader(body))
	New().Handler().ServeHTTP(rec, req)
	return rec.Code, rec.Body.String()
}

// An upstream that speaks Messages itself is relayed as-is; when it died
// mid-reply the relay just stopped, so the client saw a stream that ended
// cleanly with neither message_stop nor an error: dsh's "DeepSeek Messages
// stream ended before message_stop" (STREAM_CLOSED), which it does not
// retry and which says nothing of what went wrong (#370).
func TestPassthroughStreamSeveredUpstream(t *testing.T) {
	fresh(t)
	severed(t, anthropicStart, anthropicText)
	code, body := postMessages(t, `{"model":"up/m","stream":true,"max_tokens":8,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 {
		t.Fatalf("status = %d, want 200; body: %s", code, body)
	}
	if !strings.Contains(body, "event: error") || !strings.Contains(body, "UP:") {
		t.Fatalf("a severed stream ended without an error event naming the provider: %s", body)
	}
	if strings.Contains(body, "message_stop") {
		t.Fatalf("a severed stream was finished as if complete: %s", body)
	}
}

// One the upstream ended cleanly but short of its last event is as broken.
func TestPassthroughStreamEndsShort(t *testing.T) {
	fresh(t)
	f := &fake{reply: anthropicStart + anthropicText}
	setup(t, "anthropic", f)
	code, body := postMessages(t, `{"model":"fake/m1","stream":true,"max_tokens":8,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "event: error") {
		t.Fatalf("a stream cut short ended without an error event: %d %s", code, body)
	}
}

// The upstream's last write stops mid-line, its last event cut inside the
// data line. The failure event that ends the relay must begin after a
// blank line: appended straight after the partial line, its "event:" name
// lands inside that line, and the client reads one event whose data is the
// truncated half and the error joined — no error of the protocol's, and
// nothing to act on (#370's error event, unread).
func TestPassthroughStreamCutMidLine(t *testing.T) {
	fresh(t)
	f := &fake{reply: anthropicStart + anthropicText + "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_re"}
	setup(t, "anthropic", f)
	code, body := postMessages(t, `{"model":"fake/m1","stream":true,"max_tokens":8,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 {
		t.Fatalf("status = %d, want 200; body: %s", code, body)
	}
	atLineEnd(t, body)
}

// One cut exactly at a line's end but short of the blank line joins the
// same way: the error event's fields land inside the event still pending.
func TestPassthroughStreamEndsOnLineEnd(t *testing.T) {
	fresh(t)
	f := &fake{reply: anthropicStart + anthropicText + "event: message_delta\n"}
	setup(t, "anthropic", f)
	code, body := postMessages(t, `{"model":"fake/m1","stream":true,"max_tokens":8,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 {
		t.Fatalf("status = %d, want 200; body: %s", code, body)
	}
	atLineEnd(t, body)
}

// atLineEnd fails when the reply's error event doesn't begin after a blank
// line, where an SSE client reads it as an event of its own.
func atLineEnd(t *testing.T, body string) {
	t.Helper()
	i := strings.Index(body, "event: error")
	if i < 0 {
		t.Fatalf("the cut stream ended without an error event: %s", body)
	}
	if !strings.HasSuffix(body[:i], "\n\n") {
		t.Fatalf("the error event began inside the event still pending: %q", body[max(0, i-60):i+len("event: error")])
	}
}

// A reply the loop guard cuts (cutLoop) while the client is mid-line ends
// in the same joined event: what is appended after it must begin after a
// blank line too.
func TestLoopCutMidLine(t *testing.T) {
	fresh(t)
	run := strings.Repeat("mcp ", 1024) // 4096 chars, no break in it: one run-on unit
	delta := func(txt string) string {
		b, _ := json.Marshal(map[string]any{"type": "content_block_delta", "index": 0,
			"delta": map[string]any{"type": "text_delta", "text": txt}})
		return "event: content_block_delta\ndata: " + string(b) + "\n\n"
	}
	// the second run-on unit trips the guard (loop_guard.go: two units cut
	// at 4096 are a loop); the line after it stops mid-line, and the write
	// that trips ends there
	f := &fake{reply: anthropicStart + delta(run) + delta(run) + "event: content_block_delta\ndata: {\"type\":\"te"}
	setup(t, "anthropic", f)
	code, body := postMessages(t, `{"model":"fake/m1","stream":true,"max_tokens":8,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 {
		t.Fatalf("status = %d, want 200; body: %s", code, body)
	}
	if !strings.Contains(body, "stuck in a loop") {
		t.Fatalf("the run-on reply was not cut by the loop guard: %s", body[:min(600, len(body))])
	}
	// the upstream's own last line is the only event the client may read
	// unparseable: a join of two halves is magpie's doing
	joined := 0
	readSSE(strings.NewReader(body), func(_, data string) error {
		if strings.Contains(data, "\n") && json.Valid([]byte(data)) == false {
			joined++
		}
		return nil
	})
	if joined != 0 {
		t.Fatalf("an appended event was joined onto the line still pending: %s", body[max(0, len(body)-400):])
	}
}

// A whole stream goes through untouched, nothing added after its end.
func TestPassthroughStreamWhole(t *testing.T) {
	fresh(t)
	f := &fake{reply: anthropicStart + anthropicText + anthropicStop}
	setup(t, "anthropic", f)
	code, body := postMessages(t, `{"model":"fake/m1","stream":true,"max_tokens":8,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || body != anthropicStart+anthropicText+anthropicStop {
		t.Fatalf("a whole stream was changed: %d %q", code, body)
	}
}

// A Responses stream cut short fails as Codex reads a failed turn.
func TestPassthroughResponsesEndsShort(t *testing.T) {
	fresh(t)
	f := &fake{reply: sse(`event: response.created`+"\n"+`data: {"type":"response.created","response":{"id":"r1","model":"m1"}}`,
		`event: response.output_text.delta`+"\n"+`data: {"type":"response.output_text.delta","delta":"hel"}`)}
	setup(t, "responses", f)
	code, body := post(t, "/v1/responses", `{"model":"fake/m1","stream":true,"input":"hi"}`)
	if code != 200 || !strings.Contains(body, `"type":"response.failed"`) || !strings.Contains(body, "Fake:") {
		t.Fatalf("a Responses stream cut short did not fail: %d %s", code, body)
	}
}

// A whole stream whose last event is too big for the sniffer to read (an
// image's base64 in response.completed) is still whole: nothing is added.
func TestPassthroughResponsesBigLastEvent(t *testing.T) {
	fresh(t)
	big := strings.Repeat("A", 2<<20)
	reply := sse(`event: response.created`+"\n"+`data: {"type":"response.created","response":{"id":"r1","model":"m1"}}`,
		`event: response.completed`+"\n"+`data: {"type":"response.completed","response":{"id":"r1","output":[{"type":"image_generation_call","result":"`+big+`"}]}}`)
	f := &fake{reply: reply}
	setup(t, "responses", f)
	code, body := post(t, "/v1/responses", `{"model":"fake/m1","stream":true,"input":"hi"}`)
	if code != 200 || body != reply {
		t.Fatalf("a whole stream with a big last event was changed: %d, %d bytes added", code, len(body)-len(reply))
	}
}
