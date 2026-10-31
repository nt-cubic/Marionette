package gui

import (
	"bytes"
	"encoding/json"
	"net/http/httptest"
	"testing"

	"github.com/yetone/magpie/internal/sessions"
)

// statsAnswer writes the stats answer a day at a time, and the answer is
// byte for byte what the Encoder of the whole gave: the page's tag rests
// on those bytes. A field added to Stats shows here as a difference.
func TestStatsAnswerWritesWhatEncodeDid(t *testing.T) {
	day := func(date string) sessions.Day {
		return sessions.Day{Date: date,
			Usage:  []sessions.Usage{{Agent: "codex", Cwd: "/a<b>&c", Model: "gpt x", Tokens: sessions.Tokens{Input: 3, CacheWrite1h: 1}, Cost: 1e21, Priced: true}},
			Active: []sessions.Active{{Agent: "claude", Cwd: "引号\"", Seconds: 61, Hours: []int64{1, 2}}}}
	}
	for _, c := range []struct {
		st     sessions.Stats
		agents map[string]string
	}{
		{sessions.Stats{}, nil},
		{sessions.Stats{From: "2026-10-01", To: "2026-10-30", Days: []sessions.Day{}, Keys: []string{}}, map[string]string{}},
		{sessions.Stats{From: "<", To: "&", Days: []sessions.Day{day("2026-10-29"), {Date: "2026-10-30"}, day("x")}, Keys: []string{"codex:1", "a<b"}, Sessions: []sessions.Summary{{Key: "hidden"}}}, map[string]string{"codex": "Codex", "z": "<Z>"}},
	} {
		var want bytes.Buffer
		if err := json.NewEncoder(&want).Encode(struct {
			sessions.Stats
			Agents map[string]string `json:"agents"`
		}{c.st, c.agents}); err != nil {
			t.Fatal(err)
		}
		var got bytes.Buffer
		if err := statsAnswer(&got, c.st, c.agents); err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(got.Bytes(), want.Bytes()) {
			t.Fatalf("\n got %s\nwant %s", got.Bytes(), want.Bytes())
		}
	}
}

// A long answer is written into a buffer kept from the last, not one grown
// from nothing each poll.
func TestAnswerBufferKeptForTheNext(t *testing.T) {
	answerBuffers.Lock()
	answerBuffers.free = nil
	answerBuffers.Unlock()
	big := bytes.Repeat([]byte("x"), 1<<20)
	w := httptest.NewRecorder()
	writeSame(w, httptest.NewRequest("GET", "/x", nil), func(b *bytes.Buffer) error {
		b.Write(big)
		return nil
	})
	if !bytes.Equal(w.Body.Bytes(), big) {
		t.Fatalf("answer of %d bytes, want %d", w.Body.Len(), len(big))
	}
	b := answerBuffer()
	if b.Len() != 0 || b.Cap() < len(big) {
		t.Fatalf("next buffer holds %d, room for %d: not the last answer's", b.Len(), b.Cap())
	}
	keepAnswer(b)
	rows := []rowsBufRow{{Name: string(big)}}
	writeJSONRows(httptest.NewRecorder(), httptest.NewRequest("GET", "/x", nil), "kept-test", rows, struct{}{})
	answerBuffers.Lock()
	n := len(answerBuffers.free)
	answerBuffers.Unlock()
	if n == 0 {
		t.Fatal("writeJSONRows kept no buffer for the next answer")
	}
}
