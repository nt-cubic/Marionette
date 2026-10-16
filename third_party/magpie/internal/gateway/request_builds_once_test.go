package gateway

import (
	"testing"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
)

// A request builds the providers once, however many times it looks a
// model up on its way: each build reads every agent's sign-in, every
// plugin's and the settings, and three builds a request kept magpie at
// 100–350% CPU under seven agents at once (#2502). The next request,
// nothing held between them, builds afresh.
func TestRequestBuildsProvidersOnce(t *testing.T) {
	plugin.UseCached(nil)
	setup(t, provider.Chat, &fake{t: t, reply: sse(`{"choices":[{"delta":{"content":"hi"}}]}`, "[DONE]")})
	for i := range 2 {
		n := provider.AllBuilt.Load()
		code, body := post(t, "/v1/chat/completions", `{"model":"fake/m1","stream":true,"messages":[{"role":"user","content":"hi"}]}`)
		if code != 200 {
			t.Fatalf("%d %s", code, body)
		}
		if got := provider.AllBuilt.Load() - n; got != 1 {
			t.Fatalf("request %d built the providers %d times, want 1", i+1, got)
		}
	}
}
