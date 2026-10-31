package gateway

import (
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// A classifier that can't say at every turn is said where the group is,
// not only in each request's own details (#1929, versun): MLX-Serve turned
// its decision model away as a chat model with a 400, and the group's
// rules with an intent quietly never matched. ClassifierFailure is why it
// couldn't say the last time it was asked, an answer that was no answer
// among them, until it answers.
func TestClassifierFailureIsKept(t *testing.T) {
	resetClassified(t)
	s, _, _, c := intented(t, provider.Rule{Use: "b/big", Intent: "refactoring"})
	if _, _, ok := ClassifierFailure("c/cls"); ok {
		t.Fatal("a failure before it was asked")
	}
	c.set("", 400)
	t0 := time.Now()
	_, r := postOK(t, s, "cf1", chat("rename this package", nil, 0, ""))
	why, at, ok := ClassifierFailure("c/cls")
	if !ok || !strings.Contains(why, "classifier down") || at.Before(t0) || r.Rule.Classified.Error == "" {
		t.Fatalf("400: %q %v %v (%+v)", why, at, ok, r.Rule.Classified)
	}
	// resting, it isn't asked; what it last said stays
	_, r = postOK(t, s, "cf2", chat("rename that package", nil, 0, ""))
	if why2, _, ok := ClassifierFailure("c/cls"); !ok || why2 != why || !strings.Contains(r.Rule.Classified.Error, "not asked again") {
		t.Fatalf("resting: %q %v", why2, ok)
	}
	// an answer that is no number of the list is a failure too
	classified.Lock()
	delete(classified.failed, "c/cls")
	classified.Unlock()
	c.set("maybe", 0)
	postOK(t, s, "cf3", chat("rename it back", nil, 0, ""))
	if why, _, ok := ClassifierFailure("c/cls"); !ok || !strings.Contains(why, "not a number") {
		t.Fatalf("no number: %q %v", why, ok)
	}
	// it answers: nothing to say
	c.set("1", 0)
	_, r = postOK(t, s, "cf4", chat("rename everything", nil, 0, ""))
	if why, _, ok := ClassifierFailure("c/cls"); ok || r.Rule.N != 1 {
		t.Fatalf("answered: %q %+v", why, r.Rule)
	}
}

// A group says its classifier's failure only when that group's own turn
// asked it (jjt on Discord): "claude难易", which no request reached, said
// "the classifier failed 3 minutes ago: claude/claude-haiku-5-5: You've hit
// your weekly limit", read as its classifier judging every turn, while the
// failure was another group's that has the same classifier. A request for
// another model asks no group's classifier at all.
func TestClassifierFailureIsTheGroupsOwn(t *testing.T) {
	resetClassified(t)
	s, _, _, c := intented(t, provider.Rule{Use: "b/big", Intent: "refactoring"})
	if err := provider.SaveGroup(provider.Group{ID: "idle", Name: "Idle", Members: []string{"a/small", "b/big"}, Routing: provider.Ordered,
		Classifier: "c/cls", Rules: []provider.Rule{{Use: "b/big", Intent: "refactoring"}}}); err != nil {
		t.Fatal(err)
	}
	c.set("", 400)
	// a model of the idle group's, asked for itself: no classifier
	postOK(t, s, "own1", `{"model":"a/small","messages":[{"role":"user","content":"rename this package"}]}`)
	if n := c.n(); n != 0 {
		t.Fatalf("a request for a/small asked a classifier %d times", n)
	}
	_, r := postOK(t, s, "own2", chat("rename this package", nil, 0, ""))
	if r.Rule == nil || r.Rule.Classified == nil || r.Rule.Classified.Error == "" {
		t.Fatalf("group/r's turn: %+v", r.Rule)
	}
	if why, _, ok := GroupClassifierFailure("r", "c/cls"); !ok || !strings.Contains(why, "classifier down") {
		t.Fatalf("group/r, whose turn it failed: %q %v", why, ok)
	}
	if why, at, ok := GroupClassifierFailure("idle", "c/cls"); ok {
		t.Fatalf("idle, which nothing asked: %q at %v", why, at)
	}
	// group/r's classifier changed since: what the old one said is not the new one's
	if _, _, ok := GroupClassifierFailure("r", "c/other"); ok {
		t.Fatal("a failure said for a classifier the group no longer has")
	}
	// resting, it is still why group/r's next turn went unjudged, in the
	// vendor's words and at the time it failed, not the rest's
	_, at0, _ := GroupClassifierFailure("r", "c/cls")
	postOK(t, s, "own3", chat("rename that package", nil, 0, ""))
	if why, at, ok := GroupClassifierFailure("r", "c/cls"); !ok || !strings.Contains(why, "classifier down") || !at.Equal(at0) {
		t.Fatalf("resting: %q %v %v", why, at, ok)
	}
	// it answers group/r: nothing to say
	classified.Lock()
	delete(classified.failed, "c/cls")
	classified.Unlock()
	c.set("1", 0)
	postOK(t, s, "own4", chat("rename everything", nil, 0, ""))
	if why, _, ok := GroupClassifierFailure("r", "c/cls"); ok {
		t.Fatalf("answered: %q", why)
	}
}
