package gateway

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// An agent left on a group magpie found, or a session begun on one, keeps
// working once the user turns found groups off (蓝猫 on Discord): its
// request goes to the model from the first provider that serves it, as a
// model of one provider — not refused as a model magpie doesn't know.
func TestFoundGroupOff(t *testing.T) {
	fresh(t)
	a, b := &keyed{}, &keyed{}
	serveOn(t, "a", "ka", []string{"m"}, a)
	serveOn(t, "b", "kb", []string{"m"}, b)
	s := New()
	const req = `{"model":"group/auto-m","messages":[{"role":"user","content":"hi"}]}`
	if code, body := postAs(t, s, "", req); code != 200 {
		t.Fatalf("on: %d %s", code, body)
	}
	if r := s.trace.routes[len(s.trace.routes)-1]; r.Group == nil {
		t.Fatalf("on, not routed as the group: %+v", r)
	}

	if err := provider.SetAutoGroups(false); err != nil {
		t.Fatal(err)
	}
	a.tried, b.tried = nil, nil
	code, body := postAs(t, s, "", req)
	if code != 200 || !strings.Contains(body, "from ka") || len(b.tried) != 0 {
		t.Fatalf("off: %d %s, a tried %v, b tried %v", code, body, a.tried, b.tried)
	}
	if r := s.trace.routes[len(s.trace.routes)-1]; r.Group != nil {
		t.Fatalf("off, routed as a group: %+v", r.Group)
	}
	// a bare model id is the first provider's too
	a.tried = nil
	if code, body := postAs(t, s, "", `{"model":"m","messages":[{"role":"user","content":"hi"}]}`); code != 200 || len(a.tried) == 0 {
		t.Fatalf("bare m: %d %s", code, body)
	}
}

// An agent set to a group magpie found keeps it while the user switches
// one of its two providers off for a while: the request is the group's,
// answered by the provider still on, not refused as a model magpie
// doesn't know, and the other is asked again once it is back on.
func TestFoundGroupWithAProviderOff(t *testing.T) {
	fresh(t)
	a, b := &keyed{}, &keyed{}
	serveOn(t, "a", "ka", []string{"m"}, a)
	serveOn(t, "b", "kb", []string{"m"}, b)
	s := New()
	if err := provider.SetOff("a", true); err != nil {
		t.Fatal(err)
	}
	for _, model := range []string{"group/auto-m", "m"} {
		a.tried, b.tried = nil, nil
		code, body := postAs(t, s, "", `{"model":"`+model+`","messages":[{"role":"user","content":"hi"}]}`)
		if code != 200 || !strings.Contains(body, "from kb") || len(a.tried) != 0 {
			t.Fatalf("%s with a off: %d %s, a tried %v", model, code, body, a.tried)
		}
		if r := s.trace.routes[len(s.trace.routes)-1]; r.Group == nil {
			t.Fatalf("%s with a off, not routed as the group: %+v", model, r)
		}
	}
	if err := provider.SetOff("a", false); err != nil {
		t.Fatal(err)
	}
	// another conversation, so b's answer to the one above doesn't hold it
	if code, body := postAs(t, s, "", `{"model":"group/auto-m","messages":[{"role":"user","content":"a new one"}]}`); code != 200 || !strings.Contains(body, "from ka") {
		t.Fatalf("a on again: %d %s", code, body)
	}
}
