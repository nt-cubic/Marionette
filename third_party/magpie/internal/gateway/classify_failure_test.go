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
