package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"slices"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// decideUp is the fake plugin's vendor serving conversations and System
// One both: it keeps the path, model and sign-in of each request.
type decideUp struct {
	mu    sync.Mutex
	asked []string // "path model authorization"
}

func (u *decideUp) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	b, _ := io.ReadAll(r.Body)
	var q struct{ Model string }
	json.Unmarshal(b, &q)
	u.mu.Lock()
	u.asked = append(u.asked, r.URL.Path+" "+q.Model+" "+r.Header.Get("Authorization"))
	u.mu.Unlock()
	w.Header().Set("Content-Type", "application/json")
	switch r.URL.Path {
	case "/v1/systemone":
		io.WriteString(w, `{"model":"`+q.Model+`","answers":{"levels":{"type":"noul","noul":0},"intent":{"type":"choice","choice":"bug","confidence":0.9}}}`)
	case "/v1/chat/completions":
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(`data: {"id":"c","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"from the plugin"},"finish_reason":"stop"}]}`, `data: [DONE]`))
	default:
		http.NotFound(w, r)
	}
}

func (u *decideUp) seen() []string {
	u.mu.Lock()
	defer u.mu.Unlock()
	return slices.Clone(u.asked)
}

// A plugin that says decide on its provider serves System One for it
// (#1514, FourLeafTec): its marked models and Jev's by name are offered as
// a routing group's classifier, kept from agents, and asked through the
// plugin's own fetch at its base /systemone.
func TestPluginServesSystemOne(t *testing.T) {
	t.Setenv("FAKE_DECIDE", "1")
	up := &decideUp{}
	pid := besideFake(t, "fakeco", up)
	p, err := provider.Find(pid)
	if err != nil {
		t.Fatal(err)
	}
	if p.Decide != "plugin://fakeco/v1" {
		t.Fatalf("Decide = %q", p.Decide)
	}
	// fake-clef is marked in the config hook, fake-judge on the models
	// hook's list, jev-9 is Jev by its name
	deciders := map[string]bool{}
	for _, e := range provider.Deciders() {
		deciders[e.ID] = true
	}
	for _, m := range []string{"fake-clef", "fake-judge", "jev-9"} {
		if !deciders[pid+"/"+m] {
			t.Errorf("%s is no classifier: %v", m, deciders)
		}
		if offered(pid + "/" + m) {
			t.Errorf("%s is offered to agents", m)
		}
	}
	if deciders[pid+"/fake-1"] {
		t.Error("fake-1, a chat model, is a classifier")
	}
	if !offered(pid + "/fake-1") {
		t.Error("fake-1 isn't offered to agents")
	}
	s := New()
	if code, out := postAs(t, s, "", `{"model":"`+pid+`/fake-1","messages":[{"role":"user","content":"hi"}]}`); code != 200 || !strings.Contains(out, "from the plugin") {
		t.Fatalf("chat: %d %s", code, out)
	}
	if code, out := postAs(t, s, "", `{"model":"`+pid+`/fake-clef","messages":[{"role":"user","content":"hi"}]}`); code != 400 || !strings.Contains(out, "only decides") {
		t.Fatalf("chat to a decision model: %d %s", code, out)
	}
	// a chat group's classifier, and never one of its models; a group of
	// decision models alone is a decision group (decide_group.go)
	g := provider.Group{ID: "d", Members: []string{pid + "/fake-1"}, Classifier: pid + "/fake-clef",
		Rules: []provider.Rule{{Use: pid + "/fake-1", Intent: "bug"}}}
	if err := provider.SaveGroup(g); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "e", Members: []string{pid + "/fake-1", pid + "/fake-judge"}}); err == nil || !strings.Contains(err.Error(), "decision models only") {
		t.Fatalf("a decision model saved beside a chat model: %v", err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "e", Members: []string{pid + "/fake-judge", pid + "/jev-9"}}); err != nil {
		t.Fatalf("a group of decision models: %v", err)
	}
	for _, m := range []string{"fake-clef", "fake-judge", "jev-9"} {
		if v, err := s.askClassifier(pid+"/"+m, []string{"feature", "bug"}, before{}, false, "fix this"); err != nil || v.Intent != "bug" {
			t.Fatalf("classify with %s: %+v %v", m, v, err)
		}
	}
	var decided []string
	for _, a := range up.seen() {
		if strings.HasPrefix(a, "/v1/systemone ") {
			decided = append(decided, a)
		}
	}
	want := []string{"/v1/systemone fake-clef Bearer k1", "/v1/systemone fake-judge Bearer k1", "/v1/systemone jev-9 Bearer k1"}
	// each is asked whether the intents are levels, then which it is
	if !slices.Equal(slices.Compact(decided), want) {
		t.Fatalf("the plugin's vendor was asked %v, want %v", up.seen(), want)
	}
	// and the editor's test asks the same way
	if err := p.AskSystemOne(t.Context(), "fake-clef"); err != nil {
		t.Fatal(err)
	}
}

// A plugin that marks models without saying decide has no decision API:
// its models stay ones to chat with, as before #1514.
func TestPluginMarksWithoutDecide(t *testing.T) {
	t.Setenv("FAKE_DECIDE", "models")
	pid := besideFake(t, "fakeco", &decideUp{})
	p, err := provider.Find(pid)
	if err != nil {
		t.Fatal(err)
	}
	if p.Decides() {
		t.Fatalf("Decide = %q", p.Decide)
	}
	for _, e := range provider.Deciders() {
		if e.Provider.ID == pid {
			t.Fatalf("%s is a classifier", e.ID)
		}
	}
	for _, m := range []string{"fake-clef", "jev-9", "fake-1"} {
		if !offered(pid + "/" + m) {
			t.Errorf("%s isn't offered to agents", m)
		}
	}
}

// offered reports whether agents are offered the model id.
func offered(id string) bool {
	return slices.ContainsFunc(provider.Catalog(), func(e provider.Entry) bool { return e.ID == id })
}
