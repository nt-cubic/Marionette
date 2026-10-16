package usage

import (
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

// a call's bodies go only to an observer (#538), never to usage.jsonl
func TestBodiesStayOutOfTheLog(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(t.TempDir(), "config"))
	Append(Record{Time: time.Now(), Model: "m1", Status: 200, BodyIn: "PRIVATE-PROMPT", BodyOut: "PRIVATE-REPLY"})
	b, err := os.ReadFile(Path())
	if err != nil || !strings.Contains(string(b), `"model":"m1"`) || strings.Contains(string(b), "PRIVATE-") {
		t.Fatalf("usage.jsonl: %s %v", b, err)
	}
}

type fakeObserver struct {
	mu            sync.Mutex
	bodies        Bodies
	serves, stops int
	rows          []Record
}

func (f *fakeObserver) Serve() func() {
	f.mu.Lock()
	f.serves++
	f.mu.Unlock()
	return func() { f.mu.Lock(); f.stops++; f.mu.Unlock() }
}
func (f *fakeObserver) Bodies() Bodies      { return f.bodies }
func (f *fakeObserver) Begin(Request) Watch { return nil }
func (f *fakeObserver) Ledger(r Record) {
	f.mu.Lock()
	f.rows = append(f.rows, r)
	f.mu.Unlock()
}
func (f *fakeObserver) counts() (int, int, int) {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.serves, f.stops, len(f.rows)
}

// An observer serves only while the gateway does, whether it is registered
// before or after the gateway starts, and is stopped once by either end.
func TestObserverServesWithTheGateway(t *testing.T) {
	early := &fakeObserver{}
	removeEarly := Observe(early)
	defer removeEarly()
	if s, _, _ := early.counts(); s != 0 {
		t.Fatal("served before the gateway did")
	}
	stop := ServeObservers()
	inner := ServeObservers() // a second gateway in the process
	late := &fakeObserver{}
	removeLate := Observe(late)
	if s, _, _ := early.counts(); s != 1 {
		t.Fatalf("early serves = %d, want 1", s)
	}
	if s, _, _ := late.counts(); s != 1 {
		t.Fatalf("late serves = %d, want 1", s)
	}
	removeLate()
	removeLate()
	if _, st, _ := late.counts(); st != 1 {
		t.Fatalf("late stops = %d, want 1", st)
	}
	inner()
	if _, st, _ := early.counts(); st != 0 {
		t.Fatal("stopped while a gateway still serves")
	}
	stop()
	stop()
	if s, st, _ := early.counts(); s != 1 || st != 1 {
		t.Fatalf("early serves, stops = %d, %d; want 1, 1", s, st)
	}
}

// Every ledger row reaches each observer; the bodies wanted are the most any
// one wants, and none registered wants none.
func TestObserverLedgerAndBodies(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(t.TempDir(), "config"))
	if got := ObservedBodies(); got != NoBodies {
		t.Fatalf("no observer: bodies = %d", got)
	}
	a, b := &fakeObserver{bodies: CapturedBodies}, &fakeObserver{bodies: WholeBodies}
	removeB, removeA := Observe(b), Observe(a) // the most wanted is not the last one
	if got := ObservedBodies(); got != WholeBodies {
		t.Fatalf("bodies = %d, want whole", got)
	}
	Append(Record{Time: time.Now(), Model: "m1", Status: 200})
	removeB()
	if got := ObservedBodies(); got != CapturedBodies {
		t.Fatalf("bodies = %d, want captured", got)
	}
	Append(Record{Time: time.Now(), Model: "m2", Status: 200})
	removeA()
	Append(Record{Time: time.Now(), Model: "m3", Status: 200})
	if _, _, n := a.counts(); n != 2 {
		t.Fatalf("a saw %d rows, want 2", n)
	}
	if _, _, n := b.counts(); n != 1 {
		t.Fatalf("b saw %d rows, want 1", n)
	}
}
