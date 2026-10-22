package gateway

import (
	"bytes"
	"context"
	"net/http"
	"net/url"
	"strings"
	"testing"
	"time"
	"unsafe"

	"github.com/yetone/magpie/internal/provider"
)

// parseMemoBody is a Claude Code conversation in a folder, long enough to
// be held, with a tool result in it.
func parseMemoBody(tag string) []byte {
	return []byte(`{"model":"` + tag + `","system":" - Primary working directory: /work/` + tag + `\n","messages":[` +
		`{"role":"user","content":"fix it"},` +
		`{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"path":"a"}}]},` +
		`{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"` + strings.Repeat("line of a file\\n", 4000) + `"}]}]}`)
}

// The turn count (turnOf) and the folder (dirOf, tellWorkPlace) of a
// request being served are read from one parse of its body, not one each:
// each decoded every message and tool output of a long conversation again
// (yg6888 on X: ~1 GB). What they say is what a parse of their own says.
func TestReadersOfAHeldBodyParseItOnce(t *testing.T) {
	ctx, done := context.WithCancel(context.Background())
	defer done()
	body := parseMemoBody(t.Name())
	holdBody(ctx, body)

	want, err := parse(provider.Anthropic, bytes.Clone(body))
	if err != nil {
		t.Fatal(err)
	}
	wantDir := "/work/" + t.Name()
	if workDir(want) != wantDir {
		t.Fatalf("the fixture's folder reads %q", workDir(want))
	}
	wantTurn, wantWithin := turnIn(want)
	var turn int
	var within bool
	var dir string
	h := http.Header{}
	readers := allocated(func() {
		turn, within = turnOf(provider.Anthropic, body)
		dir = dirOf(provider.Anthropic, body)()
		tellWorkPlace(h, provider.Anthropic, body)
	})
	if turn != wantTurn || within != wantWithin || turn != 1 || !within {
		t.Fatalf("turnOf = %d %v, a parse of its own says %d %v", turn, within, wantTurn, wantWithin)
	}
	if dir != wantDir {
		t.Fatalf("dirOf = %q, want %q", dir, wantDir)
	}
	if got, _ := url.PathUnescape(h.Get(provider.WorkDirHeader)); got != wantDir {
		t.Fatalf("tellWorkPlace said %q, want %q", got, wantDir)
	}
	whole := allocated(func() { parse(provider.Anthropic, body) })
	// Under -race, sync.Pool drops a quarter of what is put in it, and
	// workDirIn's regexps take their matchers (~37 KB for this body) from
	// one, so the readers' bytes swing by whole matchers: 1.3× to 2.1×
	// a parse over five tries, against a parse's steady 135 KB.
	if !raceOn && readers > whole*3/2 {
		t.Fatalf("turnOf, dirOf and tellWorkPlace allocate %d bytes on a held body, a parse %d: they parse it each", readers, whole)
	}
	// and again, nothing: what was read is kept while the body is held
	if again := allocated(func() { turnOf(provider.Anthropic, body); dirOf(provider.Anthropic, body)() }); again > whole/8 {
		t.Fatalf("a held body's second readers allocate %d bytes, a parse %d", again, whole)
	}

	// another protocol's reading of the same bytes is its own
	if got, want := factsOf(provider.Chat, body), readFacts(provider.Chat, body); got != want {
		t.Fatalf("a held body read as Chat says %+v, a parse of its own %+v", got, want)
	}
	if got := factsOf(provider.Anthropic, body); got.dir != wantDir || got.turn != 1 {
		t.Fatalf("read as Messages again after Chat: %+v", got)
	}
	// a body not held, of the same bytes, is parsed afresh: nothing says
	// its memory isn't written over
	clone := bytes.Clone(body)
	if fresh := allocated(func() { factsOf(provider.Anthropic, clone) }); fresh < whole/2 {
		t.Fatalf("a body not held read %d bytes, a parse %d: it got a held body's facts", fresh, whole)
	}
}

// What is kept goes when the request ends, so the body's memory, given to
// another body, never reads as this one, and a body no longer held is not
// listed again.
func TestHeldFactsGoWithTheirBody(t *testing.T) {
	ctx, done := context.WithCancel(context.Background())
	body := parseMemoBody(t.Name())
	holdBody(ctx, body)
	factsOf(provider.Anthropic, body)
	key := unsafe.SliceData(body)
	listed := func() bool {
		heldBodies.Lock()
		defer heldBodies.Unlock()
		_, ok := heldBodies.facts[key]
		return ok
	}
	if !listed() {
		t.Fatal("a held body's facts were not kept for its next reader")
	}
	done()
	for deadline := time.Now().Add(5 * time.Second); listed(); {
		if time.Now().After(deadline) {
			t.Fatal("a body's facts are still listed after its request ended")
		}
		time.Sleep(time.Millisecond)
	}
	if f := factsOf(provider.Anthropic, body); f.dir != "/work/"+t.Name() {
		t.Fatalf("a released body reads %+v", f)
	}
	if listed() {
		t.Fatal("a body no longer held got its facts listed")
	}
	// a part of a held body is not the body
	ctx2, done2 := context.WithCancel(context.Background())
	defer done2()
	holdBody(ctx2, body)
	factsOf(provider.Anthropic, body[:len(body)-1])
	if listed() {
		t.Fatal("a held body's prefix got facts listed under the body")
	}
}
