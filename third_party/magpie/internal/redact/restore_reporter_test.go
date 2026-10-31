package redact

import (
	"encoding/json"
	"fmt"
	"math/rand"
	"net/http/httptest"
	"strings"
	"testing"
	"unicode/utf8"
)

var reporterNames = map[string]string{
	"{{PERSON_kb32hzkg}}": "张伟",
	"{{PERSON_iye66366}}": "李娜",
	"{{PERSON_isyrz5ih}}": "王芳",
	"{{PERSON_j5kluh3x}}": "刘洋",
}

const reporterTable = "| 角色 | 成员 |\n| --- | --- |\n| 负责人 | {{PERSON_kb32hzkg}}、{{PERSON_iye66366}} |\n| 审核 | {{PERSON_isyrz5ih}}、{{PERSON_j5kluh3x}} |\n"

func holdReporterNames(t *testing.T) string {
	t.Helper()
	mu.Lock()
	for p, v := range reporterNames {
		values[p] = v
	}
	kinds["PERSON"] = true
	mu.Unlock()
	want := reporterTable
	for p, v := range reporterNames {
		want = strings.ReplaceAll(want, p, v)
	}
	return want
}

type proto struct {
	name string
	ev   func(i int, s string) string
	read func(data map[string]any) string
}

func j(v any) string { b, _ := json.Marshal(v); return string(b) }

func randObf(r *rand.Rand) string {
	const a = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"
	n := 1 + r.Intn(12)
	b := make([]byte, n)
	for i := range b {
		b[i] = a[r.Intn(len(a))]
	}
	return string(b)
}

func protos(r *rand.Rand) []proto {
	get := func(m any, path ...any) any {
		for _, p := range path {
			switch k := p.(type) {
			case string:
				mm, _ := m.(map[string]any)
				m = mm[k]
			case int:
				aa, _ := m.([]any)
				if k >= len(aa) {
					return nil
				}
				m = aa[k]
			}
		}
		return m
	}
	str := func(v any) string { s, _ := v.(string); return s }
	return []proto{
		{"anthropic-text", func(i int, s string) string {
			return "event: content_block_delta\ndata: " + j(map[string]any{"type": "content_block_delta", "index": 0, "delta": map[string]any{"type": "text_delta", "text": s}}) + "\n\n"
		}, func(d map[string]any) string { return str(get(d, "delta", "text")) }},
		{"anthropic-tool", func(i int, s string) string {
			return "event: content_block_delta\ndata: " + j(map[string]any{"type": "content_block_delta", "index": 1, "delta": map[string]any{"type": "input_json_delta", "partial_json": s}}) + "\n\n"
		}, func(d map[string]any) string { return str(get(d, "delta", "partial_json")) }},
		{"chat-text", func(i int, s string) string {
			return "data: " + j(map[string]any{"id": "chatcmpl-1", "object": "chat.completion.chunk", "choices": []any{map[string]any{"index": 0, "delta": map[string]any{"content": s}}}}) + "\n\n"
		}, func(d map[string]any) string { return str(get(d, "choices", 0, "delta", "content")) }},
		{"chat-tool", func(i int, s string) string {
			return "data: " + j(map[string]any{"id": "chatcmpl-1", "choices": []any{map[string]any{"index": 0, "delta": map[string]any{"tool_calls": []any{map[string]any{"index": 0, "function": map[string]any{"arguments": s}}}}}}}) + "\n\n"
		}, func(d map[string]any) string {
			return str(get(d, "choices", 0, "delta", "tool_calls", 0, "function", "arguments"))
		}},
		{"responses-text", func(i int, s string) string {
			return "event: response.output_text.delta\ndata: " + `{"type":"response.output_text.delta","sequence_number":` + fmt.Sprint(i) + `,"item_id":"msg_1","output_index":0,"content_index":0,"delta":` + j(s) + `,"logprobs":[],"obfuscation":` + j(randObf(r)) + "}\n\n"
		}, func(d map[string]any) string { return str(get(d, "delta")) }},
		{"responses-tool", func(i int, s string) string {
			return "event: response.function_call_arguments.delta\ndata: " + `{"type":"response.function_call_arguments.delta","sequence_number":` + fmt.Sprint(i) + `,"item_id":"fc_1","output_index":1,"delta":` + j(s) + `,"obfuscation":` + j(randObf(r)) + "}\n\n"
		}, func(d map[string]any) string { return str(get(d, "delta")) }},
		{"gemini-text", func(i int, s string) string {
			return "data: " + j(map[string]any{"candidates": []any{map[string]any{"content": map[string]any{"parts": []any{map[string]any{"text": s}}, "role": "model"}}}, "modelVersion": "gemini-2.5-pro", "responseId": "r1"}) + "\r\n\r\n"
		}, func(d map[string]any) string { return str(get(d, "candidates", 0, "content", "parts", 0, "text")) }},
	}
}

func runStream(p proto, pieces []string) string {
	rec := httptest.NewRecorder()
	rec.Header().Set("Content-Type", "text/event-stream")
	w := NewWriter(rec)
	w.WriteHeader(200)
	for i, s := range pieces {
		w.Write([]byte(p.ev(i, s)))
	}
	w.Finish()
	var b strings.Builder
	for _, line := range strings.Split(rec.Body.String(), "\n") {
		line = strings.TrimRight(line, "\r")
		d, ok := strings.CutPrefix(line, "data: ")
		if !ok {
			continue
		}
		var m map[string]any
		if json.Unmarshal([]byte(d), &m) != nil {
			return "BAD EVENT " + d
		}
		b.WriteString(p.read(m))
	}
	return b.String()
}

func runeSplits(s string, k int) []string {
	var out []string
	for s != "" {
		n := 0
		for c := 0; c < k && n < len(s); c++ {
			_, sz := utf8.DecodeRuneInString(s[n:])
			n += sz
		}
		out = append(out, s[:n])
		s = s[n:]
	}
	return out
}

func TestReporterTableRestoredOnEveryStream(t *testing.T) {
	want := holdReporterNames(t)
	args := j(map[string]any{"file_path": "/tmp/team.md", "content": reporterTable})
	wantArgs := j(map[string]any{"file_path": "/tmp/team.md", "content": want})
	r := rand.New(rand.NewSource(1))
	for _, p := range protos(r) {
		in, out := reporterTable, want
		if strings.HasSuffix(p.name, "tool") {
			in, out = args, wantArgs
		}
		fails := 0
		for k := 1; k <= 7; k++ {
			for rep := 0; rep < 20; rep++ {
				got := runStream(p, runeSplits(in, k))
				if got != out {
					fails++
					if fails < 3 {
						t.Errorf("%s k=%d: got %q", p.name, k, got)
					}
				}
			}
		}
		for i := 1; i < len(in); i++ {
			if !utf8.RuneStart(in[i]) {
				continue
			}
			got := runStream(p, []string{in[:i], in[i:]})
			if got != out {
				fails++
				if fails < 5 {
					t.Errorf("%s split %d: got %q", p.name, i, got)
				}
			}
		}
		t.Logf("%s: %d fails", p.name, fails)
	}
}

func TestPlaceholdersRestoredWhereverTheySit(t *testing.T) {
	holdReporterNames(t)
	for _, c := range []struct{ name, in, notWant string }{
		{"anthropic tool_use name key", `{"type":"message","content":[{"type":"tool_use","id":"t1","name":"write_table","input":{"rows":[{"name":"{{PERSON_kb32hzkg}}","role":"负责人"}]}}]}`, "PERSON_"},
		{"anthropic tool_use data key", `{"type":"message","content":[{"type":"tool_use","id":"t1","name":"x","input":{"data":"{{PERSON_kb32hzkg}}、{{PERSON_iye66366}}","title":"{{PERSON_isyrz5ih}}"}}]}`, "PERSON_"},
		{"gemini functionCall args name", `{"candidates":[{"content":{"parts":[{"functionCall":{"name":"write","args":{"name":"{{PERSON_kb32hzkg}}","status":"{{PERSON_iye66366}}"}}}]}}]}`, "PERSON_"},
		{"chat no braces", `{"choices":[{"message":{"content":"负责人 PERSON_kb32hzkg、PERSON_iye66366"}}]}`, "PERSON_"},
		{"chat single brace", `{"choices":[{"message":{"content":"负责人 {PERSON_kb32hzkg}、{PERSON_iye66366}"}}]}`, "PERSON_"},
		{"text", reporterTable, "PERSON_"},
	} {
		got := string(RestoreJSON([]byte(c.in)))
		if strings.Contains(got, c.notWant) {
			t.Errorf("%s: %s", c.name, got)
		}
	}
	// stream gemini args object
	rec := httptest.NewRecorder()
	rec.Header().Set("Content-Type", "text/event-stream")
	w := NewWriter(rec)
	w.Write([]byte(`data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"write","args":{"name":"{{PERSON_kb32hzkg}}","type":"{{PERSON_iye66366}}"}}}]}}]}` + "\n\n"))
	w.Finish()
	if strings.Contains(rec.Body.String(), "PERSON_") {
		t.Errorf("gemini stream args: %s", rec.Body.String())
	}
}

// Past maxValues the values used longest ago go, never the ones just
// masked: their placeholders are the ones a reply in flight brings back.
func TestFullValuesKeepTheNewest(t *testing.T) {
	Forget()
	defer Forget()
	for i := 0; len(values) < maxValues-1000; i++ {
		placeholder("PERSON", fmt.Sprint("old", i))
	}
	var recent []string
	for i := 0; i < 1000; i++ {
		recent = append(recent, placeholder("PERSON", fmt.Sprint("new", i)))
	}
	placeholder("PERSON", "the one that makes room")
	lost := 0
	for _, p := range recent {
		if _, ok := values[p]; !ok {
			lost++
		}
	}
	if lost > 0 {
		t.Fatalf("%d of the 1000 newest values were let go", lost)
	}
}

// Audio, images and other binary replies go through as they come: there is
// no text in them to restore, and holding a speech stream whole delays it.
func TestBinaryReplyIsNotHeld(t *testing.T) {
	rec := httptest.NewRecorder()
	rec.Header().Set("Content-Type", "audio/mpeg")
	w := NewWriter(rec)
	w.WriteHeader(200)
	w.Write([]byte("ID3\x04\x00{{PERSON_"))
	if rec.Body.Len() == 0 {
		t.Fatal("an audio reply was held back")
	}
	w.Finish()
}
