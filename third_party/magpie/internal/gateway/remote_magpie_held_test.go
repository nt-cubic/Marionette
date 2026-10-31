package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// zaidu259434 on X: a routing group on another computer's magpie ("永不卡住")
// had one member, Open Design, out of its Coding Plan quota and wallet. The
// remote answered 402, its own hold naming when it would ask again
// (Retry-After), and this magpie rested the remote's group model until then
// and gave the 402 again on every request, never asking the remote. The
// user moved the group to another vendor there, and Codex here still got
// Open Design's refusal: the remote magpie routes its own groups and holds
// its own rests, and what it serves under a name can change at any moment,
// so a refusal of its is never given again here without asking it.
func TestRemoteMagpieQuotaIsAskedNotReplayed(t *testing.T) {
	fresh(t)
	var mu sync.Mutex
	spent := true
	asked := 0
	remote := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Model string `json:"model"`
		}
		json.NewDecoder(r.Body).Decode(&req)
		mu.Lock()
		asked++
		out := spent
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		if out {
			// what a remote magpie holding its group's lone member says
			w.Header().Set("Retry-After", "3600")
			w.Header().Set(providerHeader, "opendesign")
			w.WriteHeader(http.StatusPaymentRequired)
			io.WriteString(w, `{"error":{"type":"api_error","message":"Open Design: [code=insufficient_balance] Coding Plan quota is exhausted and wallet balance is insufficient (out of its allowance: not asked again before 06:19 UTC+1)"}}`)
			return
		}
		io.WriteString(w, `{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":"from `+req.Model+` on its new member"},"finish_reason":"stop"}]}`)
	}))
	t.Cleanup(remote.Close)
	if err := provider.Save(provider.Provider{ID: "office", Name: "Remote magpie", Preset: provider.RemoteMagpiePreset, Key: "k",
		Chat: remote.URL + "/v1", Models: []string{"group/never-stuck"}}); err != nil {
		t.Fatal(err)
	}
	s := New()
	body := `{"model":"office/group/never-stuck","messages":[{"role":"user","content":"hi"}]}`
	if code, got := postAs(t, s, "", body); code != http.StatusPaymentRequired {
		t.Fatalf("the remote's 402: %d %s", code, got)
	}
	// a second refusal, as the agent's retry meets it
	postAs(t, s, "", body)
	// the group is changed on the remote
	mu.Lock()
	spent, asked = false, 0
	mu.Unlock()
	code, got := postAs(t, s, "", body)
	mu.Lock()
	n := asked
	mu.Unlock()
	if code != http.StatusOK || !strings.Contains(got, "new member") || n != 1 {
		t.Fatalf("after the group changed on the remote: %d %s; the remote was asked %d times", code, got, n)
	}
}

// The same for a magpie added by hand, as a plain OpenAI-compatible
// provider: its reply names the provider that answered there.
func TestHandAddedMagpieQuotaIsAskedNotReplayed(t *testing.T) {
	fresh(t)
	var mu sync.Mutex
	spent := true
	remote := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		out := spent
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		if out {
			w.Header().Set("Retry-After", "3600")
			w.Header().Set(providerHeader, "opendesign")
			w.WriteHeader(http.StatusPaymentRequired)
			io.WriteString(w, `{"error":{"type":"api_error","message":"Open Design: [code=insufficient_balance] wallet balance is insufficient"}}`)
			return
		}
		io.WriteString(w, `{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":"answered"},"finish_reason":"stop"}]}`)
	}))
	t.Cleanup(remote.Close)
	if err := provider.Save(provider.Provider{ID: "desk", Name: "Desk", Key: "k",
		Chat: remote.URL + "/v1", Models: []string{"group/g"}}); err != nil {
		t.Fatal(err)
	}
	s := New()
	body := `{"model":"desk/group/g","messages":[{"role":"user","content":"hi"}]}`
	postAs(t, s, "", body)
	postAs(t, s, "", body)
	mu.Lock()
	spent = false
	mu.Unlock()
	if code, got := postAs(t, s, "", body); code != http.StatusOK || !strings.Contains(got, "answered") {
		t.Fatalf("after the group changed there: %d %s", code, got)
	}
}
