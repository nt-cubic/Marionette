package sessions

import (
	"bufio"
	"database/sql"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestCodexTraceNativeCallsAndMCPResult(t *testing.T) {
	for _, bodies := range []bool{false, true} {
		c := traceTestCursor("codex")
		spans := clientTraceFeed(t, c, bodies,
			`{"type":"session_meta","payload":{"id":"session"}}`,
			`{"type":"turn_context","timestamp":"2026-10-02T12:00:00Z","payload":{"turn_id":"turn","model":"model"}}`,
			`{"type":"response_item","timestamp":"2026-10-02T12:00:01Z","payload":{"type":"custom_tool_call","name":"exec","call_id":"exec","input":"private"}}`,
			`{"type":"response_item","timestamp":"2026-10-02T12:00:02Z","payload":{"type":"function_call","name":"wait","call_id":"wait","arguments":"private"}}`,
			`{"type":"response_item","timestamp":"2026-10-02T12:00:03Z","payload":{"type":"function_call_output","call_id":"wait","output":"wait result"}}`,
			`{"type":"response_item","timestamp":"2026-10-02T12:00:04Z","payload":{"type":"custom_tool_call_output","call_id":"exec","output":"exec result"}}`,
			`{"type":"response_item","timestamp":"2026-10-02T12:00:04Z","payload":{"type":"custom_tool_call_output","call_id":"exec","output":"exec result"}}`,
			`{"type":"event_msg","timestamp":"2026-10-02T12:00:05Z","payload":{"type":"item_completed","item":{"type":"McpToolCall","id":"mcp","tool":"read","result":{"content":[{"type":"text","text":"mcp result"}]}}}}`)
		_, _, tools := spanKinds(spans)
		if len(tools) != 3 || tools[0].Name != "wait" || tools[1].Name != "exec" || !tools[0].Inferred || !tools[1].Inferred || tools[1].End.Sub(tools[1].Start) != 3*time.Second {
			t.Fatalf("tools %+v", tools)
		}
		for _, span := range tools {
			if !bodies && (span.Input != "" || span.Output != "") {
				t.Fatal("body leaked")
			}
		}
		if bodies && (!strings.Contains(tools[2].Output, "mcp result") || !strings.Contains(tools[1].Output, "exec result")) {
			t.Fatal("tool result missing")
		}
	}
}

func TestCodexTraceRecordedMCPWinsOverNativeOutput(t *testing.T) {
	for _, bodies := range []bool{false, true} {
		c := traceTestCursor("codex")
		spans := clientTraceFeed(t, c, bodies,
			`{"type":"session_meta","payload":{"id":"session"}}`,
			`{"type":"turn_context","timestamp":"2026-10-02T12:00:00Z","payload":{"turn_id":"turn","model":"model"}}`,
			`{"type":"response_item","timestamp":"2026-10-02T12:00:01Z","payload":{"type":"function_call","call_id":"same-id","name":"read","arguments":"private"}}`,
			`{"type":"event_msg","timestamp":"2026-10-02T12:00:04Z","payload":{"type":"item_completed","started_at_ms":1790942402000,"completed_at_ms":1790942404000,"item":{"type":"McpToolCall","id":"same-id","tool":"read","arguments":{"path":"private"},"result":{"content":[{"type":"text","text":"recorded result"}]}}}}`,
			`{"type":"response_item","timestamp":"2026-10-02T12:00:05Z","payload":{"type":"function_call_output","call_id":"same-id","output":"inferred result"}}`)
		_, _, tools := spanKinds(spans)
		if len(tools) != 1 || tools[0].Inferred || tools[0].End.Sub(tools[0].Start) != 2*time.Second {
			t.Fatalf("duplicate or inferred MCP: %+v", tools)
		}
		if bodies && (!strings.Contains(tools[0].Output, "recorded result") || strings.Contains(tools[0].Output, "inferred result")) {
			t.Fatal("recorded result overwritten")
		}
		if !bodies && (tools[0].Input != "" || tools[0].Output != "") {
			t.Fatal("body leaked")
		}
		if len(c.tools) != 0 {
			t.Fatal("completed MCP call retained")
		}
	}
}

func TestSanitizedRealTraceSessions(t *testing.T) {
	for _, agent := range []string{"pi", "codex"} {
		for _, bodies := range []bool{false, true} {
			f, err := os.Open(filepath.Join("testdata", "traces", agent+"-real-sanitized.jsonl"))
			if err != nil {
				t.Fatal(err)
			}
			c := traceTestCursor(agent)
			var spans []TraceSpan
			s := bufio.NewScanner(f)
			for s.Scan() {
				spans = append(spans, c.line(s.Bytes(), bodies)...)
			}
			f.Close()
			if s.Err() != nil {
				t.Fatal(s.Err())
			}
			roots, models, tools := spanKinds(spans)
			wantModels, wantTools := 17, 16
			if agent == "codex" {
				wantModels, wantTools = 5, 8
			} // Four exec calls, each containing a recorded command.
			if len(models) != wantModels || len(tools) != wantTools || len(roots) < 2 {
				t.Fatalf("%s bodies=%v: roots=%d models=%d tools=%d", agent, bodies, len(roots), len(models), len(tools))
			}
			ids := map[string]bool{}
			for _, span := range spans {
				ids[span.ID] = true
				if !bodies && (span.Input != "" || span.Output != "") {
					t.Fatal("body leaked")
				}
			}
			for _, span := range spans {
				if span.Parent != "" && !ids[span.Parent] {
					t.Fatalf("orphan %s", span.ID)
				}
			}
		}
	}
}

func TestOpenCodeTraceLargeSessionProgressAndIncrementalUpdate(t *testing.T) {
	path := filepath.Join(t.TempDir(), "opencode.db")
	t.Setenv("OPENCODE_DB", path)
	db, err := sql.Open("sqlite", path)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	for _, q := range []string{
		`CREATE TABLE session (id TEXT PRIMARY KEY,parent_id TEXT,directory TEXT,title TEXT,time_created INTEGER,time_updated INTEGER)`,
		`CREATE TABLE message (id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,time_updated INTEGER,data TEXT)`,
		`CREATE TABLE part (id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,time_updated INTEGER,data TEXT)`,
		`CREATE INDEX message_updates ON message(session_id,time_updated,id)`,
		`CREATE INDEX part_updates ON part(session_id,time_updated,id)`,
	} {
		if _, err = db.Exec(q); err != nil {
			t.Fatal(err)
		}
	}
	now := time.Now().Add(-time.Second).UnixMilli()
	tx, err := db.Begin()
	if err != nil {
		t.Fatal(err)
	}
	if _, err = tx.Exec(`INSERT INTO session VALUES ('s',NULL,'','',?,?)`, now, now); err != nil {
		t.Fatal(err)
	}
	user := fmt.Sprintf(`{"role":"user","text":"prompt","time":{"created":%d}}`, now)
	if _, err = tx.Exec(`INSERT INTO message VALUES ('u','s',?,?,?)`, now, now, user); err != nil {
		t.Fatal(err)
	}
	large := strings.Repeat("x", 200<<10)
	for i := 0; i < 200; i++ {
		id := fmt.Sprintf("a%03d", i)
		m := fmt.Sprintf(`{"role":"assistant","parentID":"u","modelID":"model","time":{"created":%d,"completed":%d}}`, now+1, now+2+int64(i))
		if _, err = tx.Exec(`INSERT INTO message VALUES (?,'s',?,?,?)`, id, now+1, now, m); err != nil {
			t.Fatal(err)
		}
		p, _ := json.Marshal(map[string]any{"type": "text", "text": large})
		if _, err = tx.Exec(`INSERT INTO part VALUES (?,?,'s',?,?)`, id, id, now, string(p)); err != nil {
			t.Fatal(err)
		}
	}
	if err = tx.Commit(); err != nil {
		t.Fatal(err)
	}
	r := NewTraceReader(ms(now).Add(-time.Second))
	models := 0
	for i := 0; i < 12; i++ {
		budget := int64(8 << 20)
		// Trace reading must not acquire the Sessions listing mutex.
		dbReadMu.Lock()
		done := make(chan []TraceSpan, 1)
		go func() { done <- r.pollOpenCode(false, &budget) }()
		var spans []TraceSpan
		select {
		case spans = <-done:
			dbReadMu.Unlock()
		case <-time.After(10 * time.Second):
			dbReadMu.Unlock()
			<-done
			t.Fatal("trace reading blocked on the listing mutex")
		}
		_, gen, _ := spanKinds(spans)
		for _, s := range gen {
			if !s.Update {
				models++
			}
		}
		if !r.ocPending {
			break
		}
	}
	if models != 200 || r.ocPending {
		t.Fatalf("large session made no progress: models=%d pending=%v", models, r.ocPending)
	}
	budget := int64(8 << 20)
	if spans := r.pollOpenCode(false, &budget); len(spans) != 0 || budget != 8<<20 {
		t.Fatal("idle store re-read")
	}
	// One changed part must not re-read the other 199 200-KiB parts.
	tool := fmt.Sprintf(`{"type":"tool","tool":"bash","state":{"status":"completed","input":{"command":"private"},"output":"private","time":{"start":%d,"end":%d}}}`, now+1, now+3)
	if _, err = db.Exec(`UPDATE part SET time_updated=?,data=? WHERE id='a000'`, now+1000, tool); err != nil {
		t.Fatal(err)
	}
	budget = 8 << 20
	spans := r.pollOpenCode(false, &budget)
	_, _, tools := spanKinds(spans)
	if len(tools) != 1 || budget < (8<<20)-4096 {
		t.Fatalf("update re-read history: tools=%d bytes=%d", len(tools), (8<<20)-budget)
	}
	for _, span := range spans {
		if span.Input != "" || span.Output != "" {
			t.Fatal("body leaked")
		}
	}
}
