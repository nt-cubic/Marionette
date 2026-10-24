package gateway

import (
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A group routed by weight with a Smart group in it whose every account is
// near its limit (91% and 92%, not out, not resting) and a key beside it
// (Creat.wu on Discord): the group in it weighing 4 and the key 1 take 16
// and 4 of 20 new conversations. Being near its limit doesn't pass the
// member over in its parent's round; inside it, Smart sends its share to
// the account with the most left.
func TestWeightedGroupGivesALowGroupItsShare(t *testing.T) {
	codexSignedIn(t, "spare@example.com")
	b := newResetBackend(t)
	capUsage(t, map[string]float64{"me@example.com": 91, "spare@example.com": 92})
	var keyed atomic.Int32
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.ReadAll(r.Body)
		keyed.Add(1)
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, grokAnswer)
	}))
	t.Cleanup(up.Close)
	if err := provider.Save(provider.Provider{ID: "xai", Name: "X", Key: "k", Models: []string{"gpt-5.5"}, Chat: up.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "sb", Name: "SB", Members: []string{"codex/gpt-5.5"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "pa", Name: "PA", Members: []string{"group/sb", "xai/gpt-5.5"}, Routing: provider.Weighted,
		Weights: map[string]int{"group/sb": 4, "xai/gpt-5.5": 1}}); err != nil {
		t.Fatal(err)
	}
	s := New()
	for i := range 20 {
		// a new conversation each: nothing is kept for affinity
		body := fmt.Sprintf(`{"model":"group/pa","stream":true,"input":[{"role":"user","content":[{"type":"input_text","text":"hi %d"}]}]}`, i)
		if rec := codexOn(s, "/v1/responses", body); rec.Code != 200 {
			t.Fatalf("request %d: %d %s", i, rec.Code, rec.Body.String())
		}
	}
	tried, _ := b.seen()
	if n := strings.Count(tried, "acct-"); n != 16 || keyed.Load() != 4 || strings.Contains(tried, "acct-2") {
		t.Fatalf("4 : 1 over 20: the group in it took %d (%s), the key %d", n, tried, keyed.Load())
	}
	// the trace: when the round gives a request to the group in it, its
	// account with the most left goes first; when to the key, the group's
	// accounts are after it, not resting
	var sb, key int
	for _, r := range s.Trace(t.Context(), 0, 0).Routes {
		if len(r.Order) != 3 || !r.Order[0].Turn {
			t.Fatalf("order: %+v", r.Order)
		}
		switch r.Order[0].Who {
		case "me@example.com":
			sb++
		case "":
			key++
			for _, w := range r.Order[1:] {
				if w.Rest != nil || len(w.Via) != 1 || w.Via[0] != "sb" {
					t.Fatalf("behind the key: %+v", w)
				}
			}
		default:
			t.Fatalf("first: %+v", r.Order[0])
		}
	}
	if sb != 16 || key != 4 {
		t.Fatalf("traced: the group in it first %d, the key %d", sb, key)
	}
}
