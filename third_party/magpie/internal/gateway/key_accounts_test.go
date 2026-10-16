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
	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// A gateway key held to some accounts (#905) is served by those alone:
// the account it names, the other one never tried, though the plan held
// both; a group it names is its members' still only through the accounts
// it may use; every candidate held out is said so on the route's left.
// When the accounts leave the plan nothing — the capped, the barred and
// the not-ready refusals say which came first: a cap holds before the
// key's accounts, those before an account barred for the model, and that
// before no candidate at all. A free key is served as before.
func TestGatewayKeyAccountsHoldAKey(t *testing.T) {
	heads := twoAccounts(t)
	s := New()
	keys, secrets := newCaller(t, "Held", "Free")
	setAccounts := func(t *testing.T, id string, as ...string) {
		t.Helper()
		if _, err := access.Update("accounts-key", access.Change{Key: id, Accounts: as}); err != nil {
			t.Fatal(err)
		}
	}
	post := func(secret string) (int, string) {
		t.Helper()
		rec := httptest.NewRecorder()
		req := httptest.NewRequest("POST", "/v1/responses", strings.NewReader(`{"model":"codex/gpt-5.5","stream":true,"input":"ping"}`))
		req.Header.Set("Authorization", "Bearer "+secret)
		s.Handler().ServeHTTP(rec, req)
		return rec.Code, rec.Body.String()
	}
	// the account the key names serves every request of its own; the other
	// one is never tried, though the plan held both
	setAccounts(t, keys[0].ID, "codex/me@example.com")
	for range 3 {
		*heads = nil
		code, body := post(secrets[0])
		if code != 200 || !strings.Contains(body, "from acct-1") {
			t.Fatalf("held to me: %d %s", code, body)
		}
		for _, h := range *heads {
			if got := h.Get("chatgpt-account-id"); got != "acct-1" {
				t.Fatalf("the account not named was tried: %v", got)
			}
		}
		if len(*heads) != 1 {
			t.Fatalf("upstream heads %v", *heads)
		}
	}
	// the plan's left says the other account was held out
	if r := s.trace.routes[len(s.trace.routes)-1]; len(r.Left) != 1 || r.Left[0].Who != "spare@example.com" || !r.Left[0].Held || r.Left[0].Barred {
		t.Fatalf("route left %+v", r.Left)
	}
	// the other account of the provider is the key's as well
	setAccounts(t, keys[0].ID, "codex/spare@example.com")
	*heads = nil
	if code, body := post(secrets[0]); code != 200 || !strings.Contains(body, "from acct-2") {
		t.Fatalf("held to spare: %d %s", code, body)
	}
	// a group the key names is its members' — through the accounts it may
	// use only, not through the members by themselves
	if err := provider.SaveGroup(provider.Group{Name: "Gpt", Members: []string{"codex/gpt-5.5"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	if _, err := access.Update("models-key", access.Change{Key: keys[0].ID, Models: []string{"group/gpt"}}); err != nil {
		t.Fatal(err)
	}
	*heads = nil
	rec := httptest.NewRecorder()
	req := httptest.NewRequest("POST", "/v1/responses", strings.NewReader(`{"model":"group/gpt","stream":true,"input":"ping"}`))
	req.Header.Set("Authorization", "Bearer "+secrets[0])
	s.Handler().ServeHTTP(rec, req)
	if rec.Code != 200 || !strings.Contains(rec.Body.String(), "from acct-2") {
		t.Fatalf("a named group through the accounts: %d %s", rec.Code, rec.Body.String())
	}
	if len(*heads) != 1 || (*heads)[0].Get("chatgpt-account-id") != "acct-2" {
		t.Fatalf("a named group tried another account: %v", *heads)
	}
	if _, err := access.Update("models-key", access.Change{Key: keys[0].ID}); err != nil {
		t.Fatal(err)
	}
	// every candidate held out by the accounts is a 403 that names the
	// key and its accounts — before the barred refusal the plan may also
	// have: me is barred for the model, spare is not the key's
	if err := provider.SetAccountModels("codex", "me@example.com", []string{"gpt-5.4-mini"}); err != nil {
		t.Fatal(err)
	}
	setAccounts(t, keys[0].ID, "codex/me@example.com")
	*heads = nil
	code, body := post(secrets[0])
	var e struct {
		Error struct{ Message, Type string }
	}
	json.Unmarshal([]byte(body), &e)
	if code != 403 || e.Error.Type != "permission_error" || !strings.Contains(e.Error.Message, `"Held"`) ||
		!strings.Contains(e.Error.Message, "is not allowed to use the accounts behind") || !strings.Contains(e.Error.Message, "codex/me@example.com") {
		t.Fatalf("every account held out: %d %s", code, body)
	}
	if len(*heads) != 0 {
		t.Fatalf("a held-out request reached a provider: %v", *heads)
	}
	if rec := lastUsage(t); !rec.Rejected || rec.Status != 403 || rec.CallerKeyID != keys[0].ID {
		t.Fatalf("held out recorded as %+v", rec)
	}
	// a cap holds before the key's accounts: me capped, spare not the
	// key's, and the refusal is the cap's 429
	if err := provider.SetAccountModels("codex", "me@example.com", nil); err != nil {
		t.Fatal(err)
	}
	capUsage(t, map[string]float64{"me@example.com": 75})
	if err := provider.SetAccountCap("codex", "me@example.com", 70); err != nil {
		t.Fatal(err)
	}
	*heads = nil
	code, body = post(secrets[0])
	if code != 429 || !strings.Contains(body, "usage cap reached") || !strings.Contains(body, "me@example.com") {
		t.Fatalf("a cap before the accounts: %d %s", code, body)
	}
	if err := provider.SetAccountCap("codex", "me@example.com", 0); err != nil {
		t.Fatal(err)
	}
	// the barred refusal stands when the accounts held nothing out: both
	// barred, one of them the key's
	if err := provider.SetAccountModels("codex", "me@example.com", []string{"gpt-5.4-mini"}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetAccountModels("codex", "spare@example.com", []string{"gpt-5.4-mini"}); err != nil {
		t.Fatal(err)
	}
	*heads = nil
	code, body = post(secrets[0])
	if code != 403 || !strings.Contains(body, "own list of models") {
		t.Fatalf("barred before the accounts held nothing: %d %s", code, body)
	}
	if err := provider.SetAccountModels("codex", "me@example.com", nil); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetAccountModels("codex", "spare@example.com", nil); err != nil {
		t.Fatal(err)
	}
	// a free key is served as before, and one let off its accounts again
	setAccounts(t, keys[0].ID)
	for _, secret := range []string{secrets[0], secrets[1]} {
		*heads = nil
		if code, body := post(secret); code != 200 || !strings.Contains(body, "from acct-1") {
			t.Fatalf("every account: %d %s", code, body)
		}
	}
}

// A gateway key held to some accounts (#905) is shown in /v1/models only
// the models an account or key it may use serves: one every allowed
// account is set not to serve (#474) is another's, not its.
func TestGatewayKeyAccountsListModels(t *testing.T) {
	heads := twoAccounts(t)
	if err := provider.Save(provider.Provider{ID: "codex", Models: []string{"gpt-5.5", "gpt-5.4-mini"}}); err != nil {
		t.Fatal(err)
	}
	s := New()
	keys, secrets := newCaller(t, "Held")
	if _, err := access.Update("accounts-key", access.Change{Key: keys[0].ID, Accounts: []string{"codex/me@example.com"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetAccountModels("codex", "me@example.com", []string{"gpt-5.4-mini"}); err != nil {
		t.Fatal(err)
	}
	list := func(secret string) string {
		t.Helper()
		r := httptest.NewRequest("GET", "/v1/models", nil)
		if secret != "" {
			r.Header.Set("Authorization", "Bearer "+secret)
		} else {
			r.RemoteAddr = "127.0.0.1:5000" // this computer asks without a key
		}
		w := httptest.NewRecorder()
		s.Handler().ServeHTTP(w, r)
		return w.Body.String()
	}
	if got := list(secrets[0]); strings.Contains(got, `"codex/gpt-5.5"`) || !strings.Contains(got, `"codex/gpt-5.4-mini"`) {
		t.Fatalf("the key's models: %s", got)
	}
	// a provider with no account and no key (a local ollama) is one the
	// key names no account of: its models stay listed, as its requests
	// pass (#905)
	if err := provider.Save(provider.Provider{ID: "local", Name: "Local", Chat: "http://127.0.0.1:11434/v1", Models: []string{"tiny"}}); err != nil {
		t.Fatal(err)
	}
	if err := catalog.SaveLive("local", "http://127.0.0.1:11434/v1", []catalog.Model{{ID: "tiny"}}); err != nil {
		t.Fatal(err)
	}
	if got := list(secrets[0]); !strings.Contains(got, `"local/tiny"`) {
		t.Fatalf("a provider with no account or key: %s", got)
	}
	// an entry kept after its provider lost its last key holds the key
	// closer: the provider it names serves none of its models
	if err := catalog.SaveLive("relay", "http://127.0.0.1:2/v1", []catalog.Model{{ID: "m"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Chat: "http://127.0.0.1:2/v1", Key: "sk-r", Models: []string{"m"}}); err != nil {
		t.Fatal(err)
	}
	if _, err := access.Update("accounts-key", access.Change{Key: keys[0].ID, Accounts: []string{"codex/me@example.com", "relay/" + provider.KeyID("sk-r")}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Chat: "http://127.0.0.1:2/v1", Models: []string{"m"}}); err != nil {
		t.Fatal(err)
	}
	if got := list(secrets[0]); strings.Contains(got, `"relay/m"`) || !strings.Contains(got, `"local/tiny"`) {
		t.Fatalf("a named provider without a key: %s", got)
	}
	// a keyless request sees both, and the accounts hold no model back
	if got := list(""); !strings.Contains(got, `"codex/gpt-5.5"`) || !strings.Contains(got, `"codex/gpt-5.4-mini"`) {
		t.Fatalf("a keyless list: %s", got)
	}
	if len(*heads) != 0 {
		t.Fatal("a list reached a provider", *heads)
	}
}

// A gateway key held to some accounts (#905) is drawn with by those alone:
// the whole fan-out of a subscription's accounts, not only its first. One
// of the key's whose plan refuses to draw doesn't hand the request to an
// account it may not use; the second in the order, when it is the key's,
// draws what the first may not; a keyless request fans out as before.
func TestGatewayKeyAccountsDrawOnAllowedAccounts(t *testing.T) {
	var drew []string
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		acct := r.Header.Get("chatgpt-account-id")
		drew = append(drew, acct)
		w.Header().Set("Content-Type", "application/json")
		if acct == "acct-1" { // a plan without images, as chatgpt.com refuses
			w.WriteHeader(http.StatusForbidden)
			io.WriteString(w, `{"detail":{"message":"Forbidden"}}`)
			return
		}
		io.WriteString(w, `{"created":1,"data":[{"b64_json":"eA=="}]}`)
	}))
	t.Cleanup(up.Close)
	codexSignedIn(t, "spare@example.com")
	was := provider.CodexBase
	provider.CodexBase = up.URL + "/backend-api/codex"
	t.Cleanup(func() { provider.CodexBase = was })
	if err := provider.Save(provider.Provider{ID: "codex", Models: []string{"gpt-image-2"}}); err != nil {
		t.Fatal(err)
	}
	s := New()
	keys, secrets := newCaller(t, "Held")
	setAccounts := func(as ...string) {
		t.Helper()
		if _, err := access.Update("accounts-key", access.Change{Key: keys[0].ID, Accounts: as}); err != nil {
			t.Fatal(err)
		}
	}
	draw := func(secret string) *httptest.ResponseRecorder {
		r := httptest.NewRequest("POST", "/v1/images/generations", strings.NewReader(`{"model":"codex/gpt-image-2","prompt":"a magpie"}`))
		r.Header.Set("Authorization", "Bearer "+secret)
		w := httptest.NewRecorder()
		s.Handler().ServeHTTP(w, r)
		return w
	}
	// the key's first account's plan refuses: the refusal stands, the
	// other account — one the key may not use — is never handed the pen
	setAccounts("codex/me@example.com")
	if w := draw(secrets[0]); w.Code == 200 || strings.Contains(w.Body.String(), "b64_json") {
		t.Fatalf("a refusing account drew: %d %s", w.Code, w.Body.String())
	}
	if !slices.Equal(drew, []string{"acct-1"}) {
		t.Fatalf("the fan-out reached an account the key may not use: %v", drew)
	}
	// the second account, when it is the key's, draws what the first may not
	setAccounts("codex/spare@example.com")
	drew = nil
	if w := draw(secrets[0]); w.Code != 200 || !strings.Contains(w.Body.String(), "b64_json") {
		t.Fatalf("the allowed account didn't draw: %d %s", w.Code, w.Body.String())
	}
	if !slices.Equal(drew, []string{"acct-2"}) {
		t.Fatalf("another account was tried: %v", drew)
	}
	// a keyless request fans out to the next account, as before
	drew = nil
	r := httptest.NewRequest("POST", "/v1/images/generations", strings.NewReader(`{"model":"codex/gpt-image-2","prompt":"a magpie"}`))
	r.RemoteAddr = "127.0.0.1:5000"
	w := httptest.NewRecorder()
	s.Handler().ServeHTTP(w, r)
	if w.Code != 200 || !slices.Equal(drew, []string{"acct-1", "acct-2"}) {
		t.Fatalf("a keyless fan-out: %d %s, drew %v", w.Code, w.Body.String(), drew)
	}
}

// A gateway key held to some accounts (#905) counts tokens on those alone:
// the counting candidates — a relay's keys, each an account the key may
// name — are held to the key's accounts as the serving ones are, none but
// the local estimate when it may use none. Gemini's counting, answered by
// the gateway itself, reaches no provider either way.
func TestGatewayKeyAccountsCountTokens(t *testing.T) {
	fresh(t)
	var counted []string
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		counted = append(counted, strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer "))
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"input_tokens":5}`)
	}))
	t.Cleanup(up.Close)
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Anthropic: up.URL, Key: "sk-0",
		Keys: []provider.KeyAccount{{Key: "sk-A"}, {Key: "sk-B"}}, Models: []string{"m"}}); err != nil {
		t.Fatal(err)
	}
	keys, secrets := newCaller(t, "Held")
	setAccounts := func(as ...string) {
		t.Helper()
		if _, err := access.Update("accounts-key", access.Change{Key: keys[0].ID, Accounts: as}); err != nil {
			t.Fatal(err)
		}
	}
	h := New().Handler()
	count := func(path, body string) *httptest.ResponseRecorder {
		r := httptest.NewRequest("POST", path, strings.NewReader(body))
		r.Header.Set("Authorization", "Bearer "+secrets[0])
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		return w
	}
	// only the key the key may use counts
	for _, k := range []string{"sk-A", "sk-B"} {
		setAccounts("relay/" + provider.KeyID(k))
		counted = nil
		w := count("/v1/messages/count_tokens", `{"model":"relay/m","messages":[{"role":"user","content":"hi"}]}`)
		if w.Code != 200 || !strings.Contains(w.Body.String(), "input_tokens") {
			t.Fatalf("counted on %s: %d %s", k, w.Code, w.Body.String())
		}
		if !slices.Equal(counted, []string{k}) {
			t.Fatalf("counted on %s by %v", k, counted)
		}
	}
	// the one key it may use, set not to serve the model: the local
	// estimate, no provider asked
	if err := provider.SetAccountModels("relay", provider.KeyID("sk-A"), []string{"other"}); err != nil {
		t.Fatal(err)
	}
	setAccounts("relay/" + provider.KeyID("sk-A"))
	counted = nil
	w := count("/v1/messages/count_tokens", `{"model":"relay/m","messages":[{"role":"user","content":"hi"}]}`)
	if w.Code != 200 || !strings.Contains(w.Body.String(), "input_tokens") || len(counted) != 0 {
		t.Fatalf("the local estimate: %d %s, asked %v", w.Code, w.Body.String(), counted)
	}
	// Gemini's counting, the model in its URL: the gateway's own estimate
	counted = nil
	w = count("/v1beta/models/relay/m:countTokens", `{"contents":[{"role":"user","parts":[{"text":"hi"}]}]}`)
	if w.Code != 200 || !strings.Contains(w.Body.String(), "totalTokens") || len(counted) != 0 {
		t.Fatalf("gemini counting: %d %s, asked %v", w.Code, w.Body.String(), counted)
	}
}

// magpie's own calls for a held key — a Codex title, of the model Settings
// names — are of the models the user picked, so the models don't hold them,
// but their spend lands on an account: the accounts hold them too (#905),
// and the title is written by an account the key may use alone.
func TestGatewayKeyAccountsHoldASideCall(t *testing.T) {
	var heads []string
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		heads = append(heads, r.Header.Get("chatgpt-account-id"))
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(`data: {"type":"response.completed","response":{"id":"r1","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"{\"title\":\"Fix login bug\"}"}]}],"usage":{"input_tokens":9,"output_tokens":2}}}`))
	}))
	t.Cleanup(up.Close)
	codexSignedIn(t, "spare@example.com")
	was := provider.CodexBase
	provider.CodexBase = up.URL + "/backend-api/codex"
	t.Cleanup(func() { provider.CodexBase = was })
	if err := provider.Save(provider.Provider{ID: "codex", Models: []string{"gpt-5.5"}}); err != nil {
		t.Fatal(err)
	}
	st := settings.Load()
	st.CodexTitles = "codex/gpt-5.5"
	if err := settings.Save(st); err != nil {
		t.Fatal(err)
	}
	keys, secrets := newCaller(t, "Held")
	if _, err := access.Update("accounts-key", access.Change{Key: keys[0].ID, Accounts: []string{"codex/spare@example.com"}}); err != nil {
		t.Fatal(err)
	}
	body := `{"model":"gpt-5.6-luna","stream":true,"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"Generate a concise title. User prompt:\nfix the login bug"}]}],` +
		`"text":{"format":{"type":"json_schema","name":"codex_output_schema","strict":true,"schema":{"type":"object","properties":{"title":{"type":"string"}},"required":["title"],"additionalProperties":false}}}}`
	r := httptest.NewRequest("POST", CodexPath+"/responses", strings.NewReader(body))
	r.Header.Set("Authorization", "Bearer "+secrets[0])
	r.Header.Set("session_id", "title-thread")
	r.Header.Set("x-codex-turn-metadata", `{"thread_source":"thread_title","parent_thread_id":"main-chat"}`)
	w := httptest.NewRecorder()
	New().Handler().ServeHTTP(w, r)
	if w.Code != 200 || !strings.Contains(w.Body.String(), "Fix login bug") {
		t.Fatalf("the title: %d %s", w.Code, w.Body.String())
	}
	if !slices.Equal(heads, []string{"acct-2"}) {
		t.Fatalf("the title was written by %v, the key may use acct-2 alone", heads)
	}
}

// System One asks its decision model of an account the calling key may use
// (#905): one of the provider it names and may use serves it; a provider
// the key names no account of, it asks as it always did.
func TestGatewayKeyAccountsSystemOne(t *testing.T) {
	fresh(t)
	var decided int
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		decided++
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"answers":{"intent":{"choice":"bug"}},"usage":{"input_tokens":10,"output_tokens":2}}`)
	}))
	t.Cleanup(up.Close)
	if err := provider.Save(provider.Provider{ID: "jev", Name: "Jev", Key: "kj", Decide: up.URL + "/v1", Models: []string{"jev-latest"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "sk-1", Chat: "http://127.0.0.1:1/v1", Models: []string{"m"}}); err != nil {
		t.Fatal(err)
	}
	keys, secrets := newCaller(t, "Held")
	h := New().Handler()
	ask := func(secret string) *httptest.ResponseRecorder {
		r := httptest.NewRequest("POST", "/v1/systemone", strings.NewReader(`{"model":"jev/jev-latest","state":{"message":"hi"},"questions":{"intent":{"type":"choice"}}}`))
		r.Header.Set("Authorization", "Bearer "+secret)
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		return w
	}
	// an account of the provider it names: asked
	if _, err := access.Update("accounts-key", access.Change{Key: keys[0].ID, Accounts: []string{"jev/" + provider.KeyID("kj")}}); err != nil {
		t.Fatal(err)
	}
	if w := ask(secrets[0]); w.Code != 200 || decided != 1 {
		t.Fatalf("the key's account: %d %s, decided %d", w.Code, w.Body.String(), decided)
	}
	// a provider the key names no account of: asked as it always did
	if _, err := access.Update("accounts-key", access.Change{Key: keys[0].ID, Accounts: []string{"relay/" + provider.KeyID("sk-1")}}); err != nil {
		t.Fatal(err)
	}
	if w := ask(secrets[0]); w.Code != 200 || decided != 2 {
		t.Fatalf("an unnamed provider: %d %s, decided %d", w.Code, w.Body.String(), decided)
	}
	// the provider named, its account set not to serve the decision model
	if _, err := access.Update("accounts-key", access.Change{Key: keys[0].ID, Accounts: []string{"jev/" + provider.KeyID("kj")}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetAccountModels("jev", provider.KeyID("kj"), []string{"other"}); err != nil {
		t.Fatal(err)
	}
	w := ask(secrets[0])
	if w.Code != http.StatusForbidden || !strings.Contains(w.Body.String(), "is not allowed to use the accounts behind") || decided != 2 {
		t.Fatalf("a barred account: %d %s, decided %d", w.Code, w.Body.String(), decided)
	}
}

// A gateway key held to a later key of a decision provider is asked with
// that key alone (#905): the decision was sent on the provider's first
// key whatever the key may use, spending one it may not — here the
// second is the key's, and the first is never asked.
func TestGatewayKeyAccountsSystemOneOnAKey(t *testing.T) {
	fresh(t)
	var asked []string
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		asked = append(asked, strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer "))
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"answers":{"intent":{"choice":"bug"}},"usage":{"input_tokens":10,"output_tokens":2}}`)
	}))
	t.Cleanup(up.Close)
	if err := provider.Save(provider.Provider{ID: "jev", Name: "Jev", Key: "kA", Keys: []provider.KeyAccount{{Key: "kB"}},
		Decide: up.URL + "/v1", Models: []string{"jev-latest"}}); err != nil {
		t.Fatal(err)
	}
	keys, secrets := newCaller(t, "Held")
	if _, err := access.Update("accounts-key", access.Change{Key: keys[0].ID, Accounts: []string{"jev/" + provider.KeyID("kB")}}); err != nil {
		t.Fatal(err)
	}
	r := httptest.NewRequest("POST", "/v1/systemone", strings.NewReader(`{"model":"jev/jev-latest","state":{"message":"hi"},"questions":{"intent":{"type":"choice"}}}`))
	r.Header.Set("Authorization", "Bearer "+secrets[0])
	w := httptest.NewRecorder()
	New().Handler().ServeHTTP(w, r)
	if w.Code != 200 || !strings.Contains(w.Body.String(), "bug") {
		t.Fatalf("the decision: %d %s", w.Code, w.Body.String())
	}
	if !slices.Equal(asked, []string{"kB"}) {
		t.Fatalf("asked with %v, the key may use kB alone", asked)
	}
}

// A gateway key held to a later key of a remote magpie makes videos with
// that key alone (#905): the videos API sent on the provider's first key,
// refusing a key held to a later one though it may use it.
func TestGatewayKeyAccountsVideoOnAnAllowedKey(t *testing.T) {
	fresh(t)
	var made []string
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		made = append(made, strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer "))
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"vid-1"}`)
	}))
	t.Cleanup(up.Close)
	if err := provider.Save(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset,
		Chat: up.URL + "/v1", Key: "kA", Keys: []provider.KeyAccount{{Key: "kB"}}, Models: []string{"grok-imagine-video"}}); err != nil {
		t.Fatal(err)
	}
	keys, secrets := newCaller(t, "Held")
	if _, err := access.Update("accounts-key", access.Change{Key: keys[0].ID, Accounts: []string{"office/" + provider.KeyID("kB")}}); err != nil {
		t.Fatal(err)
	}
	r := httptest.NewRequest("POST", "/v1/videos", strings.NewReader(`{"model":"office/grok-imagine-video","prompt":"a magpie"}`))
	r.Header.Set("Authorization", "Bearer "+secrets[0])
	w := httptest.NewRecorder()
	New().Handler().ServeHTTP(w, r)
	if w.Code != 200 {
		t.Fatalf("the video: %d %s", w.Code, w.Body.String())
	}
	if !slices.Equal(made, []string{"kB"}) {
		t.Fatalf("made with %v, the key may use kB alone", made)
	}
}

// A gateway key held to a later key of a keyed provider draws with that
// key alone (#905): the images API's keyed path refused a key held to a
// later one though it may use it.
func TestGatewayKeyAccountsDrawOnAnAllowedKey(t *testing.T) {
	fresh(t)
	var drew []string
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		drew = append(drew, strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer "))
		w.Header().Set("Content-Type", "application/json")
		b64 := base64.StdEncoding.EncodeToString(pngBytes)
		if strings.HasSuffix(r.URL.Path, "/chat/completions") {
			io.WriteString(w, `{"choices":[{"message":{"role":"assistant","content":"Here it is.","images":[{"type":"image_url","image_url":{"url":"data:image/png;base64,`+b64+`"}}]}}],"usage":{"prompt_tokens":3,"completion_tokens":50}}`)
			return
		}
		io.WriteString(w, `{"created":1,"data":[{"b64_json":"`+b64+`"}]}`)
	}))
	t.Cleanup(up.Close)
	if err := provider.Save(provider.Provider{ID: "art", Name: "Art", Chat: up.URL + "/v1", Key: "kA",
		Keys: []provider.KeyAccount{{Key: "kB"}}, Models: []string{"gpt-image-1"}}); err != nil {
		t.Fatal(err)
	}
	keys, secrets := newCaller(t, "Held")
	if _, err := access.Update("accounts-key", access.Change{Key: keys[0].ID, Accounts: []string{"art/" + provider.KeyID("kB")}}); err != nil {
		t.Fatal(err)
	}
	r := httptest.NewRequest("POST", "/v1/images/generations", strings.NewReader(`{"model":"art/gpt-image-1","prompt":"a magpie"}`))
	r.Header.Set("Authorization", "Bearer "+secrets[0])
	w := httptest.NewRecorder()
	New().Handler().ServeHTTP(w, r)
	if w.Code != 200 || !strings.Contains(w.Body.String(), "b64_json") {
		t.Fatalf("the drawing: %d %s", w.Code, w.Body.String())
	}
	if !slices.Equal(drew, []string{"kB"}) {
		t.Fatalf("drew with %v, the key may use kB alone", drew)
	}
}

// A key held to some accounts of one provider uses another's as it always
// did (#905): the list holds it to some accounts of the providers it
// names, not to the providers themselves.
func TestGatewayKeyAccountsOfANamedProvider(t *testing.T) {
	fresh(t)
	plan := &fake{t: t, ctype: "application/json", reply: `{"id":"from-plan","choices":[]}`}
	spare := &fake{t: t, ctype: "application/json", reply: `{"id":"from-spare","choices":[]}`}
	twoProviders(t, plan, spare)
	keys, secrets := newCaller(t, "Held")
	if _, err := access.Update("accounts-key", access.Change{Key: keys[0].ID, Accounts: []string{"plan/" + provider.KeyID("k")}}); err != nil {
		t.Fatal(err)
	}
	h := New().Handler()
	// plan is named: only its key may serve; spare, named nowhere, serves
	// as it always did
	r := httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"spare/m2","messages":[{"role":"user","content":"hi"}]}`))
	r.Header.Set("Authorization", "Bearer "+secrets[0])
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	if w.Code != 200 || !strings.Contains(w.Body.String(), "from-spare") {
		t.Fatalf("a provider the key names no account of: %d %s", w.Code, w.Body.String())
	}
	if spare.calls != 1 || plan.calls != 0 {
		t.Fatalf("spare %d, plan %d", spare.calls, plan.calls)
	}
}
