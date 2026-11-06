package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/provider"
)

// keyedUpstream answers chat, messages and count_tokens, each reply
// naming the key it came in on, and records the keys it was asked on.
// refuse, when set, answers that key with a 429.
type keyedUpstream struct {
	mu     sync.Mutex
	keys   []string
	refuse map[string]bool
}

func (u *keyedUpstream) asked() []string {
	u.mu.Lock()
	defer u.mu.Unlock()
	return append([]string(nil), u.keys...)
}

func (u *keyedUpstream) reset() {
	u.mu.Lock()
	u.keys = nil
	u.mu.Unlock()
}

func (u *keyedUpstream) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	body, _ := io.ReadAll(r.Body)
	key := r.Header.Get("x-api-key")
	if key == "" {
		key = strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer ")
	}
	u.mu.Lock()
	u.keys = append(u.keys, key)
	refused := u.refuse[key]
	u.mu.Unlock()
	if refused {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(429)
		io.WriteString(w, `{"error":{"message":"rate limited","type":"rate_limit_error"}}`)
		return
	}
	stream := strings.Contains(string(body), `"stream":true`)
	text := "from " + key
	switch {
	case strings.HasSuffix(r.URL.Path, "/count_tokens"):
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"input_tokens":5}`)
	case strings.HasSuffix(r.URL.Path, "/messages") && stream:
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(
			`event: message_start`+"\n"+`data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"m","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}`,
			`event: content_block_start`+"\n"+`data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}`,
			`event: content_block_delta`+"\n"+`data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"`+text+`"}}`,
			`event: content_block_stop`+"\n"+`data: {"type":"content_block_stop","index":0}`,
			`event: message_delta`+"\n"+`data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}`,
			`event: message_stop`+"\n"+`data: {"type":"message_stop"}`))
	case strings.HasSuffix(r.URL.Path, "/messages"):
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"msg_1","type":"message","role":"assistant","model":"m","content":[{"type":"text","text":"`+text+`"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}`)
	case stream:
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(
			`data: {"id":"c1","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"role":"assistant","content":"`+text+`"}}]}`,
			`data: {"id":"c1","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}`,
			`data: [DONE]`))
	default:
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"c1","object":"chat.completion","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"`+text+`"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}`)
	}
}

// groupAccountsSetup: a relay "relay" with a personal key and a company
// key, speaking chat and messages, and a second provider "other" with
// one key, both serving m.
func groupAccountsSetup(t *testing.T) (relay, other *keyedUpstream) {
	t.Helper()
	fresh(t)
	t.Cleanup(noRests)
	relay, other = &keyedUpstream{}, &keyedUpstream{}
	ru, ou := httptest.NewServer(relay), httptest.NewServer(other)
	t.Cleanup(ru.Close)
	t.Cleanup(ou.Close)
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Chat: ru.URL + "/v1", Anthropic: ru.URL, Key: "sk-mine",
		Keys: []provider.KeyAccount{{Key: "sk-work"}}, Models: []string{"m"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.Save(provider.Provider{ID: "other", Name: "Other", Chat: ou.URL + "/v1", Anthropic: ou.URL, Key: "sk-other", Models: []string{"m"}}); err != nil {
		t.Fatal(err)
	}
	return relay, other
}

func askGroup(h http.Handler, secret, path, body string) *httptest.ResponseRecorder {
	rec := httptest.NewRecorder()
	req := httptest.NewRequest("POST", path, strings.NewReader(body))
	if secret != "" {
		req.Header.Set("Authorization", "Bearer "+secret)
	}
	h.ServeHTTP(rec, req)
	return rec
}

// every route a request through a group can take, each one asked for
// the model given
func groupRoutes(model string) [][2]string {
	return [][2]string{
		{"/v1/chat/completions", `{"model":"` + model + `","messages":[{"role":"user","content":"hi"}]}`},
		{"/v1/chat/completions", `{"model":"` + model + `","stream":true,"messages":[{"role":"user","content":"hi"}]}`},
		{"/v1/responses", `{"model":"` + model + `","input":"hi"}`},
		{"/v1/responses", `{"model":"` + model + `","stream":true,"input":"hi"}`},
		{"/v1/messages", `{"model":"` + model + `","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}`},
		{"/v1/messages", `{"model":"` + model + `","max_tokens":10,"stream":true,"messages":[{"role":"user","content":"hi"}]}`},
		{"/v1/messages/count_tokens", `{"model":"` + model + `","messages":[{"role":"user","content":"hi"}]}`},
	}
}

// A routing group held to some accounts of a provider (duanckham on X: a
// personal GPT account and a company one, a group for each) is served by
// those alone, on every route: chat, responses and messages, streamed or
// not, and count_tokens. The same model through a group without the
// setting is served as before, by the provider's first key.
func TestGroupAccountsHoldEveryRoute(t *testing.T) {
	relay, _ := groupAccountsSetup(t)
	work := "relay/" + provider.KeyID("sk-work")
	if err := provider.SaveGroup(provider.Group{Name: "Work", Members: []string{"relay/m"}, Routing: provider.Ordered, Accounts: []string{work}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "Mine", Members: []string{"relay/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	if g, _, _ := provider.FindGroup("group/work"); len(g.Accounts) != 1 || g.Accounts[0] != work {
		t.Fatalf("saved accounts %v", g.Accounts)
	}
	h := New().Handler()
	for _, rt := range groupRoutes("group/work") {
		for range 2 {
			relay.reset()
			w := askGroup(h, "", rt[0], rt[1])
			if w.Code != 200 {
				t.Fatalf("%s %s: %d %s", rt[0], rt[1], w.Code, w.Body.String())
			}
			asked := relay.asked()
			if len(asked) == 0 && !strings.HasSuffix(rt[0], "/count_tokens") {
				t.Fatalf("%s %s: no upstream asked", rt[0], rt[1])
			}
			for _, k := range asked {
				if k != "sk-work" {
					t.Fatalf("%s %s: the group spent %s, asked %v", rt[0], rt[1], k, asked)
				}
			}
			if len(asked) > 0 && !strings.HasSuffix(rt[0], "/count_tokens") && !strings.Contains(w.Body.String(), "from sk-work") {
				t.Fatalf("%s %s: answered %s", rt[0], rt[1], w.Body.String())
			}
		}
	}
	// a group without the setting: as before, the provider's first key
	relay.reset()
	if w := askGroup(h, "", "/v1/chat/completions", `{"model":"group/mine","messages":[{"role":"user","content":"hi"}]}`); w.Code != 200 || !strings.Contains(w.Body.String(), "from sk-mine") {
		t.Fatalf("an unheld group: %d %s", w.Code, w.Body.String())
	}
	// and the model asked by itself is not held by any group
	relay.reset()
	if w := askGroup(h, "", "/v1/chat/completions", `{"model":"relay/m","messages":[{"role":"user","content":"hi"}]}`); w.Code != 200 || !strings.Contains(w.Body.String(), "from sk-mine") {
		t.Fatalf("the bare model: %d %s", w.Code, w.Body.String())
	}
	// the route's left says why the other key was held out, and by whom
	var held *Weighed
	h2 := New()
	relay.reset()
	if w := askGroup(h2.Handler(), "", "/v1/chat/completions", `{"model":"group/work","messages":[{"role":"user","content":"hi"}]}`); w.Code != 200 {
		t.Fatalf("work: %d %s", w.Code, w.Body.String())
	}
	r := h2.trace.routes[len(h2.trace.routes)-1]
	for i := range r.Left {
		if r.Left[i].Held {
			held = &r.Left[i]
		}
	}
	if held == nil || held.HeldBy != "Work" {
		t.Fatalf("route left %+v", r.Left)
	}
}

// A group's allowed account resting or refusing never sends the request
// to an account it excluded: with another member it falls back to that
// member, alone it answers the error a group returns today. A group in
// it holds its own members the same way.
func TestGroupAccountsFallBackWithinTheGroup(t *testing.T) {
	relay, other := groupAccountsSetup(t)
	work := "relay/" + provider.KeyID("sk-work")
	relay.refuse = map[string]bool{"sk-work": true}
	if err := provider.SaveGroup(provider.Group{Name: "Alone", Members: []string{"relay/m"}, Routing: provider.Ordered, Accounts: []string{work}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "Pair", Members: []string{"relay/m", "other/m"}, Routing: provider.Ordered, Accounts: []string{work}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "Outer", Members: []string{"group/alone"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	h := New().Handler()
	chat := func(model string) *httptest.ResponseRecorder {
		return askGroup(h, "", "/v1/chat/completions", `{"model":"`+model+`","messages":[{"role":"user","content":"hi"}]}`)
	}
	spent := func(where string) {
		t.Helper()
		for _, k := range relay.asked() {
			if k != "sk-work" {
				t.Fatalf("%s: an excluded account was spent: %v", where, relay.asked())
			}
		}
	}
	// alone: the refusal, twice (the second time the key rests), never
	// the personal key
	for i := range 2 {
		relay.reset()
		w := chat("group/alone")
		if w.Code < 400 {
			t.Fatalf("alone %d: %d %s", i, w.Code, w.Body.String())
		}
		spent("alone")
	}
	// through a group in a group: held the same
	relay.reset()
	if w := chat("group/outer"); w.Code < 400 {
		t.Fatalf("outer: %d %s", w.Code, w.Body.String())
	}
	spent("outer")
	// with another member: that member answers
	noRests()
	relay.reset()
	other.reset()
	if w := chat("group/pair"); w.Code != 200 || !strings.Contains(w.Body.String(), "from sk-other") {
		t.Fatalf("pair: %d %s", w.Code, w.Body.String())
	}
	spent("pair")
	if len(other.asked()) != 1 {
		t.Fatalf("other asked %v", other.asked())
	}
	// the work key back: the pair is served by it, not by other
	relay.refuse = nil
	noRests()
	sticks.Lock()
	sticks.m = map[string]stick{}
	sticks.Unlock()
	relay.reset()
	if w := chat("group/pair"); w.Code != 200 || !strings.Contains(w.Body.String(), "from sk-work") {
		t.Fatalf("pair, work back: %d %s", w.Code, w.Body.String())
	}
}

// A group held to accounts a gateway key may not use is a 403 that names
// the group and its accounts, and reaches no provider; a key and a group
// that share an account are served by it.
func TestGroupAccountsAndAKeysAccounts(t *testing.T) {
	relay, _ := groupAccountsSetup(t)
	work, mine := "relay/"+provider.KeyID("sk-work"), "relay/"+provider.KeyID("sk-mine")
	if err := provider.SaveGroup(provider.Group{Name: "Work", Members: []string{"relay/m"}, Routing: provider.Ordered, Accounts: []string{work}}); err != nil {
		t.Fatal(err)
	}
	keys, secrets := newCaller(t, "Personal")
	if _, err := access.Update("accounts-key", access.Change{Key: keys[0].ID, Accounts: []string{mine}}); err != nil {
		t.Fatal(err)
	}
	h := New().Handler()
	relay.reset()
	w := askGroup(h, secrets[0], "/v1/chat/completions", `{"model":"group/work","messages":[{"role":"user","content":"hi"}]}`)
	var e struct {
		Error struct{ Message, Type string }
	}
	json.Unmarshal(w.Body.Bytes(), &e)
	if w.Code != 403 || !strings.Contains(e.Error.Message, "Work") || !strings.Contains(e.Error.Message, "Routing") {
		t.Fatalf("a key and a group apart: %d %s", w.Code, w.Body.String())
	}
	if len(relay.asked()) != 0 {
		t.Fatalf("held apart, asked %v", relay.asked())
	}
	// both may use the work key
	if _, err := access.Update("accounts-key", access.Change{Key: keys[0].ID, Accounts: []string{mine, work}}); err != nil {
		t.Fatal(err)
	}
	relay.reset()
	if w := askGroup(h, secrets[0], "/v1/chat/completions", `{"model":"group/work","messages":[{"role":"user","content":"hi"}]}`); w.Code != 200 || !strings.Contains(w.Body.String(), "from sk-work") {
		t.Fatalf("a key and a group together: %d %s", w.Code, w.Body.String())
	}
}

// SaveGroup takes an account by its name or id, refuses one the provider
// doesn't have, and a shared group carries no accounts: they are this
// magpie's.
func TestGroupAccountsSaved(t *testing.T) {
	groupAccountsSetup(t)
	if err := provider.SaveGroup(provider.Group{Name: "Bad", Members: []string{"relay/m"}, Accounts: []string{"relay/nobody"}}); err == nil {
		t.Fatal("an account the provider lacks was saved")
	}
	if err := provider.SaveGroup(provider.Group{Name: "Bad", Members: []string{"relay/m"}, Accounts: []string{"nothing"}}); err == nil {
		t.Fatal("an entry with no provider was saved")
	}
	work := "relay/" + provider.KeyID("sk-work")
	if err := provider.SaveGroup(provider.Group{Name: "Work", Members: []string{"relay/m"}, Accounts: []string{work, work}}); err != nil {
		t.Fatal(err)
	}
	if g, _, _ := provider.FindGroup("group/work"); len(g.Accounts) != 1 {
		t.Fatalf("twice saved as %v", g.Accounts)
	}
	text, err := provider.ShareGroup("work")
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(text, provider.KeyID("sk-work")) {
		t.Fatalf("shared with its accounts: %s", text)
	}
	// one shared with accounts in it (by hand, or an older magpie's)
	// is imported without them
	text = strings.Replace(text, `"name":"Work"`, `"name":"Imported","accounts":["`+work+`"]`, 1)
	if !strings.Contains(text, `"accounts"`) {
		t.Fatalf("no accounts put in %s", text)
	}
	if _, err := provider.ImportGroup(text); err != nil {
		t.Fatal(err)
	}
	for _, g := range provider.Groups() {
		if g.Name == "Imported" && len(g.Accounts) != 0 {
			t.Fatalf("imported with accounts %v", g.Accounts)
		}
	}
}
