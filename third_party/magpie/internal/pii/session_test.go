package pii

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/redact"
)

// sessionBody is turn n of an agent's Chat Completions session, as a
// gateway gets it: the whole conversation again, a tool result of about
// 4 KB a turn.
func sessionBody(n int) []byte {
	msgs := []any{map[string]any{"role": "system", "content": "You are a coding agent working in /home/mrwl/projects/api. " + strings.Repeat("Run the tests, keep changes small. ", 40)}}
	for i := 0; i < n; i++ {
		var out strings.Builder
		for k := 0; out.Len() < 4<<10; k++ {
			fmt.Fprintf(&out, "/home/mrwl/projects/api/store_%d_%d.go:%d: db.QueryRow(\"SELECT email FROM users WHERE id = $1\") // Alice %d\n", i, k, 40+k, k)
		}
		msgs = append(msgs,
			map[string]any{"role": "user", "content": fmt.Sprintf("step %d: the webhook still fails on retries", i)},
			map[string]any{"role": "assistant", "content": "", "tool_calls": []any{map[string]any{"id": fmt.Sprintf("call_%d", i), "type": "function", "function": map[string]any{"name": "shell", "arguments": fmt.Sprintf(`{"command":"grep -rn webhook . | head -%d"}`, 50+i)}}}},
			map[string]any{"role": "tool", "tool_call_id": fmt.Sprintf("call_%d", i), "content": out.String()},
		)
	}
	b, _ := json.Marshal(map[string]any{"model": "gpt-6-luna", "stream": true, "messages": msgs})
	return b
}

// A long conversation's request has the local model read what that turn
// added, not the conversation again: masking turn 61 of a ~260 KB session
// after turn 60 reads the new turn's few KB. mrwl666 on Discord (11-01)
// took the 2-3 s at 100% of a core a request for every message read
// again; it is the model reading the new turn's tool result, on the CPU
// about a KB a second a core.
func TestSessionTurnReadsOnlyWhatItAdds(t *testing.T) {
	r := newReader(t, guardChunk)
	o := redact.Options{Secrets: true, Personal: true, Route: "openai"}
	mask := func(body []byte) []byte {
		oo := o
		oo.Model = func(texts []string) [][]redact.Found {
			got, err := r.find(Bounded(context.Background()), texts)
			if err != nil {
				t.Fatal(err)
			}
			return got
		}
		out, _ := redact.MaskJSON(body, oo)
		return out
	}
	const turns = 60
	mask(sessionBody(turns))
	first, reads := r.bytes()
	body := sessionBody(turns + 1)
	added := len(body) - len(sessionBody(turns))
	out := mask(body)
	n, _ := r.bytes()
	// what it adds is a tool result read in two pieces, which overlap
	if read := n - first; read == 0 || read > 2*added {
		t.Fatalf("turn %d of a %d KB session had the model read %d bytes; the turn added %d", turns+1, len(body)>>10, read, added)
	}
	for _, a := range r.asked[reads:] {
		if !strings.Contains(a, fmt.Sprintf("store_%d_", turns)) && !strings.Contains(a, fmt.Sprintf("step %d:", turns)) {
			t.Fatalf("turn %d had the model read text of an earlier turn again: %.80q", turns+1, a)
		}
	}
	// and it is masked as the turn before had it: the same history, the
	// same placeholders
	again := mask(body)
	if string(out) != string(again) {
		t.Fatal("masking the same turn twice gave different bodies")
	}
	if first < 200<<10 {
		t.Fatalf("the first request read %d bytes: not the whole session", first)
	}
}
