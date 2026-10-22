package pii

import (
	"errors"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/redact"
)

func person(s, e int) redact.Found { return redact.Found{Start: s, End: e, Label: "private_person"} }

// Short texts go to pf-serve joined, in one request, and what it finds
// comes back to each text at that text's own offsets; a text seen before
// isn't asked about again.
func TestFindJoinsTextsAndMapsBack(t *testing.T) {
	log := fakeInstall(t, "ok", false)
	texts := []string{"hi Alice", "nothing here", "Bob, Alice and Alice"}
	got, err := Find(texts)
	if err != nil {
		t.Fatal(err)
	}
	want := [][]redact.Found{{person(3, 8)}, nil, {person(5, 10), person(15, 20)}}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("Find = %v, want %v", got, want)
	}
	if reqs := fakeLines(t, log, "request"); len(reqs) != 1 || !strings.HasPrefix(reqs[0], "request 1 ") {
		t.Fatalf("requests = %q, want one of one joined text", reqs)
	}
	again, err := Find([]string{"Bob, Alice and Alice", "new Alice"})
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(again, [][]redact.Found{want[2], {person(4, 9)}}) {
		t.Fatalf("Find again = %v", again)
	}
	reqs := fakeLines(t, log, "request")
	if len(reqs) != 2 || strings.Contains(reqs[1], "Bob") {
		t.Fatalf("requests = %q, want the second to ask about the new text alone", reqs)
	}
	if n := len(fakeLines(t, log, "start")); n != 1 {
		t.Fatalf("pf-serve started %d times, want once", n)
	}
	if st := Now(); !st.Running || !st.GPU || st.Device != "MTL0" || st.LoadMS != 7 {
		t.Fatalf("Now = %+v", st)
	}
}

func TestJoinSplitsAtChunkSize(t *testing.T) {
	long := strings.Repeat("x", chunkSize)
	texts := []string{"a", "b", long, "c"}
	chunks, parts := join(texts, []int{0, 1, 2, 3})
	if len(chunks) != 3 || chunks[0] != "a"+sep+"b" || chunks[1] != long || chunks[2] != "c" {
		t.Fatalf("chunks = %d: %.20q", len(chunks), chunks)
	}
	if !reflect.DeepEqual(parts, [][]part{{{0, 0}, {1, 1 + len(sep)}}, {{2, 0}}, {{3, 0}}}) {
		t.Fatalf("parts = %v", parts)
	}
}

// A span the model ran across the separator into the next text is cut at
// each text's end.
func TestWithinCutsSpansAtTheTextsEnds(t *testing.T) {
	fs := []redact.Found{person(2, 9), {Start: 0, End: 2, Label: "secret"}}
	if got := within(fs, 0, 5); !reflect.DeepEqual(got, []redact.Found{person(2, 5), {Start: 0, End: 2, Label: "secret"}}) {
		t.Fatalf("first text: %v", got)
	}
	if got := within(fs, 7, 12); !reflect.DeepEqual(got, []redact.Found{person(0, 2)}) {
		t.Fatalf("second text: %v", got)
	}
	if got := within(fs, 10, 12); got != nil {
		t.Fatalf("third text: %v", got)
	}
}

// pf-serve that dies on the GPU is started again on the CPU, which the
// request then gets its answer from, and stays there; the status says why.
func TestGPUDeathMovesToCPU(t *testing.T) {
	log := fakeInstall(t, "gpudies", false)
	got, err := Find([]string{"hi Alice"})
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(got, [][]redact.Found{{person(3, 8)}}) {
		t.Fatalf("Find = %v", got)
	}
	if starts := fakeLines(t, log, "start"); !reflect.DeepEqual(starts, []string{"start pf-serve MTL0", "start pf-serve CPU"}) {
		t.Fatalf("starts = %q", starts)
	}
	st := Now()
	if st.GPU || st.Device != "CPU" || !strings.Contains(st.GPUError, "device lost") {
		t.Fatalf("Now = %+v, want on the CPU saying the GPU was lost", st)
	}
	Stop()
	if _, err := Find([]string{"Alice again"}); err != nil {
		t.Fatal(err)
	}
	if starts := fakeLines(t, log, "start"); len(starts) != 3 || starts[2] != "start pf-serve CPU" {
		t.Fatalf("starts = %q, want the CPU after a stop too", starts)
	}
}

// A pf-serve-vulkan that can't start (no Vulkan loader) leaves it to
// pf-serve.
func TestVulkanThatWontStartFallsBack(t *testing.T) {
	log := fakeInstall(t, "novulkan", true)
	if _, err := Find([]string{"hi Alice"}); err != nil {
		t.Fatal(err)
	}
	if starts := fakeLines(t, log, "start"); !reflect.DeepEqual(starts, []string{"start pf-serve-vulkan MTL0", "start pf-serve MTL0"}) {
		t.Fatalf("starts = %q", starts)
	}
}

// A pf-serve that doesn't answer is killed, and the request fails rather
// than wait for good.
func TestHungServeIsKilled(t *testing.T) {
	log := fakeInstall(t, "hang", false)
	was := classifyTimeout
	classifyTimeout = func(int) time.Duration { return 300 * time.Millisecond }
	t.Cleanup(func() { classifyTimeout = was })
	_, err := Find([]string{"hi Alice"})
	if err == nil || !strings.Contains(err.Error(), "failed") {
		t.Fatalf("err = %v, want the model failed", err)
	}
	if n := len(fakeLines(t, log, "start")); n != 2 {
		t.Fatalf("started %d times, want 2 (one try again)", n)
	}
	if Now().Running {
		t.Fatal("a hung pf-serve is still taken for running")
	}
}

func TestServeThatCantLoadSaysWhy(t *testing.T) {
	fakeInstall(t, "noload", false)
	_, err := Find([]string{"hi Alice"})
	if err == nil || !strings.Contains(err.Error(), "can't load noload") {
		t.Fatalf("err = %v", err)
	}
}

// Warm starts pf-serve before a request asks, and one that can't start
// says why in Now; a request that is answered clears it.
func TestWarm(t *testing.T) {
	wait := func(what string, ok func(Status) bool) {
		for deadline := time.Now().Add(10 * time.Second); !ok(Now()); time.Sleep(10 * time.Millisecond) {
			if time.Now().After(deadline) {
				t.Fatalf("%s: %+v", what, Now())
			}
		}
	}
	fakeInstall(t, "noload", false)
	Warm()
	wait("no failure", func(s Status) bool { return strings.Contains(s.Failed, "can't load noload") })

	log := fakeInstall(t, "ok", false)
	srv.failed = "before"
	Warm()
	wait("not running", func(s Status) bool { return s.Running && s.Device == "MTL0" && s.Failed == "" })
	if n := len(fakeLines(t, log, "start")); n != 1 {
		t.Fatalf("started %d times, want 1", n)
	}
}

func TestNotInstalled(t *testing.T) {
	fakeInstall(t, "ok", false)
	modelSize = 5 // the file on disk isn't the model's size
	if _, err := Find([]string{"hi Alice"}); !errors.Is(err, ErrNotInstalled) {
		t.Fatalf("err = %v, want ErrNotInstalled", err)
	}
}

// pf-serve with no request for idleAfter is ended, and started again by
// the next one.
func TestIdleServeEnds(t *testing.T) {
	log := fakeInstall(t, "ok", false)
	idleAfter = 50 * time.Millisecond
	if _, err := Find([]string{"hi Alice"}); err != nil {
		t.Fatal(err)
	}
	for deadline := time.Now().Add(10 * time.Second); Now().Running; time.Sleep(10 * time.Millisecond) {
		if time.Now().After(deadline) {
			t.Fatal("pf-serve still runs")
		}
	}
	if _, err := Find([]string{"Alice, later"}); err != nil {
		t.Fatal(err)
	}
	if n := len(fakeLines(t, log, "start")); n != 2 {
		t.Fatalf("started %d times, want 2", n)
	}
}

func TestCacheKeepsTheLatest(t *testing.T) {
	c := newLRU(2)
	k := func(b byte) [32]byte { return [32]byte{b} }
	c.put(k(1), []redact.Found{person(0, 1)})
	c.put(k(2), nil)
	c.get(k(1))
	c.put(k(3), nil)
	if _, ok := c.get(k(2)); ok {
		t.Fatal("the least recent entry was kept")
	}
	if fs, ok := c.get(k(1)); !ok || len(fs) != 1 {
		t.Fatal("an entry read lately was dropped")
	}
}
