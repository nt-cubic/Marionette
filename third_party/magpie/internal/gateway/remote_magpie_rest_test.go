package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// bigsteven on Discord: another computer's magpie (LAN share) serves each
// of its models from its own providers, and rests them itself. When one of
// them was out of quota there, its 429 rested the whole remote magpie here
// for the 3 hours it named, so a group whose member was another of its
// models, with quota left, went to its next member, or had none. Such a
// refusal rests only the model it was about.
func TestRemoteMagpieQuotaRestsTheModel(t *testing.T) {
	fresh(t)
	var asked []string
	remote := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Model string `json:"model"`
		}
		json.NewDecoder(r.Body).Decode(&req)
		asked = append(asked, req.Model)
		w.Header().Set("Content-Type", "application/json")
		if req.Model == "codex/gpt-5.5" {
			w.WriteHeader(http.StatusTooManyRequests)
			io.WriteString(w, `{"error":{"message":"You've hit your usage limit. Your quota resets in 3 hours.","type":"usage_limit_reached"}}`)
			return
		}
		if req.Model == "glm/glm-5" {
			w.WriteHeader(http.StatusTooManyRequests)
			io.WriteString(w, `{"error":{"message":"rate limit exceeded, slow down"}}`)
			return
		}
		io.WriteString(w, `{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":"from `+req.Model+`"},"finish_reason":"stop"}]}`)
	}))
	t.Cleanup(remote.Close)
	if err := provider.Save(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset, Key: "k",
		Chat: remote.URL + "/v1", Models: []string{"codex/gpt-5.5", "glm/glm-5", "kimi/kimi-k3"}}); err != nil {
		t.Fatal(err)
	}
	other := &keyed{}
	serveOn(t, "other", "ko", []string{"m"}, other)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"office/kimi/kimi-k3", "other/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	if code, body := postAs(t, s, "", `{"model":"office/codex/gpt-5.5","messages":[{"role":"user","content":"hi"}]}`); code != 429 {
		t.Fatalf("codex on the remote: %d %s", code, body)
	}
	if r, ok := restOf("office"); ok {
		t.Errorf("the whole remote magpie rests for one model's quota: %+v", r)
	}
	if _, ok := restOf("office/codex/gpt-5.5"); !ok {
		t.Errorf("codex/gpt-5.5 doesn't rest: %v", rests())
	}
	// and a rate limit on one of its models, the same: the group's next
	// member on that computer answers
	if err := provider.SaveGroup(provider.Group{Name: "R", Members: []string{"office/glm/glm-5", "office/kimi/kimi-k3"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	if code, body := postAs(t, s, "", `{"model":"group/r","messages":[{"role":"user","content":"hi"}]}`); code != 200 || !strings.Contains(body, "from kimi/kimi-k3") {
		t.Fatalf("glm on the remote: %d %s", code, body)
	}
	if r, ok := restOf("office"); ok {
		t.Errorf("the whole remote magpie rests for one model's rate limit: %+v", r)
	}
	if r, ok := restOf("office/glm/glm-5"); !ok || r.Why != failRate {
		t.Errorf("glm/glm-5 doesn't rest as rate limited: %+v %v", r, rests())
	}
	asked = nil
	code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "from kimi/kimi-k3") || len(other.tried) != 0 {
		t.Fatalf("the group: %d %s; the remote was asked %v, other %v", code, body, asked, other.tried)
	}
}
