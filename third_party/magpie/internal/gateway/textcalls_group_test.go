package gateway

import (
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// 喜欢一个人 on Discord called the model through a routing group of several
// of its entries, in order, from Alma, and only the group went wrong. A
// group's reply is mended as a model's own is: by the member that answers,
// the first or one after a refusal, on every route, with nothing the
// refused try sent before it.
func TestDSMLThroughGroup(t *testing.T) {
	native := sse(nativeExec, chunkOf(reportedClose[:6]), chunkOf(reportedClose[6:]), stopChunk, `data: [DONE]`)
	written := sse(chunkOf("Let me look.\n\n"+reportedBlock[:40]), chunkOf(reportedBlock[40:]), stopChunk, `data: [DONE]`)
	refused := reply{500, "application/json", `{"error":{"message":"upstream overloaded"}}`}
	for _, up := range []struct{ name, body string }{{"native", native}, {"written", written}} {
		for _, first := range []bool{true, false} {
			for _, route := range []struct{ path, body string }{
				{"/v1/chat/completions", `{"model":"group/flash","stream":true,"messages":[{"role":"user","content":"go"}],` + execChatTool + `}`},
				{"/v1/chat/completions", `{"model":"group/flash","messages":[{"role":"user","content":"go"}],` + execChatTool + `}`},
				{"/v1/responses", `{"model":"group/flash","stream":true,"input":"go",` + execResponsesTool + `}`},
				{"/v1/responses", `{"model":"group/flash","stream":false,"input":"go",` + execResponsesTool + `}`},
				{"/v1/messages", `{"model":"group/flash","max_tokens":100,"stream":false,"messages":[{"role":"user","content":"go"}],"tools":[{"name":"exec_command","input_schema":{"type":"object"}}]}`},
				{"/v1/messages", `{"model":"group/flash","max_tokens":100,"stream":true,"messages":[{"role":"user","content":"go"}],"tools":[{"name":"exec_command","input_schema":{"type":"object"}}]}`},
			} {
				fresh(t)
				ok := reply{200, "text/event-stream", up.body}
				a, b := &scripted{replies: []reply{ok}}, &scripted{replies: []reply{ok}}
				if !first {
					a.replies = []reply{refused}
				}
				scriptedOn(t, "a", provider.Chat, a)
				scriptedOn(t, "b", provider.Chat, b)
				if err := provider.SaveGroup(provider.Group{ID: "flash", Name: "DeepSeek-V4.1 flash", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
					t.Fatal(err)
				}
				rec := httptest.NewRecorder()
				New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", route.path, strings.NewReader(route.body)))
				body := rec.Body.String()
				served := "a"
				if !first {
					served = "b"
				}
				bad := rec.Code != 200 || strings.Contains(body, "DSML") || !strings.Contains(body, "exec_command") || strings.Contains(body, "overloaded")
				switch route.path {
				case "/v1/chat/completions":
					bad = bad || !strings.Contains(body, `"finish_reason":"tool_calls"`) || strings.Contains(body, `"finish_reason":"stop"`)
				case "/v1/messages":
					bad = bad || !strings.Contains(body, `"stop_reason":"tool_use"`)
				}
				if up.name == "native" {
					bad = bad || strings.Contains(body, `"content":"\n"`) || strings.Contains(body, `"text":"\n"`)
				}
				if bad || (first && b.n != 0) || (!first && (a.n != 1 || b.n != 1)) {
					t.Fatalf("%s, served by %s, %s %s: %d (a %d, b %d tries)\n%s", up.name, served, route.path, route.body, rec.Code, a.n, b.n, body)
				}
			}
		}
	}
}
