package gateway

import (
	"io"
	"net/http"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// hits counts the requests each upstream took, and fails them all as out
// of credit: a failure that passes the request on, with no retry.
type hits struct {
	sync.Mutex
	n map[string]int
}

func (h *hits) on(id string) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		h.Lock()
		h.n[id]++
		h.Unlock()
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusPaymentRequired)
		io.WriteString(w, `{"error":{"message":"insufficient balance"}}`)
	})
}

func (h *hits) got() map[string]int {
	h.Lock()
	defer h.Unlock()
	out := map[string]int{}
	for k, v := range h.n {
		out[k] = v
	}
	return out
}

// restFor rests each named member of group's candidates until the time
// given, as a failure before would have.
func restFor(t *testing.T, s *Server, group string, until map[string]time.Time) {
	t.Helper()
	g, ms, ok := provider.FindGroup("group/" + group)
	if !ok {
		t.Fatalf("no group %s", group)
	}
	cs, _ := s.planGroup(g.Live(), ms, provider.Chat)
	restingUntil.Lock()
	defer restingUntil.Unlock()
	for _, c := range cs {
		if u, ok := until[c.p.ID]; ok {
			restingUntil.m[c.restKey()] = u
			restingUntil.note[c.restKey()] = Rest{Why: failOther, Status: 500, Key: c.restKey()}
		}
	}
}

// #1799: a group whose members are resting after failures doesn't send
// each request to every one of them in turn. Of those resting, only the
// one whose rest ends soonest is asked; a ready member is asked as ever,
// first.
func TestGroupAsksOneRestingMemberNotEach(t *testing.T) {
	fresh(t)
	h := &hits{n: map[string]int{}}
	for _, id := range []string{"a", "b", "c", "d"} {
		serveOn(t, id, "k-"+id, []string{"m"}, h.on(id))
	}
	if err := provider.SaveGroup(provider.Group{Name: "Down", Members: []string{"a/m", "b/m", "c/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "Half", Members: []string{"a/m", "b/m", "d/m", "c/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	now := time.Now()
	// minutes apart, so which is back soonest doesn't hang on the clock
	restFor(t, s, "down", map[string]time.Time{"a": now.Add(9 * time.Minute), "b": now.Add(3 * time.Minute), "c": now.Add(6 * time.Minute)})

	code, body := postAs(t, s, "", `{"model":"group/down","messages":[{"role":"user","content":"hi"}]}`)
	if code < 400 {
		t.Fatalf("every member fails, answered %d %s", code, body)
	}
	if got := h.got(); got["a"] != 0 || got["c"] != 0 || got["b"] != 1 {
		t.Fatalf("resting members asked %v, want b alone, once", got)
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	if len(r.Order) != 3 || len(r.Tries) != 1 || r.Tries[0].ID != "b" {
		t.Fatalf("trace: order %d, tries %+v", len(r.Order), r.Tries)
	}
	for _, w := range r.Order {
		if w.Rest == nil {
			t.Fatalf("the trace shows %s not resting: %+v", w.Provider, r.Order)
		}
	}

	// a member not resting is asked first, then the resting one back
	// soonest: b rests longer now that it failed again
	restFor(t, s, "half", map[string]time.Time{"a": now.Add(9 * time.Minute), "b": now.Add(8 * time.Minute), "c": now.Add(2 * time.Minute)})
	h.n = map[string]int{}
	if code, body := postAs(t, s, "", `{"model":"group/half","messages":[{"role":"user","content":"hi"}]}`); code < 400 {
		t.Fatalf("every member fails, answered %d %s", code, body)
	}
	if got := h.got(); got["d"] != 1 || got["c"] != 1 || got["a"] != 0 || got["b"] != 0 {
		t.Fatalf("asked %v, want d then c", got)
	}
}

// A lone member resting is still asked: the request has nobody else, and
// gets the vendor's own answer.
func TestLoneRestingMemberStillAsked(t *testing.T) {
	fresh(t)
	h := &hits{n: map[string]int{}}
	serveOn(t, "a", "k-a", []string{"m"}, h.on("a"))
	if err := provider.SaveGroup(provider.Group{Name: "One", Members: []string{"a/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	restFor(t, s, "one", map[string]time.Time{"a": time.Now().Add(5 * time.Minute)})
	code, body := postAs(t, s, "", `{"model":"group/one","messages":[{"role":"user","content":"hi"}]}`)
	if code != http.StatusPaymentRequired || !strings.Contains(body, "insufficient balance") {
		t.Fatalf("lone member: %d %s", code, body)
	}
	if got := h.got(); got["a"] != 1 {
		t.Fatalf("lone resting member asked %v", got)
	}
}

// The embeddings path goes over a group as a chat does, and asks one
// resting member too.
func TestRetrievalGroupAsksOneRestingMember(t *testing.T) {
	fresh(t)
	h := &hits{n: map[string]int{}}
	for _, id := range []string{"a", "b", "c"} {
		serveOn(t, id, "k-"+id, []string{"e"}, h.on(id))
	}
	if err := provider.SaveGroup(provider.Group{Name: "Emb", Members: []string{"a/e", "b/e", "c/e"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	now := time.Now()
	restFor(t, s, "emb", map[string]time.Time{"a": now.Add(9 * time.Minute), "b": now.Add(6 * time.Minute), "c": now.Add(3 * time.Minute)})
	if code, raw := postRetrieval(t, s, "/v1/embeddings", `{"model":"group/emb","input":"hi"}`); code < 400 {
		t.Fatalf("every member fails, answered %d %s", code, raw)
	}
	if got := h.got(); got["a"] != 0 || got["b"] != 0 || got["c"] != 1 {
		t.Fatalf("resting members asked %v, want c alone", got)
	}
}

// probeOne keeps every ready candidate, and of the resting ones only the
// one back soonest, at its place; with one resting or none, all.
func TestProbeOne(t *testing.T) {
	fresh(t)
	c := func(id string) candidate { return candidate{p: provider.Provider{ID: id}, model: "m", rest: id} }
	cs := []candidate{c("a"), c("b"), c("c"), c("d"), c("e")}
	ids := func(cs []candidate) string {
		var out []string
		for _, c := range cs {
			out = append(out, c.p.ID)
		}
		return strings.Join(out, " ")
	}
	if got := ids(probeOne(cs)); got != "a b c d e" {
		t.Fatalf("none resting: %s", got)
	}
	now := time.Now()
	restingUntil.Lock()
	restingUntil.m["b"] = now.Add(4 * time.Minute)
	restingUntil.Unlock()
	if got := ids(probeOne(cs)); got != "a b c d e" {
		t.Fatalf("one resting: %s", got)
	}
	restingUntil.Lock()
	restingUntil.m["d"] = now.Add(2 * time.Minute)
	restingUntil.m["e"] = now.Add(2 * time.Minute)
	restingUntil.Unlock()
	if got := ids(probeOne(cs)); got != "a c d" {
		t.Fatalf("three resting: %s", got)
	}
	if got := ids(probeOne(cs[1:2])); got != "b" {
		t.Fatalf("lone: %s", got)
	}
}
