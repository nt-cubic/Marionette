package sessions

import (
	"fmt"
	"path/filepath"
	"testing"
	"time"
)

// A cached file's calls are read into a slice made for them, not grown
// call by call: append's doublings cost a warm start ~500 MB of garbage
// across every cached file, all of it in its first seconds.
func TestLoadedCallsAreSizedOnce(t *testing.T) {
	d := setupCalls(t)
	path := filepath.Join(d.claude, "projects", "p", "s.jsonl")
	st := &callFile{Path: path, Agent: "claude", Began: map[string]time.Time{}}
	add := func(n int) {
		for range n {
			i := len(st.Calls)
			st.Calls = append(st.Calls, Call{Time: time.Unix(int64(i), 0), Agent: "claude", Session: "s", Model: "claude-opus-5-5", Msg: fmt.Sprint("msg", i)})
		}
	}
	add(3000)
	writeCalls(st, 0, false)
	got := loadCalls(file{path: path, agent: "claude"})
	if got == nil || len(got.Calls) != 3000 || got.Calls[2999].Msg != "msg2999" || got.Calls[2999].File != path || got.Msgs["msg2999"] != 2999 {
		t.Fatal("cached calls not read back")
	}
	// made once, to the allocator's size class; grown by append it ends
	// a growth step past
	if cap(got.Calls)-len(got.Calls) > len(got.Calls)/100 {
		t.Fatalf("3000 calls read into a slice of %d", cap(got.Calls))
	}
	// a later frame appended to the file is read after the first, in order
	add(500)
	writeCalls(st, 3000, true)
	got = loadCalls(file{path: path, agent: "claude"})
	if got == nil || len(got.Calls) != 3500 || got.Calls[3499].Msg != "msg3499" || got.Calls[2999].Msg != "msg2999" {
		t.Fatal("appended calls not read back")
	}
}
