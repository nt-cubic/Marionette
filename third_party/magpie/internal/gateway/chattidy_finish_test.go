package gateway

import (
	"strings"
	"testing"
)

// 喜欢一个人 on Discord: a relay in front of deepseek-v4.1-flash made the
// calls and ended the reply "stop", and a Chat client that reads
// finish_reason ended its turn with them not run. A relayed Chat reply
// that made calls ends "tool_calls", streamed and whole; one that made
// none keeps its "stop", and the other reasons stay as they came.
func TestChatCallsEndToolCalls(t *testing.T) {
	stream := `{"model":"a/m","stream":true,"messages":[{"role":"user","content":"go"}],` + execChatTool + `}`
	_, calls, finish, raw := relayedChat(t, stream, chunkOf("Running it."), nativeExec, stopChunk)
	if calls["exec_command"] != `{"cmd":"ls -la"}` || finish != "tool_calls" || !strings.Contains(raw, `"total_tokens":5`) {
		t.Fatalf("stream: calls %v finish %q\n%s", calls, finish, raw)
	}
	// with no tools offered, the same
	_, _, finish, raw = relayedChat(t, `{"model":"a/m","stream":true,"messages":[{"role":"user","content":"go"}]}`, nativeExec, stopChunk)
	if finish != "tool_calls" {
		t.Fatalf("stream, no tools offered: finish %q\n%s", finish, raw)
	}
	_, _, finish, raw = relayedChat(t, stream, chunkOf("Hi."), stopChunk)
	if finish != "stop" {
		t.Fatalf("stream, no call: finish %q\n%s", finish, raw)
	}
	_, _, finish, raw = relayedChat(t, stream, nativeExec, strings.Replace(stopChunk, `"stop"`, `"length"`, 1))
	if finish != "length" {
		t.Fatalf("stream cut short: finish %q\n%s", finish, raw)
	}

	whole := func(content, reason string) string {
		return `{"id":"c1","object":"chat.completion","created":1791370000,"model":"m","choices":[{"index":0,"message":{"role":"assistant","content":` + content + `,"tool_calls":[{"id":"call_up","type":"function","function":{"name":"exec_command","arguments":"{\"cmd\":\"ls -la\"}"}}]},"finish_reason":"` + reason + `"}]}`
	}
	for _, body := range []string{
		`{"model":"a/m","messages":[{"role":"user","content":"go"}],` + execChatTool + `}`,
		`{"model":"a/m","messages":[{"role":"user","content":"go"}]}`,
	} {
		for _, c := range []struct{ content, reason, want string }{
			{`"Running it."`, "stop", "tool_calls"},
			{`null`, "stop", "tool_calls"},
			{`null`, "length", "length"},
		} {
			_, wcalls, finish, raw := wholeChat(t, body, "application/json", whole(c.content, c.reason))
			if len(wcalls) != 1 || finish != c.want {
				t.Fatalf("whole %s %s: calls %+v finish %q\n%s", c.content, c.reason, wcalls, finish, raw)
			}
		}
	}
	_, _, finish, raw = wholeChat(t, `{"model":"a/m","messages":[{"role":"user","content":"go"}],`+execChatTool+`}`, "application/json", wholeChatUp("Hi."))
	if finish != "stop" {
		t.Fatalf("whole, no call: finish %q\n%s", finish, raw)
	}
}
