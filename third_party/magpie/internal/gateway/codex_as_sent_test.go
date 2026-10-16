package gateway

import (
	"encoding/json"
	"net/http/httptest"
	"reflect"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// asSentRequest is a Responses Lite request with what a relay checking
// Codex's own shape looks at (#1758): the additional_tools item, the
// functions namespace, Codex's stream_options and its turn metadata.
var asSentRequest = strings.Replace(liteRequest, `"stream":true,`,
	`"stream":true,"stream_options":{"reasoning_summary_delivery":"inline"},"client_metadata":{"x-codex-turn-metadata":"{\"turn_id\":\"t1\"}"},`, 1)

// A relay set to take Codex's requests as sent (codex=as-sent) gets the
// request as Codex sent it, its model name alone swapped, and Codex's turn
// metadata header (#1758: lifting additional_tools into tools was answered
// 400 "Codex 客户端请求特征无效", 10014). Off, the default, the request is
// still fitted as before (#350).
func TestCodexAsSent(t *testing.T) {
	f := &fake{t: t, reply: sse(
		`data: {"type":"response.created","response":{"id":"r1","model":"m1"}}`,
		`data: {"type":"response.completed","response":{"id":"r1","output":[],"usage":{"input_tokens":5,"output_tokens":2}}}`)}
	setup(t, provider.Responses, f)
	ask := func() {
		t.Helper()
		rec := httptest.NewRecorder()
		req := httptest.NewRequest("POST", "/v1/responses", strings.NewReader(asSentRequest))
		req.Header.Set("User-Agent", "codex_cli_rs/0.162.0-alpha.17.2 (Mac OS 26.2.0; arm64)")
		req.Header.Set("originator", "codex_cli_rs")
		req.Header.Set("X-Codex-Turn-Metadata", `{"turn_id":"t1"}`)
		req.Header.Set("X-OpenAI-Internal-Codex-Responses-Lite", "true")
		New().Handler().ServeHTTP(rec, req)
		if rec.Code != 200 {
			t.Fatalf("status %d: %s", rec.Code, rec.Body)
		}
	}
	var sent map[string]any
	if err := json.Unmarshal([]byte(asSentRequest), &sent); err != nil {
		t.Fatal(err)
	}
	sent["model"] = "m1"

	ask()
	if strings.Contains(string(f.got), `"additional_tools"`) || strings.Contains(string(f.got), "reasoning_summary_delivery") {
		t.Fatalf("by default the request is fitted as before: %s", f.got)
	}

	p, err := provider.Find("fake")
	if err != nil {
		t.Fatal(err)
	}
	p.CodexAsSent = true
	if err := provider.Save(*p); err != nil {
		t.Fatal(err)
	}
	ask()
	var got map[string]any
	if err := json.Unmarshal(f.got, &got); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(got, sent) {
		t.Fatalf("as sent, the relay got\n%s\nwant Codex's request with model m1", f.got)
	}
	for _, h := range []string{"X-Codex-Turn-Metadata", "X-Openai-Internal-Codex-Responses-Lite"} {
		if f.head.Get(h) == "" {
			t.Errorf("%s not sent on: %v", h, f.head)
		}
	}
}
