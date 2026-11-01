package redact

import (
	"encoding/json"
	"fmt"
	"strings"
	"testing"
)

// A long agent session as it reaches the gateway: every turn sends the
// whole conversation again, a few KB of tool output a turn, with what the
// rules look for here and there in it (paths with a user's name, an
// email, an address, a key in a config file, a UUID, a log's IPs).

// toolOutput is turn i's tool output, about kb KB of it.
func toolOutput(i, kb int) string {
	var b strings.Builder
	for n := 0; b.Len() < kb*1024; n++ {
		switch (i + n) % 6 {
		case 0:
			fmt.Fprintf(&b, "%d:func (s *Server) handle%d(w http.ResponseWriter, r *http.Request) {\n\tctx, cancel := context.WithTimeout(r.Context(), 30*time.Second)\n\tdefer cancel()\n\tif err := s.store.Put(ctx, r.URL.Query().Get(\"id\"), body); err != nil {\n\t\thttp.Error(w, err.Error(), 500)\n\t}\n}\n", n, i*100+n)
		case 1:
			fmt.Fprintf(&b, "/home/mrwl/projects/api/internal/store/store_%d.go:%d:\terr := db.QueryRowContext(ctx, \"SELECT id, name, email FROM users WHERE id = $1\", id).Scan(&u.ID, &u.Name, &u.Email)\n", n, 40+n)
		case 2:
			fmt.Fprintf(&b, "2026-10-31T12:%02d:%02d.%03dZ INFO request id=%08x-1f2e-4d3c-9b8a-%012x method=POST path=/v1/orders/%d status=200 dur=%dms remote=10.0.%d.%d\n", n%60, i%60, n%1000, i*7919+n, n*104729, n, 3+n%90, n%255, i%255)
		case 3:
			fmt.Fprintf(&b, "    --- PASS: TestOrderFlow/case_%d (0.%02ds)\n    order_test.go:%d: created order %d for customer %d, total 129.%02d EUR\n", n, n%100, 100+n, 5000+n, 300+i, n%100)
		case 4:
			fmt.Fprintf(&b, "  config.yaml:%d  database_url: postgres://app:%s@db.internal:5432/orders  # turn %d\n", n, "S3cr3tPassw0rd2026", i)
		case 5:
			fmt.Fprintf(&b, "Author: Wei Zhang <wei.zhang@example.com>  commit %040x\n    fix: retry the payment webhook when the upstream answers 502 (#%d)\n", i*31337+n, 1000+n)
		}
	}
	return b.String()
}

// chatSession is a Chat Completions request on turn n, as an agent on a
// gateway sends gpt-6-luna one: a system prompt, then each turn a user
// message, an assistant tool call and its tool output.
func chatSession(n, kb int) []byte {
	msgs := []any{map[string]any{"role": "system", "content": "You are a coding agent working in /home/mrwl/projects/api. " + strings.Repeat("Follow the repository's conventions, run the tests, keep changes small. ", 60)}}
	for i := 0; i < n; i++ {
		msgs = append(msgs,
			map[string]any{"role": "user", "content": fmt.Sprintf("step %d: the order webhook still fails on retries, look at the logs and fix it", i)},
			map[string]any{"role": "assistant", "content": "", "tool_calls": []any{map[string]any{"id": fmt.Sprintf("call_%d", i), "type": "function", "function": map[string]any{"name": "shell", "arguments": fmt.Sprintf(`{"command":"grep -rn webhook /home/mrwl/projects/api | head -%d"}`, 50+i)}}}},
			map[string]any{"role": "tool", "tool_call_id": fmt.Sprintf("call_%d", i), "content": toolOutput(i, kb)},
		)
	}
	b, _ := json.Marshal(map[string]any{"model": "gpt-6-luna", "stream": true, "messages": msgs,
		"tools": []any{map[string]any{"type": "function", "function": map[string]any{"name": "shell", "description": "Run a shell command", "parameters": map[string]any{"type": "object", "properties": map[string]any{"command": map[string]any{"type": "string"}}}}}}})
	return b
}

// claudeSession is a Messages request on turn n, as Claude Code sends one:
// tool_use and tool_result blocks, thinking signed by the vendor.
func claudeSession(n, kb int) []byte {
	msgs := []any{}
	for i := 0; i < n; i++ {
		msgs = append(msgs,
			map[string]any{"role": "user", "content": []any{map[string]any{"type": "text", "text": fmt.Sprintf("step %d: fix the failing webhook retry", i)}}},
			map[string]any{"role": "assistant", "content": []any{
				map[string]any{"type": "thinking", "thinking": fmt.Sprintf("I should grep for the webhook handler, turn %d.", i), "signature": strings.Repeat("EqQBCkYIBxgCKkA", 20)},
				map[string]any{"type": "tool_use", "id": fmt.Sprintf("toolu_%024d", i), "name": "Bash", "input": map[string]any{"command": fmt.Sprintf("grep -rn webhook /home/mrwl/projects/api | head -%d", 50+i)}},
			}},
			map[string]any{"role": "user", "content": []any{map[string]any{"type": "tool_result", "tool_use_id": fmt.Sprintf("toolu_%024d", i), "content": toolOutput(i, kb)}}},
		)
	}
	b, _ := json.Marshal(map[string]any{"model": "claude-opus-5-5", "stream": true, "system": []any{map[string]any{"type": "text", "text": strings.Repeat("You are Claude Code. Use the tools to work in the user's repository. ", 60)}}, "messages": msgs})
	return b
}

// sessions are the three protocols' bodies on turn n, ~66k tokens (about
// 260 KB) at n = 60 and 4 KB of tool output a turn.
var sessions = []struct {
	name string
	body func(n, kb int) []byte
}{
	{"chat", chatSession},
	{"responses", func(n, kb int) []byte { return codexSession(n, kb) }},
	{"messages", claudeSession},
}

// codexSession is a Responses request on turn n, as Codex sends one.
func codexSession(n, kb int) []byte {
	var items []any
	for i := 0; i < n; i++ {
		items = append(items,
			map[string]any{"type": "message", "role": "user", "content": []any{map[string]any{"type": "input_text", "text": fmt.Sprintf("step %d: fix the failing webhook retry", i)}}},
			map[string]any{"type": "reasoning", "summary": []any{}, "encrypted_content": strings.Repeat("gAAAAABo", 40)},
			map[string]any{"type": "function_call", "name": "shell", "arguments": fmt.Sprintf(`{"command":["bash","-lc","grep -rn webhook /home/mrwl/projects/api | head -%d"]}`, 50+i), "call_id": fmt.Sprintf("call_%d", i)},
			map[string]any{"type": "function_call_output", "call_id": fmt.Sprintf("call_%d", i), "output": toolOutput(i, kb)},
		)
	}
	b, _ := json.Marshal(map[string]any{"model": "gpt-6-luna", "instructions": strings.Repeat("You are Codex, a coding agent. ", 100), "input": items, "stream": true, "prompt_cache_key": "0199a0b2-5c3d-7e8f-9a0b-1c2d3e4f5a6b"})
	return b
}

// BenchmarkMaskLongSession is masking one turn of a ~66k-token session
// whose turns before it were masked already, as each request of a long
// conversation is: the history is the same, one turn is new.
func BenchmarkMaskLongSession(b *testing.B) {
	for _, s := range sessions {
		b.Run(s.name, func(b *testing.B) {
			Forget()
			resetRuleMemo()
			o := Options{Secrets: true, Personal: true, Route: "openai"}
			const turns, kb = 60, 4
			MaskJSON(s.body(turns, kb), o)
			body := s.body(turns+1, kb)
			b.SetBytes(int64(len(body)))
			b.ReportAllocs()
			b.ResetTimer()
			for b.Loop() {
				MaskJSON(body, o)
			}
		})
	}
}
