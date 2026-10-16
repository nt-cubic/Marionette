package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
)

// tierUp is an upstream for the fake plugin that answers Chat and Responses
// and keeps the service_tier each request was sent with ("" for none).
type tierUp struct {
	mu    sync.Mutex
	tiers []any
}

func (u *tierUp) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	var q map[string]any
	b, _ := io.ReadAll(r.Body)
	json.Unmarshal(b, &q)
	u.mu.Lock()
	u.tiers = append(u.tiers, q["service_tier"])
	u.mu.Unlock()
	w.Header().Set("Content-Type", "text/event-stream")
	if strings.HasSuffix(r.URL.Path, "/responses") {
		io.WriteString(w, sse(
			`event: response.created`+"\n"+`data: {"type":"response.created","response":{"id":"r1","object":"response","status":"in_progress","output":[]}}`,
			`event: response.output_item.done`+"\n"+`data: {"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"m1","role":"assistant","status":"completed","content":[{"type":"output_text","text":"hi","annotations":[]}]}}`,
			`event: response.completed`+"\n"+`data: {"type":"response.completed","response":{"id":"r1","object":"response","status":"completed","output":[{"type":"message","id":"m1","role":"assistant","status":"completed","content":[{"type":"output_text","text":"hi","annotations":[]}]}],"usage":{"input_tokens":3,"output_tokens":1,"total_tokens":4}}}`))
		return
	}
	io.WriteString(w, sse(`data: {"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"hi"},"finish_reason":"stop"}]}`, `data: [DONE]`))
}

func (u *tierUp) last(t *testing.T) any {
	t.Helper()
	u.mu.Lock()
	defer u.mu.Unlock()
	if len(u.tiers) == 0 {
		t.Fatal("the plugin asked its upstream nothing")
	}
	return u.tiers[len(u.tiers)-1]
}

// A provider plugin says which service tiers its vendor serves a model at
// (serviceTiers), and the model list Codex reads offers them: priority as
// Fast, ultrafast by its id (Lovest on Discord, #2514). A model that says
// none is offered none.
func TestPluginModelServiceTiersListed(t *testing.T) {
	t.Setenv("FAKE_TIERS", "1")
	pid := besideFake(t, "tiered", &tierUp{})
	rec := httptest.NewRecorder()
	New().Handler().ServeHTTP(rec, httptest.NewRequest("GET", "/v1/models", nil))
	var list struct {
		Data []struct {
			ID    string `json:"id"`
			Tiers []struct {
				ID   string `json:"id"`
				Name string `json:"name"`
			} `json:"service_tiers"`
		} `json:"data"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &list); err != nil {
		t.Fatalf("%v: %s", err, rec.Body)
	}
	got := map[string]string{}
	for _, m := range list.Data {
		s := ""
		for _, x := range m.Tiers {
			s += x.ID + "=" + x.Name + " "
		}
		got[m.ID] = s
	}
	for id, want := range map[string]string{pid + "/fake-1": "priority=Fast ultrafast= ", pid + "/fake-resp": "ultrafast= ", pid + "/fake-claude": ""} {
		if s, ok := got[id]; !ok || !tiersLike(s, want) {
			t.Errorf("%s tiers %q (listed %v), want %q", id, s, ok, want)
		}
	}
}

// tiersLike compares "id=name " lists by id, and by name where want names
// one: ultrafast's name comes from Codex's own catalog when it has one.
func tiersLike(got, want string) bool {
	g, w := strings.Fields(got), strings.Fields(want)
	if len(g) != len(w) {
		return false
	}
	for i := range w {
		gi, gn, _ := strings.Cut(g[i], "=")
		wi, wn, _ := strings.Cut(w[i], "=")
		if gi != wi || wn != "" && gn != wn {
			return false
		}
	}
	return true
}

// A request that asks a plugin's model for a tier the plugin says it is
// served at reaches the plugin's fetch with that service_tier, on Chat and
// on Responses (#2514), also where magpie translates the request to the
// plugin's protocol. There, one it doesn't say, or a model that says none, is
// sent without it, as before; a request relayed in the client's own protocol
// was already sent as the client wrote it.
func TestPluginServiceTierReachesFetch(t *testing.T) {
	t.Setenv("FAKE_TIERS", "1")
	t.Setenv("FAKE_RESPONSES", "1")
	up := &tierUp{}
	pid := besideFake(t, "tiered", up)
	for _, c := range []struct {
		path, model, tier string
		want              any
	}{
		// translated: Responses to the plugin's Chat model, Chat to its
		// Responses one
		{"/v1/responses", "fake-1", "ultrafast", "ultrafast"},
		{"/v1/responses", "fake-1", "priority", "priority"},
		{"/v1/responses", "fake-1", "flex", nil},
		{"/v1/chat/completions", "fake-resp", "ultrafast", "ultrafast"},
		{"/v1/chat/completions", "fake-resp", "priority", nil},
		{"/v1/responses", "fake-claude", "priority", nil},
		// relayed as the client sent it, as before
		{"/v1/chat/completions", "fake-1", "ultrafast", "ultrafast"},
		{"/v1/responses", "fake-resp", "ultrafast", "ultrafast"},
	} {
		body := `{"model":"` + pid + "/" + c.model + `","service_tier":"` + c.tier + `","stream":true,`
		if c.path == "/v1/responses" {
			body += `"input":"hi"}`
		} else {
			body += `"messages":[{"role":"user","content":"hi"}]}`
		}
		rec := httptest.NewRecorder()
		New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", c.path, strings.NewReader(body)))
		if rec.Code != 200 {
			t.Fatalf("%s %s: %d %s", c.path, c.model, rec.Code, rec.Body)
		}
		if got := up.last(t); got != c.want {
			t.Errorf("%s %s asked %s: the plugin was sent service_tier %v, want %v", c.path, c.model, c.tier, got, c.want)
		}
	}
}
