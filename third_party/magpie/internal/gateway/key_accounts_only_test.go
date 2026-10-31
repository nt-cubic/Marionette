package gateway

import (
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/provider"
)

// A gateway key that names an account and no model (Linka on Discord:
// "Models this key may use" with only an account ticked) may use only
// that account's provider's models, on every route a model reaches it
// by: the model lists, a chat on each API, counting tokens, embeddings,
// pictures, Codex's backend, System One, a group with another
// provider's model in it and a fallback to one. Before, the account held
// only its own provider, and the key could list and use every model of
// every other provider.
func TestGatewayKeyAccountsOnlyHoldTheModels(t *testing.T) {
	fresh(t)
	plan := &fake{t: t, ctype: "application/json", reply: `{"id":"from-plan","choices":[]}`}
	spare := &fake{t: t, ctype: "application/json", reply: `{"id":"from-spare","choices":[]}`}
	twoProviders(t, plan, spare)
	var decided int
	jev := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		decided++
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"answers":{"intent":{"choice":"bug"}},"usage":{"input_tokens":10,"output_tokens":2}}`)
	}))
	t.Cleanup(jev.Close)
	if err := provider.Save(provider.Provider{ID: "jev", Name: "Jev", Key: "k", Decide: jev.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	var drew int
	art := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		drew++
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"created":1,"data":[{"b64_json":"`+base64.StdEncoding.EncodeToString(pngBytes)+`"}]}`)
	}))
	t.Cleanup(art.Close)
	if err := provider.Save(provider.Provider{ID: "art", Name: "Art", Chat: art.URL + "/v1", Key: "kA", Models: []string{"gpt-image-1"}}); err != nil {
		t.Fatal(err)
	}
	for _, g := range []provider.Group{
		{Name: "Both", Members: []string{"plan/m1", "spare/m2"}, Routing: provider.Ordered},
		{Name: "Solo", Members: []string{"plan/m1"}, Routing: provider.Ordered},
	} {
		if err := provider.SaveGroup(g); err != nil {
			t.Fatal(err)
		}
	}
	keys, secrets := newCaller(t, "Held", "Free")
	if _, err := access.Update("accounts-key", access.Change{Key: keys[0].ID, Accounts: []string{"plan/" + provider.KeyID("k")}}); err != nil {
		t.Fatal(err)
	}
	if ks, _ := access.List(); len(ks[len(ks)-2].Models) != 0 {
		t.Fatalf("the key's own models were written: %+v", ks[len(ks)-2])
	}
	h := New().Handler()
	do := func(secret, method, path, body string) *httptest.ResponseRecorder {
		r := httptest.NewRequest(method, path, strings.NewReader(body))
		r.Header.Set("Authorization", "Bearer "+secret)
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		return w
	}
	refused := func(what string, w *httptest.ResponseRecorder) {
		t.Helper()
		if w.Code != 403 || !strings.Contains(w.Body.String(), "Held") {
			t.Errorf("%s: %d %s", what, w.Code, w.Body.String())
		}
	}

	// the lists: the account's provider's models and a group of them only
	var l struct{ Data []struct{ ID string } }
	if w := do(secrets[0], "GET", "/v1/models", ""); json.Unmarshal(w.Body.Bytes(), &l) != nil {
		t.Fatal(w.Body.String())
	}
	var ids []string
	for _, m := range l.Data {
		ids = append(ids, m.ID)
	}
	if !slices.Contains(ids, "plan/m1") || !slices.Contains(ids, "group/solo") || slices.Contains(ids, "spare/m2") || slices.Contains(ids, "group/both") || slices.Contains(ids, "jev/jev-latest") {
		t.Errorf("/v1/models lists %v", ids)
	}
	if w := do(secrets[0], "GET", "/v1beta/models", ""); w.Code != 200 || !strings.Contains(w.Body.String(), "m1") || strings.Contains(w.Body.String(), "m2") {
		t.Errorf("gemini list %d %s", w.Code, w.Body.String())
	}
	if w := do(secrets[0], "GET", "/v1/models/spare/m2", ""); w.Code == 200 {
		t.Error("a model of another provider was found", w.Body.String())
	}

	// a request for another provider's model, on every API
	for _, model := range []string{"spare/m2", "m2", "group/both"} {
		refused("chat "+model, do(secrets[0], "POST", "/v1/chat/completions", `{"model":"`+model+`","messages":[{"role":"user","content":"hi"}]}`))
	}
	refused("responses", do(secrets[0], "POST", "/v1/responses", `{"model":"spare/m2","input":"hi"}`))
	refused("messages", do(secrets[0], "POST", "/v1/messages", `{"model":"spare/m2","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}`))
	refused("gemini", do(secrets[0], "POST", "/v1beta/models/spare/m2:generateContent", `{"contents":[{"role":"user","parts":[{"text":"hi"}]}]}`))
	refused("count_tokens", do(secrets[0], "POST", "/v1/messages/count_tokens", `{"model":"spare/m2","messages":[{"role":"user","content":"hi"}]}`))
	refused("countTokens", do(secrets[0], "POST", "/v1beta/models/spare/m2:countTokens", `{"contents":[{"role":"user","parts":[{"text":"hi"}]}]}`))
	refused("embeddings", do(secrets[0], "POST", "/v1/embeddings", `{"model":"spare/m2","input":"hi"}`))
	refused("codex backend", do(secrets[0], "POST", CodexPath+"/responses", `{"model":"spare/m2","input":"hi","stream":true}`))
	refused("system one", do(secrets[0], "POST", "/v1/systemone", `{"model":"jev/jev-latest","state":{"message":"hi"},"questions":{"intent":{"type":"choice"}}}`))
	refused("images", do(secrets[0], "POST", "/v1/images/generations", `{"model":"art/gpt-image-1","prompt":"a magpie"}`))
	if spare.calls != 0 || decided != 0 || drew != 0 {
		t.Fatal("a held request reached another provider", spare.calls, decided, drew)
	}
	if w := do(secrets[1], "POST", "/v1/images/generations", `{"model":"art/gpt-image-1","prompt":"a magpie"}`); w.Code != 200 || drew != 1 {
		t.Errorf("a free key's drawing: %d %s", w.Code, w.Body.String())
	}

	// its own provider's models are served, a group of them too
	for _, model := range []string{"plan/m1", "m1", "group/solo"} {
		if w := do(secrets[0], "POST", "/v1/chat/completions", `{"model":"`+model+`","messages":[{"role":"user","content":"hi"}]}`); w.Code != 200 || !strings.Contains(w.Body.String(), "from-plan") {
			t.Errorf("%s: %d %s", model, w.Code, w.Body.String())
		}
	}
	// a fallback to another provider's model is not the key's
	plan.code, plan.reply = 429, `{"error":{"message":"slow down"}}`
	if w := do(secrets[0], "POST", "/v1/chat/completions", chatReq); w.Code == 200 || spare.calls != 0 {
		t.Errorf("a held key fell back to another provider: %d %s", w.Code, w.Body.String())
	}
	// a free key is served everywhere, the fallback too
	if w := do(secrets[1], "POST", "/v1/chat/completions", chatReq); w.Code != 200 || !strings.Contains(w.Body.String(), "from-spare") {
		t.Errorf("a free key's fallback: %d %s", w.Code, w.Body.String())
	}
	plan.code, plan.reply = 0, `{"id":"from-plan","choices":[]}`

	// a key that lists models keeps them: its account holds only the
	// provider it names, as before (#905)
	if _, err := access.Update("models-key", access.Change{Key: keys[0].ID, Models: []string{"spare/*"}}); err != nil {
		t.Fatal(err)
	}
	if w := do(secrets[0], "POST", "/v1/chat/completions", `{"model":"spare/m2","messages":[{"role":"user","content":"hi"}]}`); w.Code != 200 || !strings.Contains(w.Body.String(), "from-spare") {
		t.Errorf("a listed model: %d %s", w.Code, w.Body.String())
	}
	// and one let off its accounts uses every model again
	if _, err := access.Update("models-key", access.Change{Key: keys[0].ID}); err != nil {
		t.Fatal(err)
	}
	if _, err := access.Update("accounts-key", access.Change{Key: keys[0].ID}); err != nil {
		t.Fatal(err)
	}
	if w := do(secrets[0], "POST", "/v1/chat/completions", `{"model":"spare/m2","messages":[{"role":"user","content":"hi"}]}`); w.Code != 200 {
		t.Errorf("a key let off: %d %s", w.Code, w.Body.String())
	}
}
