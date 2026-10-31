package provider

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"regexp"
	"strings"
	"sync"
	"testing"
)

// headerPeer is a remote magpie behind a proxy that wants its own
// headers: it answers every request of magpie's own (cards, a refresh,
// the quota history, a key's limit, the model list) and keeps each one's
// headers by its path.
type headerPeer struct {
	sync.Mutex
	seen map[string][]http.Header
}

func (f *headerPeer) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	f.Lock()
	f.seen[r.Method+" "+r.URL.Path] = append(f.seen[r.Method+" "+r.URL.Path], r.Header.Clone())
	f.Unlock()
	switch r.URL.Path {
	case RemoteKeyLimitPath:
		w.Write([]byte(`{"limited":false,"key":"k"}`))
	case "/v1/magpie/quotas/history":
		json.NewEncoder(w).Encode(map[string]any{"object": "list", "data": []QuotaHistory{{Provider: "codex"}}})
	case "/v1/models":
		w.Write([]byte(`{"object":"list","data":[{"id":"gpt-6","object":"model"}]}`))
	default:
		json.NewEncoder(w).Encode(map[string]any{"object": "list", "data": []SubscriptionQuota{{Provider: "codex", Name: "Codex", Kind: "subscription"}}})
	}
}

// Yirasumi on Discord: a Remote magpie behind a proxy that asks for a
// header of its own answered the chat, but the Usage page's read of its
// cards went without the provider's Headers. Every request magpie makes
// to a remote magpie now carries them, ${sessionId} as an id of its own,
// and the key still reaches the remote as x-api-key: a user's
// Authorization (the proxy's) goes on in place of magpie's Bearer, and a
// user's x-api-key never replaces the key.
func TestRemoteMagpieRequestsCarryHeaders(t *testing.T) {
	for _, userAuth := range []bool{false, true} {
		t.Run(map[bool]string{false: "own auth", true: "user's Authorization"}[userAuth], func(t *testing.T) {
			peer := &headerPeer{seen: map[string][]http.Header{}}
			remoteQuotaHome(t, peer)
			p, err := Find("office")
			if err != nil {
				t.Fatal(err)
			}
			p.Headers = map[string]string{"X-Proxy-Token": "tok", "X-Session": "${sessionId}", "X-Api-Key": "not-the-key"}
			if userAuth {
				p.Headers["Authorization"] = "Basic cHJveHk6cHc="
			}
			p.Keys = []KeyAccount{{Name: "spare", Key: "sk-magpie-spare"}}
			if err := Save(*p); err != nil {
				t.Fatal(err)
			}
			ForgetRemoteCardsForTest()
			if p, err = Find("office"); err != nil {
				t.Fatal(err)
			}
			ctx := context.Background()
			RemoteCards(ctx)
			RefreshUsage(ctx, "office/codex", "")
			RemoteQuotaHistories(ctx, "7")
			if _, err := p.Fetch(ctx); err != nil {
				t.Fatalf("model list: %v", err)
			}
			req, _ := http.NewRequest(http.MethodPost, p.Chat+"/chat/completions", bytes.NewReader([]byte(`{}`)))
			if err := p.Sign(ctx, req, Chat, []byte(`{}`)); err != nil {
				t.Fatal(err)
			}
			res, err := p.Do(http.DefaultClient, req)
			if err != nil {
				t.Fatal(err)
			}
			res.Body.Close()

			own := regexp.MustCompile(`^magpie-[a-z0-9]{12}$`)
			peer.Lock()
			defer peer.Unlock()
			for _, path := range []string{
				"GET " + RemoteCardsPath, "POST " + RemoteRefreshPath, "GET /v1/magpie/quotas/history",
				"GET " + RemoteKeyLimitPath, "GET /v1/models", "POST /v1/chat/completions",
			} {
				hs := peer.seen[path]
				if len(hs) == 0 {
					t.Errorf("%s: never asked", path)
					continue
				}
				keys := map[string]bool{}
				for _, h := range hs {
					if h.Get("X-Proxy-Token") != "tok" {
						t.Errorf("%s: X-Proxy-Token %q", path, h.Get("X-Proxy-Token"))
					}
					if s := h.Get("X-Session"); !own.MatchString(s) {
						t.Errorf("%s: X-Session %q, want an id of its own", path, s)
					}
					if v := h.Values("X-Api-Key"); len(v) != 1 || !strings.HasPrefix(v[0], "sk-magpie-") {
						t.Errorf("%s: x-api-key %q, want magpie's key", path, v)
					} else {
						keys[v[0]] = true
					}
					want := "Bearer " + h.Get("X-Api-Key")
					if userAuth {
						want = "Basic cHJveHk6cHc="
					}
					if a := h.Values("Authorization"); len(a) != 1 || a[0] != want {
						t.Errorf("%s: Authorization %q, want %q", path, a, want)
					}
				}
				// each key's limit is read with that key
				if path == "GET "+RemoteKeyLimitPath && (!keys["sk-magpie-office"] || !keys["sk-magpie-spare"]) {
					t.Errorf("key limits read with %v", keys)
				}
			}
		})
	}
}
