package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"slices"
	"strconv"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// listing is a relay that lists ids and answers each request with the
// model it was asked for, recording it.
func listing(t *testing.T, a *asked, ids ...string) *httptest.Server {
	t.Helper()
	s := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.HasSuffix(r.URL.Path, "/models") {
			var l struct {
				Data []map[string]string `json:"data"`
			}
			for _, id := range ids {
				l.Data = append(l.Data, map[string]string{"id": id})
			}
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(l)
			return
		}
		var b struct {
			Model string `json:"model"`
		}
		json.NewDecoder(r.Body).Decode(&b)
		a.add(b.Model)
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, `data: {"id":"x","model":`+strconv.Quote(b.Model)+
			`,"choices":[{"index":0,"delta":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],`+
			`"usage":{"prompt_tokens":5,"completion_tokens":3}}`+"\n\n")
		io.WriteString(w, "data: [DONE]\n\n")
	}))
	t.Cleanup(s.Close)
	return s
}

// A model the user lists by an id of their own (zeewell on X:
// deepseek-v4.1-flash-1017 as deepseek-v4.1-flash, grok-4.6@500k as
// grok-4.6) is that id on every route an agent reaches it by — the model
// list, a request by provider/model, by the bare id through the group
// magpie finds, both protocols, a gateway key held to it — and the relay
// is asked for it by its own id, which the usage record keeps as served.
func TestLocalModelIDOnEveryRoute(t *testing.T) {
	fresh(t)
	var seenA, seenB asked
	a := listing(t, &seenA, "deepseek-v4.1-flash-1017", "grok-4.6", "grok-4.6@500k")
	b := listing(t, &seenB, "deepseek-v4.1-flash")
	for _, r := range []struct {
		id  string
		url string
		ids []string
	}{
		{"relay-a", a.URL, []string{"deepseek-v4.1-flash-1017", "grok-4.6", "grok-4.6@500k"}},
		{"relay-b", b.URL, []string{"deepseek-v4.1-flash"}},
	} {
		if err := provider.Save(provider.Provider{ID: r.id, Name: r.id, Key: "k", Chat: r.url}); err != nil {
			t.Fatal(err)
		}
		var ms []catalog.Model
		for _, id := range r.ids {
			ms = append(ms, catalog.Model{ID: id})
		}
		if err := catalog.SaveLive(r.id, r.url, ms); err != nil {
			t.Fatal(err)
		}
	}
	keys, secrets := newCaller(t, "Held")
	if _, err := access.Update("models-key", access.Change{Key: keys[0].ID, Models: []string{"relay-a/grok-4.6@500k"}}); err != nil {
		t.Fatal(err)
	}
	for ref, id := range map[string]string{
		"relay-a/deepseek-v4.1-flash-1017": "deepseek-v4.1-flash",
		"relay-a/grok-4.6@500k":            "grok-4.6",
	} {
		if _, _, err := provider.SetModelID(ref, id); err != nil {
			t.Fatal(err)
		}
	}
	// the key's model went to the id it is listed by
	if ks, _ := access.List(); !slices.Equal(ks[len(ks)-1].Models, []string{"relay-a/grok-4.6"}) {
		t.Fatalf("the held key's models: %v", ks[len(ks)-1].Models)
	}

	h := New().Handler()
	do := func(secret, method, path, body string) *httptest.ResponseRecorder {
		t.Helper()
		r := httptest.NewRequest(method, path, strings.NewReader(body))
		if secret != "" {
			r.Header.Set("Authorization", "Bearer "+secret)
		}
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		return w
	}
	listed := func(secret string) []string {
		t.Helper()
		var l struct{ Data []struct{ ID string } }
		w := do(secret, "GET", "/v1/models", "")
		if err := json.Unmarshal(w.Body.Bytes(), &l); err != nil {
			t.Fatal(w.Body.String())
		}
		var ids []string
		for _, m := range l.Data {
			ids = append(ids, m.ID)
		}
		return ids
	}
	ids := listed("")
	for _, want := range []string{"relay-a/deepseek-v4.1-flash", "relay-a/grok-4.6", "relay-b/deepseek-v4.1-flash", "auto/deepseek-v4.1-flash"} {
		if !slices.Contains(ids, want) {
			t.Errorf("/v1/models has no %s: %v", want, ids)
		}
	}
	for _, own := range []string{"relay-a/deepseek-v4.1-flash-1017", "relay-a/grok-4.6@500k"} {
		if slices.Contains(ids, own) {
			t.Errorf("/v1/models lists %s by its own id: %v", own, ids)
		}
	}

	chat := func(secret, model string) *httptest.ResponseRecorder {
		return do(secret, "POST", "/v1/chat/completions", `{"model":"`+model+`","stream":true,"messages":[{"role":"user","content":"hi"}]}`)
	}
	messages := func(model string) *httptest.ResponseRecorder {
		return do("", "POST", "/v1/messages", `{"model":"`+model+`","max_tokens":64,"stream":true,"messages":[{"role":"user","content":"hi"}]}`)
	}
	for _, c := range []struct {
		what  string
		send  func() *httptest.ResponseRecorder
		seen  *asked
		asked string
		model string // recorded
	}{
		{"chat relay-a/deepseek-v4.1-flash", func() *httptest.ResponseRecorder { return chat("", "relay-a/deepseek-v4.1-flash") }, &seenA, "deepseek-v4.1-flash-1017", "deepseek-v4.1-flash"},
		{"messages relay-a/grok-4.6", func() *httptest.ResponseRecorder { return messages("relay-a/grok-4.6") }, &seenA, "grok-4.6@500k", "grok-4.6"},
		{"an old ref by the own id", func() *httptest.ResponseRecorder { return chat("", "relay-a/deepseek-v4.1-flash-1017") }, &seenA, "deepseek-v4.1-flash-1017", "deepseek-v4.1-flash-1017"},
		{"relay-b's own", func() *httptest.ResponseRecorder { return chat("", "relay-b/deepseek-v4.1-flash") }, &seenB, "deepseek-v4.1-flash", "deepseek-v4.1-flash"},
		{"the held key's model", func() *httptest.ResponseRecorder { return chat(secrets[0], "relay-a/grok-4.6") }, &seenA, "grok-4.6@500k", "grok-4.6"},
	} {
		if w := c.send(); w.Code != 200 {
			t.Fatalf("%s: %d %s", c.what, w.Code, w.Body.String())
		}
		c.seen.wire(t, c.what, []string{c.asked})
		if u := lastUsage(t); u.Provider == "" || u.Model != c.model || u.Served != c.asked {
			t.Errorf("%s recorded as %s %s served %s, want %s served %s", c.what, u.Provider, u.Model, u.Served, c.model, c.asked)
		}
	}

	// the bare id is the group magpie found over both relays
	gid, ok := provider.GroupFor("deepseek-v4.1-flash")
	if !ok {
		t.Fatal("no found group for deepseek-v4.1-flash")
	}
	_, ms, _ := provider.FindGroup(gid)
	var members []string
	for _, m := range ms {
		members = append(members, m.Provider.ID+"/"+m.Model)
	}
	slices.Sort(members)
	if want := []string{"relay-a/deepseek-v4.1-flash", "relay-b/deepseek-v4.1-flash"}; !slices.Equal(members, want) {
		t.Fatalf("%s has %v, want %v", gid, members, want)
	}
	if w := chat("", "deepseek-v4.1-flash"); w.Code != 200 {
		t.Fatalf("the bare id: %d %s", w.Code, w.Body.String())
	}
	got := append(seenA.all(), seenB.all()...)
	if len(got) != 1 || got[0] != "deepseek-v4.1-flash-1017" && got[0] != "deepseek-v4.1-flash" {
		t.Fatalf("the bare id asked %v", got)
	}

	// the held key: shown only its model, refused another
	if ids := listed(secrets[0]); !slices.Contains(ids, "relay-a/grok-4.6") || slices.Contains(ids, "relay-a/deepseek-v4.1-flash") {
		t.Fatalf("the held key lists %v", ids)
	}
	if w := chat(secrets[0], "relay-a/deepseek-v4.1-flash"); w.Code == 200 {
		t.Fatal("the held key used a model it may not")
	}
	// a key held to the own id (written by hand, or before) is held to
	// the model by either id
	if _, err := access.Update("models-key", access.Change{Key: keys[0].ID, Models: []string{"relay-a/deepseek-v4.1-flash-1017"}}); err != nil {
		t.Fatal(err)
	}
	if w := chat(secrets[0], "relay-a/deepseek-v4.1-flash"); w.Code != 200 {
		t.Fatalf("a key held to the own id: %d %s", w.Code, w.Body.String())
	}
	if w := chat(secrets[0], "relay-a/grok-4.6"); w.Code == 200 {
		t.Fatal("a key held to deepseek used grok")
	}
}
