package gateway

import (
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
)

// A request builds the providers once, however many times it looks a
// model up on its way: each build reads every agent's sign-in, every
// plugin's and the settings, and three builds a request kept magpie at
// 100–350% CPU under seven agents at once (#2502). The next request,
// within a second and nothing written, builds none: one request after
// another built the providers again each time. After a write it builds
// once more.
func TestRequestBuildsProvidersOnce(t *testing.T) {
	plugin.UseCached(nil)
	setup(t, provider.Chat, &fake{t: t, reply: sse(`{"choices":[{"delta":{"content":"hi"}}]}`, "[DONE]")})
	var first time.Time
	for i, want := range []int64{1, 0, 1} {
		if i == 2 {
			provider.Changed() // a provider saved
		}
		n, start := provider.AllBuilt.Load(), time.Now()
		if i == 1 && start.Sub(first) > 900*time.Millisecond {
			// what was built is held for a second: a request this late
			// (a loaded machine) builds again, rightly
			want = 1
		}
		if i == 0 {
			first = start
		}
		code, body := post(t, "/v1/chat/completions", `{"model":"fake/m1","stream":true,"messages":[{"role":"user","content":"hi"}]}`)
		if code != 200 {
			t.Fatalf("%d %s", code, body)
		}
		if got := provider.AllBuilt.Load() - n; got != want {
			t.Fatalf("request %d built the providers %d times, want %d", i+1, got, want)
		}
	}
}
