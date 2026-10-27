package gateway

import (
	"io"
	"math"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// Speed lately is a decayed average: a sample counts half after
// speedHalfLife against those since, and a seat not measured for
// speedStale counts as not measured at all, so Speed first gives it a
// probe's turn instead of holding an old number against it.
func TestSpeedLatelyDecaysAndGoesStale(t *testing.T) {
	forgetSpeeds()
	t.Cleanup(forgetSpeeds)
	c := candidate{p: provider.Provider{ID: "v"}, rest: "v", model: "m"}
	t0 := time.Date(2026, 10, 26, 12, 0, 0, 0, time.UTC)
	noteSpeed(c, 1000, 0, 0, t0)
	if ttft, tps := speedOf(c, t0); ttft != 1000 || tps != 0 {
		t.Fatalf("one sample without a pace: %v %v", ttft, tps)
	}
	// a sample a half-life later counts double the first: (1000*0.5 + 4000)/1.5
	noteSpeed(c, 4000, 100, 1000, t0.Add(speedHalfLife))
	ttft, tps := speedOf(c, t0.Add(speedHalfLife))
	if math.Abs(ttft-3000) > 0.01 || tps != 100 {
		t.Fatalf("after a half-life: ttft %v tps %v, want 3000 and 100", ttft, tps)
	}
	// a reply that wasn't streamed tells nothing
	noteSpeed(c, 0, 50, 1000, t0.Add(speedHalfLife))
	if again, _ := speedOf(c, t0.Add(speedHalfLife)); again != ttft {
		t.Fatalf("an unstreamed reply moved the average: %v", again)
	}
	// stale: not measured lately
	if ttft, tps := speedOf(c, t0.Add(speedHalfLife+speedStale+time.Second)); ttft != 0 || tps != 0 {
		t.Fatalf("an hour and more on, still measured: %v %v", ttft, tps)
	}
	// another model of the same key is its own
	if ttft, _ := speedOf(candidate{p: c.p, rest: "v", model: "n"}, t0); ttft != 0 {
		t.Fatalf("another model shares the speed: %v", ttft)
	}
}

// streamed is a vendor that streams one short chat answer, its first
// content a few ms after the request so its time to first token is
// measured (in whole ms).
func streamed(w http.ResponseWriter, r *http.Request) {
	io.Copy(io.Discard, r.Body)
	time.Sleep(3 * time.Millisecond)
	w.Header().Set("Content-Type", "text/event-stream")
	io.WriteString(w, "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"hi\"}}]}\n\n")
	io.WriteString(w, "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1,\"total_tokens\":4}}\n\n")
	io.WriteString(w, "data: [DONE]\n\n")
}

// A group routed Speed first sends the same model to the provider that
// answered it fastest lately, though it is listed second, and says so in
// the trace; In order on the same speeds keeps the listed one. The speeds
// are those the gateway measured on the group's own answers.
func TestSpeedFirstGroupSendsTheFastestLately(t *testing.T) {
	fresh(t)
	forgetSpeeds()
	t.Cleanup(forgetSpeeds)
	serveOn(t, "slowv", "sk-slow", []string{"m"}, http.HandlerFunc(streamed))
	serveOn(t, "fastv", "sk-fast", []string{"m"}, http.HandlerFunc(streamed))
	if err := provider.SaveGroup(provider.Group{
		Name: "G", Members: []string{"slowv/m", "fastv/m"}, Routing: provider.Fastest,
	}); err != nil {
		t.Fatal(err)
	}
	s := New()
	ask := func() *Route {
		t.Helper()
		code, body := postAs(t, s, "", `{"model":"group/g","stream":true,"messages":[{"role":"user","content":"x"}]}`)
		if code != 200 {
			t.Fatalf("code %d: %s", code, body)
		}
		return s.trace.routes[len(s.trace.routes)-1]
	}
	// none measured: listed order, and that answer is measured
	if first := ask(); first.Order[0].Provider != "slowv" {
		t.Fatalf("with nothing measured Speed first left the list: %+v", first.Order)
	}
	var slowID string
	speeds.Lock()
	for id, r := range speeds.m {
		if strings.HasPrefix(id, "slowv") && r.ttft > 0 {
			slowID = id
		}
	}
	speeds.Unlock()
	if slowID == "" {
		t.Fatal("the streamed answer wasn't measured")
	}
	// what each has answered at lately, as many answers would leave it
	now := time.Now()
	speeds.Lock()
	speeds.m[slowID] = speedRec{ttft: 3000, ttftW: 5, at: now, tps: 40, tpsW: 5, tpsAt: now}
	speeds.m[strings.Replace(slowID, "slowv", "fastv", 1)] = speedRec{ttft: 800, ttftW: 5, at: now, tps: 60, tpsW: 5, tpsAt: now}
	speeds.Unlock()

	last := ask()
	if last.Order[0].Provider != "fastv" {
		t.Fatalf("Speed first went to %q first, want fastv: %+v", last.Order[0].Provider, last.Order)
	}
	if last.Order[0].LateTTFT < 700 || last.Order[0].LateTTFT > 800 || last.Order[0].LateTPS == 0 {
		t.Fatalf("the trace row doesn't tell fastv's speed: %+v", last.Order[0])
	}

	// In order, on the same speeds: the list, and no row tells a speed
	if err := provider.SaveGroup(provider.Group{
		Name: "G", Members: []string{"slowv/m", "fastv/m"}, Routing: provider.Ordered,
	}); err != nil {
		t.Fatal(err)
	}
	inOrder := ask()
	if inOrder.Order[0].Provider != "slowv" {
		t.Fatalf("In order went to %q first: %+v", inOrder.Order[0].Provider, inOrder.Order)
	}
	for _, w := range inOrder.Order {
		if w.LateTTFT != 0 {
			t.Fatalf("an In order row tells a speed: %+v", w)
		}
	}
}
