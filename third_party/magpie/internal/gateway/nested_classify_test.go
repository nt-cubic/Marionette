package gateway

import (
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// #4358: a group in the group, itself effort=auto with a classifier of
// its own, is asked its classifier even when the group above it already
// picked the turn's reasoning — an answer the turn throws away, the
// requests going out at the parent's pick. The parent's pick stands, and
// the group in it is asked only when the parent gave no pick.
func TestNestedGroupClassifierNotAskedWhenTheParentPicked(t *testing.T) {
	fresh(t)
	parent, child := &clsUp{answer: "3"}, &clsUp{answer: "1"} // parent picks high, child low
	for _, x := range []struct {
		id string
		up *clsUp
	}{{"cp", parent}, {"cc", child}} {
		srv := httptest.NewServer(x.up)
		t.Cleanup(srv.Close)
		if err := provider.Save(provider.Provider{ID: x.id, Name: strings.ToUpper(x.id), Key: "k" + x.id, Models: []string{"cls"}, Chat: srv.URL + "/v1"}); err != nil {
			t.Fatal(err)
		}
		if err := catalog.SaveLive(x.id, srv.URL+"/v1", []catalog.Model{{ID: "cls", Context: 32000}}); err != nil {
			t.Fatal(err)
		}
	}
	f := &ruleUp{key: "kf"}
	fs := httptest.NewServer(f)
	t.Cleanup(fs.Close)
	if err := provider.Save(provider.Provider{ID: "f1", Name: "F1", Key: "kf1", Models: []string{"m"}, Chat: fs.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := catalog.SaveLive("f1", fs.URL+"/v1", []catalog.Model{{ID: "m", Context: 32000}}); err != nil {
		t.Fatal(err)
	}
	for _, g := range []provider.Group{
		{Name: "Quality", Members: []string{"f1/m"}, Routing: provider.Ordered, Effort: provider.EffortAuto, Classifier: "cc/cls"},
		{Name: "R", Members: []string{"group/quality"}, Routing: provider.Ordered, Effort: provider.EffortAuto, Classifier: "cp/cls"},
	} {
		if err := provider.SaveGroup(g); err != nil {
			t.Fatal(err)
		}
	}
	s := New()
	// the parent's classifier answers: its pick is the one sent, and the
	// group in it is asked nothing
	out, r := postOK(t, s, "s1", chat("fix the bug", nil, 0, `,"reasoning_effort":"medium"`))
	if !strings.Contains(out, "from kf") {
		t.Fatalf("%s", out)
	}
	if parent.n() != 1 {
		t.Fatalf("the parent's classifier asked %d times", parent.n())
	}
	if n := child.n(); n != 0 {
		t.Fatalf("the group in it asked its classifier %d times though the parent picked", n)
	}
	if !strings.Contains(f.last, `"reasoning_effort":"high"`) {
		t.Fatalf("the parent's pick not sent: %s", f.last)
	}
	if r.Rule == nil || r.Rule.Pick != "high" || r.Rule.Classified == nil || r.Rule.Classified.By != "cp/cls" {
		t.Fatalf("parent traced %+v", r.Rule)
	}
	if len(r.Nested) != 1 || r.Nested[0].Group != "quality" || r.Nested[0].Rule == nil || r.Nested[0].Rule.Pick != "" {
		t.Fatalf("nested traced %+v", r.Nested)
	}
	// the trace says the group above picked, not that the group in it
	// couldn't say (the Routing page told that as "reasons as the agent
	// asked", which the turn doesn't)
	if c := r.Nested[0].Rule.Classified; c == nil || c.Above != "high" || c.Error != "" {
		t.Fatalf("nested classified %+v", c)
	}
	// the parent's classifier giving nothing, the group in it is asked as
	// before: its pick is what the turn goes by
	parent.set("", 500)
	was := parent.n()
	out, r = postOK(t, s, "s2", chat("write the parser", nil, 0, `,"reasoning_effort":"medium"`))
	if !strings.Contains(out, "from kf") {
		t.Fatalf("%s", out)
	}
	// a failing upstream is asked again (and again, the pipeline's
	// same-one retries of a 500, seen on the plain intent path too);
	// the turn asked it afresh, whatever the count came to
	if parent.n() <= was {
		t.Fatalf("the parent's classifier asked %d times, was %d", parent.n(), was)
	}
	if child.n() != 1 {
		t.Fatalf("the fallback asked %d times", child.n())
	}
	if !strings.Contains(f.last, `"reasoning_effort":"low"`) {
		t.Fatalf("the group in it's pick not sent: %s", f.last)
	}
	if r.Rule == nil || r.Rule.Pick != "" || len(r.Nested) != 1 || r.Nested[0].Rule == nil || r.Nested[0].Rule.Pick != "low" {
		t.Fatalf("traced %+v %+v", r.Rule, r.Nested)
	}
}
