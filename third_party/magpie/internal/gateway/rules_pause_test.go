package gateway

import (
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// peak is a vendor's peak hours, Monday to Friday.
var peak = &provider.TimeWindow{From: "09:00", To: "18:00", Days: []string{"mon", "tue", "wed", "thu", "fri"}}

// A rule that sends a turn to another member in a vendor's peak hours
// still leaves the vendor's model behind it: when the member sent to fails,
// the turn falls back onto it (what John on Discord asked to stop). A pause
// rule keeps it out of the group in those hours: not first, not a
// fallback, not where the conversation was, and back once they end.
func TestPauseKeepsAMemberOutInItsHours(t *testing.T) {
	now := time.Date(2026, 9, 29, 10, 0, 0, 0, time.Local) // a Tuesday, in the hours
	ruleClock = func() time.Time { return now }
	t.Cleanup(func() { ruleClock = time.Now })

	// "send to b/big" in the hours: a/small still answers when b fails
	s, a, b := ruled(t, provider.Rule{Use: "b/big", Time: peak})
	b.fail = 500
	if code, out := postAs(t, s, "s0", chat("hello", nil, 0, "")); code != 200 || !strings.Contains(out, "from ka") {
		t.Fatalf("send-to rule: %d %s", code, out)
	}

	// pause a/small in the hours
	s, a, b = ruled(t, provider.Rule{Use: "a/small", Pause: true, Time: peak})
	// before them, a/small answers a conversation first (the group's order)
	now = time.Date(2026, 9, 29, 8, 50, 0, 0, time.Local)
	if out, r := postOK(t, s, "s1", chat("hello", nil, 0, "")); !strings.Contains(out, "from ka") || len(r.Group.Paused) != 0 {
		t.Fatalf("before the hours: %s %+v", out, r.Group)
	}
	// in them, the same conversation's next tool round goes to b/big: the
	// pause moves it, though a/small answered it last
	now = time.Date(2026, 9, 29, 9, 1, 0, 0, time.Local)
	out, r := postOK(t, s, "s1", chat("hello", nil, 1, ""))
	if !strings.Contains(out, "from kb") {
		t.Fatalf("in the hours, mid-turn: %s", out)
	}
	if p := r.Group.Paused; len(p) != 1 || p[0].Member != "a/small" || p[0].Rule != 1 || strings.Join(p[0].When, " · ") != "time 09:00–18:00 Mon–Fri" {
		t.Fatalf("trace: %+v", r.Group.Paused)
	}
	if strings.Join(r.Group.Members, ",") != "b/big" {
		t.Fatalf("members: %v", r.Group.Members)
	}
	// b/big failing in them: a/small is not asked
	b.fail = 500
	before := a.n()
	if code, out := postAs(t, s, "s2", chat("hi", nil, 0, "")); code == 200 || strings.Contains(out, "from ka") {
		t.Fatalf("paused member answered as a fallback: %d %s", code, out)
	}
	if a.n() != before {
		t.Fatalf("a/small asked %d times while paused", a.n()-before)
	}
	// after them, a/small is back
	b.fail = 0
	now = time.Date(2026, 9, 29, 18, 0, 0, 0, time.Local)
	if out, _ := postOK(t, s, "s3", chat("hi", nil, 0, "")); !strings.Contains(out, "from ka") {
		t.Fatalf("after the hours: %s", out)
	}
	// and on a Saturday's peak hours, as they hold on weekdays only
	now = time.Date(2026, 10, 3, 10, 0, 0, 0, time.Local)
	if out, _ := postOK(t, s, "s4", chat("hi", nil, 0, "")); !strings.Contains(out, "from ka") {
		t.Fatalf("saturday: %s", out)
	}
}

// Every model of a group paused: the agent is told so, and nobody is asked.
func TestEveryMemberPaused(t *testing.T) {
	ruleClock = func() time.Time { return time.Date(2026, 9, 29, 10, 0, 0, 0, time.Local) }
	t.Cleanup(func() { ruleClock = time.Now })
	s, a, b := ruled(t, provider.Rule{Use: "a/small", Pause: true, Time: peak}, provider.Rule{Use: "b/big", Pause: true, Time: peak})
	code, out := postAs(t, s, "s", chat("hello", nil, 0, ""))
	if code != 503 || !strings.Contains(out, "paused now") || !strings.Contains(out, "a/small (rule 1: time 09:00–18:00 Mon–Fri)") || !strings.Contains(out, "b/big (rule 2") {
		t.Fatalf("%d %s", code, out)
	}
	if a.n()+b.n() != 0 {
		t.Fatalf("asked a %d b %d", a.n(), b.n())
	}
	// embeddings go by the same rules
	rec := httptest.NewRecorder()
	s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/embeddings", strings.NewReader(`{"model":"group/r","input":"x"}`)))
	if rec.Code != 503 || !strings.Contains(rec.Body.String(), "paused now") {
		t.Fatalf("embeddings: %d %s", rec.Code, rec.Body.String())
	}
}

// A send-to rule naming the paused member doesn't bring it back: the
// group routes as it would were the member not ready.
func TestPauseOutranksASendToRule(t *testing.T) {
	ruleClock = func() time.Time { return time.Date(2026, 9, 29, 10, 0, 0, 0, time.Local) }
	t.Cleanup(func() { ruleClock = time.Now })
	s, a, _ := ruled(t, provider.Rule{Use: "b/big", Pause: true, Time: peak}, provider.Rule{Use: "b/big", Tokens: 1})
	out, r := postOK(t, s, "s", chat("hello", nil, 0, ""))
	if !strings.Contains(out, "from ka") || r.Rule == nil || r.Rule.N != 2 || !r.Rule.Unready {
		t.Fatalf("%s %+v", out, r.Rule)
	}
	if a.n() != 1 {
		t.Fatalf("a asked %d", a.n())
	}
}
