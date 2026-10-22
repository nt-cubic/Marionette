package sessions

import (
	"bufio"
	"fmt"
	"io"
	"math/rand"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/testenv"
)

// readBytesLines is how TraceReader.Poll read a file before nextTraceLine:
// each whole line from off, its newline included, and where the lines end.
func readBytesLines(t *testing.T, p string, off int64) ([]string, int64) {
	t.Helper()
	f, err := os.Open(p)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	f.Seek(off, io.SeekStart)
	b := bufio.NewReader(f)
	var out []string
	for {
		line, err := b.ReadBytes('\n')
		if err != nil {
			return out, off
		}
		off += int64(len(line))
		out = append(out, string(line))
	}
}

func nextTraceLines(t *testing.T, p string, off int64) ([]string, int64) {
	t.Helper()
	f, err := os.Open(p)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	f.Seek(off, io.SeekStart)
	b, release := lineReader(f, scanBuffer(f, off))
	defer release()
	var out []string
	var long []byte
	for {
		var line []byte
		line, long, err = nextTraceLine(b, f, off, long)
		if err != nil {
			return out, off
		}
		off += int64(len(line))
		out = append(out, string(line))
	}
}

// A trace file's lines come out of nextTraceLine as they came out of
// ReadBytes: lines longer than the read buffer, one exactly its size,
// empty ones, CRLF, and a last line not yet ended, short or long, which
// is left for the next poll.
func TestNextTraceLineReadsWhatReadBytesRead(t *testing.T) {
	r := rand.New(rand.NewSource(4))
	long := func(n int) string {
		b := make([]byte, n)
		for i := range b {
			b[i] = byte('a' + r.Intn(26))
		}
		return `{"type":"x","data":"` + string(b) + `"}`
	}
	for name, body := range map[string]string{
		"long lines":     long(3<<20) + "\n" + `{"a":1}` + "\n" + long(1<<20) + "\n" + long(5<<20) + "\n",
		"buffer's size":  long(1<<20-len(`{"type":"x","data":""}`)-1) + "\n" + long(1<<20) + "\n{}\n",
		"empty lines":    "\n\n{}\n\n" + long(2<<20) + "\n\n",
		"crlf":           "{}\r\n" + long(2<<20) + "\r\n",
		"short unended":  long(2<<20) + "\n{\"a\":",
		"long unended":   "{}\n" + long(3<<20),
		"no lines yet":   long(2 << 20),
		"nothing at all": "",
	} {
		t.Run(name, func(t *testing.T) {
			p := filepath.Join(t.TempDir(), "rollout.jsonl")
			if err := os.WriteFile(p, []byte(body), 0o600); err != nil {
				t.Fatal(err)
			}
			for _, off := range []int64{0, int64(strings.IndexByte(body, '\n') + 1)} {
				want, wantEnd := readBytesLines(t, p, off)
				got, gotEnd := nextTraceLines(t, p, off)
				if gotEnd != wantEnd || !reflect.DeepEqual(got, want) {
					t.Fatalf("from %d: %d lines ending at %d, want %d ending at %d", off, len(got), gotEnd, len(want), wantEnd)
				}
			}
		})
	}
}

// A line still being written is not read until its newline is there,
// however long it is, and is then read whole.
func TestNextTraceLineWaitsForALongLineToEnd(t *testing.T) {
	p := filepath.Join(t.TempDir(), "rollout.jsonl")
	line := `{"type":"x","data":"` + strings.Repeat("y", 3<<20) + `"}`
	os.WriteFile(p, []byte("{}\n"+line), 0o600)
	if got, end := nextTraceLines(t, p, 0); len(got) != 1 || end != 3 {
		t.Fatalf("read %d lines to %d before the long one ended, want 1 to 3", len(got), end)
	}
	f, _ := os.OpenFile(p, os.O_APPEND|os.O_WRONLY, 0o600)
	f.WriteString("\n")
	f.Close()
	if got, end := nextTraceLines(t, p, 3); len(got) != 1 || got[0] != line+"\n" || end != int64(4+len(line)) {
		t.Fatalf("the ended long line read as %d lines to %d", len(got), end)
	}
}

// pollEnv is a home with nothing in it but Codex's sessions, under codex.
func pollEnv(t *testing.T) (codex string) {
	testenv.SetHome(t, t.TempDir())
	t.Setenv("XDG_DATA_HOME", t.TempDir())
	t.Setenv("CLAUDE_CONFIG_DIR", t.TempDir())
	t.Setenv("OPENCODE_DB", "")
	t.Setenv("GEMINI_CLI_HOME", t.TempDir())
	t.Setenv("PI_CODING_AGENT_DIR", t.TempDir())
	t.Setenv("PI_CODING_AGENT_SESSION_DIR", "")
	codex = t.TempDir()
	t.Setenv("CODEX_HOME", codex)
	return codex
}

// imageTurns is a Codex rollout whose turns each begin with a user message
// carrying a pasted image of the size given, as the Codex CLI writes one.
func imageTurns(start time.Time, images []int) string {
	r := rand.New(rand.NewSource(3))
	const b64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
	ts := func(i int) string { return start.Add(time.Duration(i) * time.Millisecond).Format(time.RFC3339Nano) }
	var b strings.Builder
	b.WriteString(`{"timestamp":"` + ts(0) + `","type":"session_meta","payload":{"id":"01a0bdc9-b5fd-7e63-9658-68bc8dce5ecd","cwd":"/Users/me/work/app","originator":"codex_cli_rs","cli_version":"0.99.0"}}` + "\n")
	for i, n := range images {
		img := make([]byte, n)
		for k := range img {
			img[k] = b64[r.Intn(64)]
		}
		fmt.Fprintf(&b, `{"timestamp":%q,"type":"turn_context","payload":{"turn_id":"turn_%d","cwd":"/Users/me/work/app","model":"gpt-5.5","effort":"medium"}}`+"\n", ts(10*i+1), i)
		fmt.Fprintf(&b, `{"timestamp":%q,"type":"event_msg","payload":{"type":"task_started","turn_id":"turn_%d"}}`+"\n", ts(10*i+2), i)
		fmt.Fprintf(&b, `{"timestamp":%q,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"look"},{"type":"input_image","image_url":"data:image/png;base64,%s"}]}}`+"\n", ts(10*i+3), img)
		b.WriteString("\n")
		fmt.Fprintf(&b, `{"timestamp":%q,"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{}","call_id":"call_%d"}}`+"\n", ts(10*i+4), i)
		fmt.Fprintf(&b, `{"timestamp":%q,"type":"response_item","payload":{"type":"function_call_output","call_id":"call_%d","output":%q}}`+"\n", ts(10*i+5), i, strings.Repeat("out ", 3000))
		fmt.Fprintf(&b, `{"timestamp":%q,"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}`+"\n", ts(10*i+6))
		fmt.Fprintf(&b, `{"timestamp":%q,"type":"event_msg","payload":{"type":"task_complete","turn_id":"turn_%d","last_agent_message":"done"}}`+"\n", ts(10*i+7), i)
	}
	return b.String()
}

// What Poll exports from a rollout with long lines, read across polls as
// it is written (a long line cut off mid-way at one poll's end), is what a
// cursor fed the same file's lines through ReadBytes exports.
func TestTracePollOfLongLinesIsUnchanged(t *testing.T) {
	codex := pollEnv(t)
	dir := filepath.Join(codex, "sessions", "2026", "10", "21")
	os.MkdirAll(dir, 0o700)
	p := filepath.Join(dir, "rollout-2026-10-21T10-00-00-01a0bdc9-b5fd-7e63-9658-68bc8dce5ecd.jsonl")
	since := time.Now().Add(-time.Hour)
	body := imageTurns(time.Now().UTC(), []int{3 << 20, 100, 2 << 20})
	for _, bodies := range []bool{false, true} {
		os.WriteFile(p, nil, 0o600)
		r := NewTraceReader(since)
		var got []TraceSpan
		// written in three parts, the first two ending inside a long line
		for _, cut := range []int{len(body) / 4, len(body) * 3 / 4, len(body)} {
			f, _ := os.OpenFile(p, os.O_APPEND|os.O_WRONLY, 0o600)
			fi, _ := f.Stat()
			f.WriteString(body[fi.Size():cut])
			f.Close()
			got = append(got, r.Poll(bodies)...)
		}
		c := newTraceCursor(file{path: p, agent: "codex"})
		var want []TraceSpan
		lines, _ := readBytesLines(t, p, 0)
		for _, line := range lines {
			for _, s := range c.line([]byte(line), bodies) {
				if !s.End.Before(since) && !s.Start.IsZero() && !s.End.Before(s.Start) && s.Session != "" {
					want = append(want, s)
				}
			}
		}
		if len(want) == 0 || !reflect.DeepEqual(got, want) {
			t.Fatalf("bodies=%v: Poll exported %d spans, ReadBytes' lines %d (or they differ)", bodies, len(got), len(want))
		}
	}
}

// A poll reads a long line into one buffer of the line's own size. Read by
// ReadBytes, it was gathered in 4 KiB pieces and then copied whole, twice
// the line, and a poll of a rollout with pasted images allocated about 4
// times what it read with bodies off.
func TestTracePollReadsALongLineOnce(t *testing.T) {
	if raceOn {
		t.Skip("-race lets a pool's buffers go")
	}
	codex := pollEnv(t)
	dir := filepath.Join(codex, "sessions", "2026", "10", "21")
	os.MkdirAll(dir, 0o700)
	p := filepath.Join(dir, "rollout-2026-10-21T10-00-00-01a0bdc9-b5fd-7e63-9658-68bc8dce5ecd.jsonl")
	// a reasoning item's encrypted content, which the parse passes over
	// without a copy, so what is left is the payload's copy and the read
	line := fmt.Sprintf(`{"timestamp":%q,"type":"response_item","payload":{"type":"reasoning","summary":[],"encrypted_content":"%s"}}`, time.Now().UTC().Format(time.RFC3339Nano), strings.Repeat("Z", 6<<20))
	os.WriteFile(p, []byte(imageTurns(time.Now().UTC(), nil)+line+"\n"), 0o600)
	since := time.Now().Add(-time.Hour)
	poll := func() { NewTraceReader(since).Poll(false) }
	poll()
	var a, b runtime.MemStats
	runtime.GC()
	runtime.ReadMemStats(&a)
	poll()
	runtime.ReadMemStats(&b)
	got := b.TotalAlloc - a.TotalAlloc
	t.Logf("a poll of a %d-byte line allocated %d bytes", len(line), got)
	// the line once, its payload once, and room for the rest; through
	// ReadBytes it was the line three times
	if most := uint64(len(line)) * 5 / 2; got > most {
		t.Fatalf("a poll of a %d-byte line allocated %d bytes, want at most %d", len(line), got, most)
	}
}
