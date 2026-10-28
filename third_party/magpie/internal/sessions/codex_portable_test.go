package sessions

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/klauspost/compress/zstd"

	"github.com/yetone/magpie/internal/claudeseal"
	"github.com/yetone/magpie/internal/codexsummary"
	"github.com/yetone/magpie/internal/filelock"
)

const portThread = "01a15820-ff81-7840-bb3f-e18771c53aa8"

// portRolloutLines is a Codex 0.162 rollout of a thread magpie compacted,
// in the shapes Codex writes (codex-rs/history/src/rollout_payload.rs,
// CompactedItemWire; the compaction item as magpie's gateway streams it):
// a session_meta, a user message that talks about "magpie1:" (text, never
// changed), magpie's compaction item as a response_item, the compacted
// checkpoint holding it in replacement_history with its metadata beside,
// a checkpoint with an unreadable one of magpie's, OpenAI's own sealed
// compaction, and a later turn.
func portRolloutLines(summary string) []string {
	enc := codexsummary.Encrypted(summary)
	return []string{
		`{"timestamp":"2026-10-20T07:05:20.118Z","type":"session_meta","payload":{"id":"` + portThread + `","timestamp":"2026-10-20T07:05:20.101Z","cwd":"/Users/me/w","originator":"codex_cli_rs","cli_version":"0.162.1","source":"cli","model_provider":"openai"}}`,
		`{"timestamp":"2026-10-20T07:05:31.002Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"why does \"magpie1:\" show in my rollout? <b>&</b>"}]}}`,
		`{"timestamp":"2026-10-20T07:40:02.551Z","type":"response_item","payload":{"type":"compaction","id":"cmp_magpie_1761000002","encrypted_content":"` + enc + `"},"metadata":{"compaction_model_hash":"h1"}}`,
		`{"timestamp":"2026-10-20T07:40:02.553Z","type":"compacted","payload":{"message":"","replacement_history":[{"type":"message","role":"developer","content":[{"type":"input_text","text":"<permissions instructions>"}]},{"type":"message","role":"user","content":[{"type":"input_text","text":"fix the build"}]},{"type":"compaction","id":"cmp_magpie_1761000002","encrypted_content":"` + enc + `"}],"replacement_history_metadata":[{"client_authored":true},{},{"compaction_model_hash":"h1"}],"window_number":2,"compaction_response_id":"resp_magpie_1761000002","latest_token_usage_record":{"input_tokens":182734,"output_tokens":1204}}}`,
		`{"timestamp":"2026-10-20T08:10:00.000Z","type":"compacted","payload":{"message":"","replacement_history":[{"type":"message","role":"user","content":[{"type":"input_text","text":"go on"}]},{"type":"compaction","encrypted_content":"magpie1:%%%"}],"replacement_history_metadata":[{"client_authored":true},{"compaction_model_hash":"h2"}]}}`,
		`{"timestamp":"2026-10-20T09:00:00.000Z","type":"response_item","payload":{"type":"compaction","id":"cmp_0a1b","encrypted_content":"gAAAAABo1x2y3z_OpenAIsOwn=="}}`,
		`{"timestamp":"2026-10-20T09:00:05.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done: 1.5e+300 tokens, <ok>"}]}}`,
	}
}

// portHome is a Codex home with thread-writer-locks, as Codex 0.162 keeps
// it, and the rollout at sessions/2026/10/20, plain or packed, last
// written an hour ago with mode 0640.
func portHome(t *testing.T, lines []string, zst bool) (home, path string) {
	t.Helper()
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	home = filepath.Join(t.TempDir(), ".codex")
	if err := os.MkdirAll(filepath.Join(home, "thread-writer-locks"), 0o755); err != nil {
		t.Fatal(err)
	}
	dir := filepath.Join(home, "sessions", "2026", "10", "20")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	path = filepath.Join(dir, "rollout-2026-10-20T15-05-20-"+portThread+".jsonl")
	b := []byte(strings.Join(lines, "\n") + "\n")
	if zst {
		path += ".zst"
		z, _ := zstd.NewWriter(nil)
		b = z.EncodeAll(b, nil)
	}
	if err := os.WriteFile(path, b, 0o640); err != nil {
		t.Fatal(err)
	}
	old := time.Now().Add(-time.Hour).Truncate(time.Second)
	if err := os.Chtimes(path, old, old); err != nil {
		t.Fatal(err)
	}
	return home, path
}

func readRollout(t *testing.T, path string) []string {
	t.Helper()
	in, err := openLines(path)
	if err != nil {
		t.Fatal(err)
	}
	defer in.Close()
	var buf bytes.Buffer
	buf.ReadFrom(in)
	return strings.Split(strings.TrimSuffix(buf.String(), "\n"), "\n")
}

func decode(t *testing.T, line string) map[string]any {
	t.Helper()
	var v map[string]any
	if err := json.Unmarshal([]byte(line), &v); err != nil {
		t.Fatalf("%v: %s", err, line)
	}
	return v
}

// LukeMa (X): a Codex conversation magpie compacted errored once magpie
// was quit, as OpenAI can't read magpie's compaction item
// (invalid_encrypted_content). PortCodexRollouts writes each of magpie's
// items as Codex's own summary message, the one the gateway sends for it,
// and changes nothing else: the other lines keep their bytes, the file
// its line count, mode and time, and a compacted line's metadata stays
// beside its items.
func TestPortCodexRollouts(t *testing.T) {
	for _, zst := range []bool{false, true} {
		t.Run(map[bool]string{false: "plain", true: "zst"}[zst], func(t *testing.T) {
			summary := "Progress: build fixed in internal/x.\nNext: run the suite."
			lines := portRolloutLines(summary)
			home, path := portHome(t, lines, zst)
			before, _ := os.Stat(path)

			res, err := PortCodexRollouts(context.Background(), home)
			if err != nil || !res.Done || len(res.Rewritten) != 1 || len(res.Held) != 0 {
				t.Fatalf("pass: %+v, %v", res, err)
			}
			after, err := os.Stat(path)
			if err != nil {
				t.Fatal(err)
			}
			if !after.ModTime().Equal(before.ModTime()) || after.Mode() != before.Mode() {
				t.Errorf("time/mode %v %v, was %v %v", after.ModTime(), after.Mode(), before.ModTime(), before.Mode())
			}
			got := readRollout(t, path)
			if len(got) != len(lines) {
				t.Fatalf("%d lines, was %d", len(got), len(lines))
			}
			for _, i := range []int{0, 1, 5, 6} {
				if got[i] != lines[i] {
					t.Errorf("line %d changed:\n%s\nwas\n%s", i, got[i], lines[i])
				}
			}
			want := codexsummary.UserMessage(codexsummary.Prefix + "\n" + summary)
			wantJSON, _ := json.Marshal(want)
			var wantAny any
			json.Unmarshal(wantJSON, &wantAny)

			l2 := decode(t, got[2])
			if !reflect.DeepEqual(l2["payload"], wantAny) {
				t.Errorf("response_item payload %v, want %v", l2["payload"], wantAny)
			}
			if !reflect.DeepEqual(l2["metadata"], map[string]any{"compaction_model_hash": "h1"}) || l2["type"] != "response_item" {
				t.Errorf("the rest of the line changed: %s", got[2])
			}
			p3 := decode(t, got[3])["payload"].(map[string]any)
			hist := p3["replacement_history"].([]any)
			if len(hist) != 3 || !reflect.DeepEqual(hist[2], wantAny) {
				t.Errorf("replacement_history %v", hist)
			}
			if meta := p3["replacement_history_metadata"].([]any); len(meta) != 3 {
				t.Errorf("metadata %v", meta)
			}
			if p3["window_number"] != float64(2) || p3["compaction_response_id"] != "resp_magpie_1761000002" {
				t.Errorf("checkpoint fields changed: %s", got[3])
			}
			// the unreadable one is left out with its metadata entry, as the
			// gateway leaves it out of what it sends
			p4 := decode(t, got[4])["payload"].(map[string]any)
			if h := p4["replacement_history"].([]any); len(h) != 1 {
				t.Errorf("replacement_history %v", h)
			}
			if m := p4["replacement_history_metadata"].([]any); len(m) != 1 || !reflect.DeepEqual(m[0], map[string]any{"client_authored": true}) {
				t.Errorf("metadata %v", m)
			}
			// nothing of magpie's that OpenAI would refuse is left
			for i, l := range got {
				if i != 1 && strings.Contains(l, codexsummary.Mark) {
					t.Errorf("line %d still has magpie's item: %s", i, l)
				}
			}
			// a second pass has nothing to do
			if res, err := PortCodexRollouts(context.Background(), home); err != nil || len(res.Rewritten) != 0 {
				t.Errorf("second pass: %+v, %v", res, err)
			}
		})
	}
}

// A rollout whose thread a Codex has open (its writer lock held) is never
// written under it: it is left as it was, kept for a later pass, and
// written once the Codex lets it go. A Codex home without
// thread-writer-locks, where there is no telling, is left alone.
func TestPortCodexRolloutsLeavesAnOpenThread(t *testing.T) {
	lines := portRolloutLines("s")
	home, path := portHome(t, lines, false)
	orig, _ := os.ReadFile(path)
	lf, err := os.OpenFile(filepath.Join(home, "thread-writer-locks", portThread+".lock"), os.O_CREATE|os.O_RDWR, 0o644)
	if err != nil {
		t.Fatal(err)
	}
	defer lf.Close()
	if got, err := filelock.TryLock(lf); !got || err != nil {
		t.Fatalf("lock: %v %v", got, err)
	}
	res, err := PortCodexRollouts(context.Background(), home)
	if err != nil || len(res.Held) != 1 || len(res.Rewritten) != 0 {
		t.Fatalf("pass with the thread open: %+v, %v", res, err)
	}
	if b, _ := os.ReadFile(path); !bytes.Equal(b, orig) {
		t.Fatal("written while a Codex had the thread open")
	}
	if left, _ := filepath.Glob(filepath.Join(filepath.Dir(path), ".magpie-*")); len(left) > 0 {
		t.Fatalf("left behind: %v", left)
	}
	filelock.Unlock(lf)
	// the pass after looks again at what it left, though its time is old
	res, err = PortCodexRollouts(context.Background(), home)
	if err != nil || len(res.Rewritten) != 1 {
		t.Fatalf("pass after: %+v, %v", res, err)
	}

	home2, path2 := portHome(t, lines, false)
	os.Remove(filepath.Join(home2, "thread-writer-locks"))
	orig2, _ := os.ReadFile(path2)
	if res, err := PortCodexRollouts(context.Background(), home2); err != nil || len(res.Rewritten) != 0 {
		t.Fatalf("no writer locks: %+v, %v", res, err)
	}
	if b, _ := os.ReadFile(path2); !bytes.Equal(b, orig2) {
		t.Fatal("written without Codex's writer locks")
	}
}

// A line is parsed only for magpie's item; text that only mentions
// "magpie1:" keeps its bytes, and a line that is one unreadable item of
// magpie's alone stays (leaving it out would move the lines after it).
func TestPortLineLeavesOthers(t *testing.T) {
	for _, l := range []string{
		`{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"magpie1:abc"}]}}` + "\n",
		`{"type":"response_item","payload":{"type":"compaction","encrypted_content":"magpie1:%%%"}}` + "\r\n",
		`{"type":"response_item","payload":{"type":"reasoning","encrypted_content":"magpie1:xyz"}}` + "\n",
		`not json magpie1:` + "\n",
	} {
		if got, ok := portLine([]byte(l)); ok || string(got) != l {
			t.Errorf("%q became %q", l, got)
		}
	}
	l := `{"type":"response_item","payload":{"type":"compaction_summary","encrypted_content":"` + codexsummary.Encrypted("x") + `"}}` + "\r\n"
	got, ok := portLine([]byte(l))
	if !ok || !bytes.HasSuffix(got, []byte("}\r\n")) || bytes.Contains(got, []byte("magpie1:")) {
		t.Errorf("compaction_summary: %q", got)
	}
}

// A reasoning item carrying Claude's block (claudeseal) is written as its
// summary, as the gateway sends it to OpenAI, in a response_item and in a
// compacted line's replacement_history; OpenAI's own encrypted_content and
// a mention of the mark in text keep their bytes.
func TestPortLineDropsClaudeBlocks(t *testing.T) {
	sealed := claudeseal.Seal([]byte(`{"type":"thinking","thinking":"hm","signature":"sig"}`))
	for _, l := range []string{
		`{"type":"response_item","payload":{"type":"reasoning","summary":[],"encrypted_content":"gAAAAopenai"}}` + "\n",
		`{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"` + sealed + `"}]}}` + "\n",
	} {
		if got, ok := portLine([]byte(l)); ok || string(got) != l {
			t.Errorf("%q became %q", l, got)
		}
	}
	for _, l := range []string{
		`{"type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"hm"}],"encrypted_content":"` + sealed + `"}}` + "\n",
		`{"type":"compacted","payload":{"message":"","replacement_history":[{"type":"reasoning","summary":[],"encrypted_content":"` + sealed + `"}]}}` + "\n",
	} {
		got, ok := portLine([]byte(l))
		if !ok || bytes.Contains(got, []byte(claudeseal.Mark)) || bytes.Contains(got, []byte("encrypted_content")) || !bytes.Contains(got, []byte(`"reasoning"`)) {
			t.Errorf("%q became %q", l, got)
		}
	}
}

// lineEnds is where each line of a rollout ends, newline included: the
// places Codex keeps by byte (history_base.end_byte_offset, the state
// database's turn and projection offsets).
func lineEnds(t *testing.T, path string) []int {
	t.Helper()
	in, err := openLines(path)
	if err != nil {
		t.Fatal(err)
	}
	defer in.Close()
	var buf bytes.Buffer
	buf.ReadFrom(in)
	var ends []int
	for i, c := range buf.Bytes() {
		if c == '\n' {
			ends = append(ends, i+1)
		}
	}
	return ends
}

// xybio (Discord): the Codex app wouldn't reopen threads ("invalid
// paginated history lineage for …: cutoff byte offset is past the source
// rollout"). A fork or a revert names the byte its parent's history ends
// at (session_meta.history_base), and Codex's state database keeps turns
// and how far it has read by byte too; a ported line, a summary message
// shorter than magpie's base64 item, made the parent shorter, so every
// such place moved or went past its end. A rollout written portable keeps
// every line where it was, and a fork's place in it stays a line's end.
func TestPortCodexRolloutsKeepsBytePlaces(t *testing.T) {
	for _, zst := range []bool{false, true} {
		t.Run(map[bool]string{false: "plain", true: "zst"}[zst], func(t *testing.T) {
			summary := strings.Repeat("Progress: build fixed in internal/x; next the suite. ", 80)
			lines := portRolloutLines(summary)
			lines[0] = strings.Replace(lines[0], `"model_provider":"openai"`, `"model_provider":"openai","history_mode":"paginated"`, 1)
			sealed := claudeseal.Seal([]byte(`{"type":"thinking","thinking":"hm","signature":"` + strings.Repeat("s", 600) + `"}`))
			lines = append(lines, `{"timestamp":"2026-10-20T09:00:06.000Z","type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"hm"}],"encrypted_content":"`+sealed+`"}}`)
			home, path := portHome(t, lines, zst)
			before := lineEnds(t, path)

			// a fork of the thread made at its end, as the Codex app makes
			// one from where it has read to
			child := "01a1430d-e3a9-7a72-a466-071d2e3dd46b"
			cpath := filepath.Join(filepath.Dir(path), "rollout-2026-10-20T18-00-00-"+child+".jsonl")
			meta := `{"timestamp":"2026-10-20T10:00:00.000Z","type":"session_meta","payload":{"id":"` + child + `","history_mode":"paginated","history_base":{"thread_id":"` + portThread + `","end_ordinal_exclusive":` + strconv.Itoa(len(lines)) + `,"end_byte_offset":` + strconv.Itoa(before[len(before)-1]) + `}}}` + "\n"
			if err := os.WriteFile(cpath, []byte(meta), 0o600); err != nil {
				t.Fatal(err)
			}

			res, err := PortCodexRollouts(context.Background(), home)
			if err != nil || len(res.Rewritten) != 1 || res.Rewritten[0] != path {
				t.Fatalf("pass: %+v, %v", res, err)
			}
			if after := lineEnds(t, path); !reflect.DeepEqual(after, before) {
				t.Fatalf("lines end at %v, were at %v", after, before)
			}
			got := readRollout(t, path)
			want, _ := json.Marshal(codexsummary.UserMessage(codexsummary.Prefix + "\n" + summary))
			var wantAny any
			json.Unmarshal(want, &wantAny)
			if p := decode(t, got[2])["payload"]; !reflect.DeepEqual(p, wantAny) {
				t.Errorf("not written portable: %s", got[2])
			}
			for i, l := range got {
				decode(t, l) // padded lines still read as JSON
				if i != 1 && (strings.Contains(l, codexsummary.Mark) || strings.Contains(l, claudeseal.Mark)) {
					t.Errorf("line %d still has magpie's item: %.200s", i, l)
				}
			}
			if b, _ := os.ReadFile(cpath); string(b) != meta {
				t.Error("the fork's rollout changed")
			}
		})
	}
}

// A line whose portable form is longer (a short summary, under Codex's
// prefix) can't keep its length: in a paginated rollout it is left as it
// was, never moving what follows; in a legacy one, where Codex keeps no
// place by byte, it is written as before.
func TestPortCodexRolloutsLongerLine(t *testing.T) {
	lines := portRolloutLines("short")
	lines[0] = strings.Replace(lines[0], `"model_provider":"openai"`, `"model_provider":"openai","history_mode":"paginated"`, 1)
	home, path := portHome(t, lines, false)
	before := lineEnds(t, path)
	if _, err := PortCodexRollouts(context.Background(), home); err != nil {
		t.Fatal(err)
	}
	if after := lineEnds(t, path); !reflect.DeepEqual(after, before) {
		t.Fatalf("lines end at %v, were at %v", after, before)
	}
	got := readRollout(t, path)
	if got[2] != lines[2] {
		t.Errorf("a longer line was written: %s", got[2])
	}
	if strings.Contains(got[4], "magpie1:%%%") {
		t.Errorf("the shorter one wasn't: %s", got[4])
	}
}

// A fork made before magpie kept its parent's lines whole names a byte
// the shorter parent doesn't end a line at, or doesn't have (xybio's
// thread). The pass after sets it again, once, from the ordinal the fork
// names, keeping the fork's first line as long as it was; a fork whose
// place is right keeps its bytes.
func TestPortCodexRolloutsRepairsForks(t *testing.T) {
	parent := []string{
		`{"timestamp":"2026-10-20T07:05:20.118Z","ordinal":0,"type":"session_meta","payload":{"id":"` + portThread + `","history_mode":"paginated","model_provider":"openai"}}`,
		`{"timestamp":"2026-10-20T07:05:31.002Z","ordinal":1,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"fix it"}]}}`,
		`{"timestamp":"2026-10-20T07:05:40.002Z","ordinal":2,"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}`,
	}
	home, path := portHome(t, parent, false)
	ends := lineEnds(t, path)
	dir := filepath.Dir(path)
	fork := func(id string, ord, at int) string {
		p := filepath.Join(dir, "rollout-2026-10-20T18-00-00-"+id+".jsonl")
		b := `{"timestamp":"2026-10-20T10:00:00.000Z","ordinal":` + strconv.Itoa(ord) + `,"type":"session_meta","payload":{"id":"` + id + `","history_mode":"paginated","history_base":{"thread_id":"` + portThread + `","end_ordinal_exclusive":` + strconv.Itoa(ord) + `,"end_byte_offset":` + strconv.Itoa(at) + `}}}` + "\n" +
			`{"timestamp":"2026-10-20T10:00:01.000Z","ordinal":` + strconv.Itoa(ord+1) + `,"type":"event_msg","payload":{"type":"task_started"}}` + "\n"
		if err := os.WriteFile(p, []byte(b), 0o600); err != nil {
			t.Fatal(err)
		}
		return p
	}
	// made when the parent was longer, at its end then
	broken := fork("01a1430d-e3a9-7a72-a466-071d2e3dd46b", 3, 14362)
	right := fork("01a1430d-ffff-7a72-a466-071d2e3dd46b", 2, ends[1])
	was, _ := os.ReadFile(broken)
	rightWas, _ := os.ReadFile(right)

	res, err := PortCodexRollouts(context.Background(), home)
	if err != nil || len(res.Repaired) != 1 || res.Repaired[0] != broken {
		t.Fatalf("pass: %+v, %v", res, err)
	}
	now, _ := os.ReadFile(broken)
	if len(now) != len(was) {
		t.Fatalf("%d bytes, was %d", len(now), len(was))
	}
	first := decode(t, readRollout(t, broken)[0])
	if at := first["payload"].(map[string]any)["history_base"].(map[string]any)["end_byte_offset"]; at != float64(ends[2]) {
		t.Errorf("end_byte_offset %v, want %d", at, ends[2])
	}
	if b, _ := os.ReadFile(right); !bytes.Equal(b, rightWas) {
		t.Error("a fork whose place was right changed")
	}
	// looked at once
	os.WriteFile(broken, was, 0o600)
	if res, err := PortCodexRollouts(context.Background(), home); err != nil || len(res.Repaired) != 0 {
		t.Fatalf("second pass: %+v, %v", res, err)
	}
}
