package sessions

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func codexFixtureSpans(t *testing.T, name string, bodies bool) []TraceSpan {
	t.Helper()
	f, err := os.Open(filepath.Join("testdata", "traces", name))
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	c := traceTestCursor("codex")
	var spans []TraceSpan
	s := bufio.NewScanner(f)
	s.Buffer(nil, 1<<20)
	for s.Scan() {
		spans = append(spans, c.line(s.Bytes(), bodies)...)
	}
	if s.Err() != nil {
		t.Fatal(s.Err())
	}
	return spans
}

type codexTestMessage struct {
	Role       string
	Content    json.RawMessage
	ToolCallID string `json:"tool_call_id"`
	ToolCalls  []struct {
		ID       string
		Function struct{ Name, Arguments string }
	} `json:"tool_calls"`
}

func codexMessages(t *testing.T, body string) []codexTestMessage {
	t.Helper()
	var m []codexTestMessage
	if body == "" {
		return nil
	}
	if err := json.Unmarshal([]byte(body), &m); err != nil {
		t.Fatalf("body is not a message list: %v\n%s", err, body)
	}
	for _, x := range m {
		if x.Role == "" {
			t.Fatalf("message without a role: %s", body)
		}
	}
	return m
}

// JuckZ's #2284 rollout: two Codex Desktop turns, 28 responses, 8 replies,
// most responses only calling a tool, and a settings notice between turns.
func TestCodexDesktopTraceEachResponseOwnsItsOutput(t *testing.T) {
	for _, bodies := range []bool{false, true} {
		spans := codexFixtureSpans(t, "codex-desktop-sanitized.jsonl", bodies)
		roots, models, tools := spanKinds(spans)
		ids := map[string]int{}
		for _, r := range roots {
			if !r.Pending {
				ids[r.ID]++
				if r.Error || r.Inferred {
					t.Errorf("bodies=%v: a completed turn exported as interrupted: %+v", bodies, r)
				}
			}
		}
		if len(ids) != 2 || len(roots)-2 != 2 {
			t.Fatalf("bodies=%v: want 2 finished roots (plus 2 pending starts), got %d spans over %d IDs", bodies, len(roots)-2, len(ids))
		}
		for id, n := range ids {
			if n != 1 {
				t.Fatalf("bodies=%v: root %s exported %d times", bodies, id, n)
			}
		}
		if len(models) != 28 {
			t.Fatalf("bodies=%v: %d model responses, want 28", bodies, len(models))
		}
		web := 0
		for _, s := range tools {
			if s.Name == "web" {
				web++
				if bodies && !strings.Contains(s.Input, "Fixture search.") {
					t.Fatalf("extension query missing: %+v", s)
				}
			}
			if bodies && s.Name == "view image" && !strings.Contains(s.Input, "fixture.png") {
				t.Fatalf("viewed image path missing: %+v", s)
			}
			if bodies && s.Name == "file change" && !strings.Contains(s.Input, "Fixture file diff.") {
				t.Fatalf("file change diff missing: %+v", s)
			}
		}
		if web != 4 {
			t.Fatalf("bodies=%v: %d web extension operations, want 4", bodies, web)
		}
		if !bodies {
			for _, s := range spans {
				if s.Input != "" || s.Output != "" {
					t.Fatalf("body exported while disabled: %+v", s)
				}
			}
			continue
		}
		replies := map[string]int{}
		called := map[string]bool{}
		answered := map[string]bool{}
		for _, m := range models {
			for _, x := range codexMessages(t, m.Output) {
				if x.Role != "assistant" {
					t.Fatalf("non-assistant output %s", m.Output)
				}
				var text []struct{ Text string }
				json.Unmarshal(x.Content, &text)
				for _, p := range text {
					replies[p.Text]++
				}
				for _, call := range x.ToolCalls {
					if called[call.ID] || call.Function.Name == "" {
						t.Fatalf("tool call %s repeated or unnamed", call.ID)
					}
					called[call.ID] = true
				}
			}
			in := codexMessages(t, m.Input)
			if len(in) < 3 || in[0].Role != "system" || !strings.Contains(string(in[0].Content), "Fixture system instructions.") || in[1].Role != "developer" {
				t.Fatalf("prompt missing system or developer message: %s", m.Input)
			}
			turn1 := strings.Contains(m.Input, "Fixture user message 1.")
			turn2 := strings.Contains(m.Input, "Fixture user message 2.")
			if turn1 == turn2 {
				t.Fatalf("input holds the wrong turn's prompt: %s", m.Input)
			}
			for _, x := range in {
				if x.Role == "tool" {
					if !called[x.ToolCallID] {
						t.Fatalf("result %s fed before its call", x.ToolCallID)
					}
					answered[x.ToolCallID] = true
				}
			}
		}
		for i := 1; i <= 8; i++ {
			text := fmt.Sprintf("Fixture assistant message %d.", i)
			if replies[text] != 1 {
				t.Errorf("%q in %d responses' output, want 1", text, replies[text])
			}
		}
		if len(called) != 25 || len(answered) != 25 {
			t.Fatalf("calls %d, results fed back %d; want 25 each", len(called), len(answered))
		}
	}
}

// The Codex CLI rollout records no turn ID on its response items; replies
// still belong only to the response that wrote them.
func TestCodexCLITraceRepliesStayWithTheirResponse(t *testing.T) {
	_, models, _ := spanKinds(codexFixtureSpans(t, "codex-real-sanitized.jsonl", true))
	text := 0
	for _, m := range models {
		for _, x := range codexMessages(t, m.Output) {
			if len(x.Content) > 0 {
				text++
			}
		}
	}
	if len(models) != 5 || text != 3 {
		t.Fatalf("%d responses carry reply text, want 3 of 5", text)
	}
}

// A screenshot in a tool result is megabytes of base64. The tool's own span
// keeps it as recorded; the prompt, sent with every response, names it.
func TestCodexTraceInlineImageNotRepeated(t *testing.T) {
	image := "data:image/png;base64," + strings.Repeat("A", 4096)
	c := traceTestCursor("codex")
	spans := clientTraceFeed(t, c, true,
		`{"type":"session_meta","payload":{"id":"session"}}`,
		`{"type":"event_msg","timestamp":"2026-10-02T12:00:00Z","payload":{"type":"task_started","turn_id":"turn"}}`,
		`{"type":"response_item","timestamp":"2026-10-02T12:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"look"},{"type":"input_image","image_url":"`+image+`"}]}}`,
		`{"type":"response_item","timestamp":"2026-10-02T12:00:02Z","payload":{"type":"custom_tool_call","name":"exec","call_id":"shot","input":"screenshot"}}`,
		`{"type":"token_usage_record","timestamp":"2026-10-02T12:00:03Z","payload":{"response_id":"r1"}}`,
		`{"type":"response_item","timestamp":"2026-10-02T12:00:04Z","payload":{"type":"custom_tool_call_output","call_id":"shot","output":[{"type":"input_text","text":"done"},{"type":"input_image","image_url":"`+image+`"}]}}`,
		`{"type":"token_usage_record","timestamp":"2026-10-02T12:00:05Z","payload":{"response_id":"r2"}}`)
	_, models, tools := spanKinds(spans)
	if len(models) != 2 || len(tools) != 1 || !strings.Contains(tools[0].Output, image) {
		t.Fatalf("models %d tools %+v", len(models), tools)
	}
	for _, m := range models {
		if strings.Contains(m.Input, "AAAA") || !strings.Contains(m.Input, "image/png, 4096 bytes") {
			t.Fatalf("inline image copied into the prompt: %.200s", m.Input)
		}
	}
	if !strings.Contains(models[1].Input, `"tool_call_id":"shot"`) || models[1].Output != "" {
		t.Fatalf("second response: in %s out %s", models[1].Input, models[1].Output)
	}
}
