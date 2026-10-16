package gwmw

import (
	"net/http"
	"slices"
	"strings"
	"sync/atomic"
	"testing"
)

type agent string

func (agent) Begin(Info) Run { return nil }

type masker string

func (m masker) JSON(b []byte) ([]byte, int)                              { return b, 0 }
func (m masker) Text(s string) (string, int)                              { return string(m) + s, 1 }
func (masker) Restore(s string, _ bool) string                            { return s }
func (masker) Writer(w http.ResponseWriter) (http.ResponseWriter, func()) { return w, func() {} }

// Agent middlewares run in the order they were registered; one removed is
// gone, and removing it again takes nothing else.
func TestAgentsInOrder(t *testing.T) {
	ra := Use(agent("a"))
	rb := Use(agent("b"))
	defer rb()
	if got := Agents(); !slices.Equal(got, []Agent{agent("a"), agent("b")}) {
		t.Fatalf("agents %v", got)
	}
	ra()
	ra()
	if got := Agents(); !slices.Equal(got, []Agent{agent("b")}) {
		t.Fatalf("after a's removal %v", got)
	}
}

// The last masker registered serves until it is removed; with none, a
// request masking is wanted for is refused (fail closed), and one it isn't
// wanted for goes as it is.
func TestMaskingFailsClosed(t *testing.T) {
	if m, err := Masking(false); m != nil || err != nil {
		t.Fatalf("not wanted: %v %v", m, err)
	}
	if _, err := Masking(true); err != ErrUnmasked {
		t.Fatalf("wanted, none served: %v", err)
	}
	r1 := Mask(masker("1"))
	defer r1()
	r2 := Mask(masker("2"))
	if m, err := Masking(true); err != nil || m != masker("2") {
		t.Fatalf("stacked: %v %v", m, err)
	}
	r2()
	if m, err := Masking(true); err != nil || m != masker("1") {
		t.Fatalf("after the last's removal: %v %v", m, err)
	}
}

// With no masker served, the refresh is asked once, and a masker it serves
// is used; another within a second isn't asked again, and is refused.
func TestServedRefreshesOnce(t *testing.T) {
	ForgetRefreshForTest()
	var asked atomic.Int32
	var remove func()
	stop := Refresh(func() { asked.Add(1); remove = Mask(masker("r")) })
	defer stop()
	if m, ok := Served(); !ok || m != masker("r") || asked.Load() != 1 {
		t.Fatalf("refreshed: %v %v asked %d", m, ok, asked.Load())
	}
	remove()
	if _, ok := Served(); ok || asked.Load() != 1 {
		t.Fatalf("within a second: asked %d", asked.Load())
	}
	ForgetRefreshForTest()
	stop()
	if _, ok := Served(); ok || asked.Load() != 1 {
		t.Fatalf("after stop: asked %d", asked.Load())
	}
}

// With no agent middleware, a refresh is asked only when want says some is
// wanted, at most once a second.
func TestAgentsWantedRefreshesWhenWanted(t *testing.T) {
	ForgetRefreshForTest()
	var asked atomic.Int32
	var remove func()
	stop := Refresh(func() { asked.Add(1); remove = Use(agent("r")) })
	defer stop()
	if a := AgentsWanted(func() bool { return false }); len(a) != 0 || asked.Load() != 0 {
		t.Fatalf("not wanted: %v asked %d", a, asked.Load())
	}
	ForgetRefreshForTest()
	if a := AgentsWanted(func() bool { return true }); len(a) != 1 || asked.Load() != 1 {
		t.Fatalf("wanted: %v asked %d", a, asked.Load())
	}
	remove()
	if a := AgentsWanted(func() bool { return true }); len(a) != 0 || asked.Load() != 1 {
		t.Fatalf("within a second: %v asked %d", a, asked.Load())
	}
}

// A masker that panics is an error, never the value unmasked.
func TestSafelyTurnsAPanicIntoAnError(t *testing.T) {
	out, err := Safely(func() string { panic("boom") })
	if err == nil || out != "" || !strings.Contains(err.Error(), "nothing was sent") {
		t.Fatalf("%q %v", out, err)
	}
	if out, err := Safely(func() string { return "ok" }); err != nil || out != "ok" {
		t.Fatalf("%q %v", out, err)
	}
}
