package provider

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

// A Command Code account on the Go plan is asked at /alpha/generate, with
// its key and Go's models; the plan is read from billing/subscriptions
// and kept, the one its sign-in said standing in when it can't be read.
// Every other plan stays on the Provider API.
func TestCommandCodeGoPlan(t *testing.T) {
	home := signIn(t)
	writeFile(t, filepath.Join(home, ".commandcode", "auth.json"), map[string]any{"apiKey": "go-key", "userName": "gouser"})
	var asked atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		key := strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer ")
		ok := func(v any) { _ = json.NewEncoder(w).Encode(v) }
		switch {
		case r.URL.Path == "/provider/v1/models" && r.Header.Get("Authorization") != "":
			t.Errorf("the Provider API asked for %q's models with its key", key)
			w.WriteHeader(403)
		case r.URL.Path == "/provider/v1/models":
			ok(map[string]any{"data": []map[string]any{{"id": "gpt-6-luna"}, {"id": "claude-opus-5-5"}, {"id": "moonshotai/Kimi-K3"}}})
		case r.URL.Path != "/alpha/billing/subscriptions":
			w.WriteHeader(404)
		case key == "go-key":
			asked.Add(1)
			ok(map[string]any{"success": true, "data": map[string]any{"planId": "individual-go-monthly", "status": "active"}})
		case key == "goat-key":
			ok(map[string]any{"success": true, "data": map[string]any{"planId": "individual-goat-monthly", "status": "active"}})
		case key == "pro-key":
			ok(map[string]any{"success": true, "data": map[string]any{"planId": "individual-pro-monthly", "status": "active"}})
		default:
			w.WriteHeader(503)
		}
	}))
	defer srv.Close()
	oldAPI := cmdAPI
	cmdAPI = srv.URL
	defer func() { cmdAPI = oldAPI }()
	cmdPlansSeen.Lock()
	cmdPlansSeen.m = map[string]cmdSeen{}
	cmdPlansSeen.Unlock()
	ctx := context.Background()

	// the CLI's own account, with no plan saved, is asked which it is on
	p, found := find(All(), CommandCodePlanID)
	if !found {
		t.Fatal("no Command Code account")
	}
	api, key, ok := CommandCodeGenerate(ctx, p)
	if !ok || api != srv.URL || key != "go-key" {
		t.Fatalf("go: %v %q %q", ok, api, key)
	}
	ms, err := p.Account.fetch(ctx)
	if err != nil || len(ms) != 2 || ms[0].ID != "gpt-6-luna" || ms[1].ID != "moonshotai/Kimi-K3" {
		t.Fatalf("go models: %v %+v", err, ms)
	}
	if got := p.Account.models(); len(got) != len(cmdGoModels) {
		t.Fatalf("go models before a fetch: %+v", got)
	}
	// read once, then kept
	if asked.Load() != 1 {
		t.Fatalf("billing/subscriptions asked %d times", asked.Load())
	}
	if p, _ := find(All(), CommandCodePlanID); p.Account.Plan != "Go" {
		t.Fatalf("plan: %q", p.Account.Plan)
	}
	// and asked again once it is old
	cmdPlansSeen.Lock()
	cmdPlansSeen.m["go-key"] = cmdSeen{"Go", time.Now().Add(-cmdPlanKeep)}
	cmdPlansSeen.Unlock()
	if _, _, ok := CommandCodeGenerate(ctx, p); !ok || asked.Load() != 2 {
		t.Fatalf("again: %v, asked %d", ok, asked.Load())
	}

	for _, c := range []struct {
		key, saved string
		want       bool
	}{
		{"pro-key", "", false},
		{"goat-key", "", false},  // GOAT is not Go
		{"pro-key", "Go", false}, // what billing says, over the sign-in
		{"down-key", "Go", true}, // can't be read: the sign-in's plan
		{"down-key", "Max", false},
	} {
		cmdPlansSeen.Lock()
		delete(cmdPlansSeen.m, c.key)
		cmdPlansSeen.Unlock()
		p := cmdProvider("u", c.saved, cmdAuth{APIKey: c.key})
		_, key, ok := CommandCodeGenerate(ctx, p)
		if ok != c.want || (ok && key != c.key) {
			t.Errorf("%s saved %q: %v %q", c.key, c.saved, ok, key)
		}
		if !ok && p.Anthropic != srv.URL+"/provider" {
			t.Errorf("%s: not on the Provider API: %+v", c.key, p)
		}
	}
	// a keyed provider is never asked there
	if _, _, ok := CommandCodeGenerate(ctx, Provider{ID: "commandcode", Key: "k"}); ok {
		t.Fatal("keyed provider")
	}
}

// Reading the usage keeps the plan it read, so the account is asked where
// its plan is without another request.
func TestCommandCodeQuotaKeepsPlan(t *testing.T) {
	signIn(t)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/alpha/billing/subscriptions":
			_ = json.NewEncoder(w).Encode(map[string]any{"data": map[string]any{"planId": "individual-go", "status": "active"}})
		case "/alpha/billing/credits":
			_ = json.NewEncoder(w).Encode(map[string]any{"credits": map[string]any{"monthlyCredits": 4}})
		default:
			w.WriteHeader(404)
		}
	}))
	defer srv.Close()
	oldAPI := cmdAPI
	cmdAPI = srv.URL
	defer func() { cmdAPI = oldAPI }()
	cmdPlansSeen.Lock()
	cmdPlansSeen.m = map[string]cmdSeen{}
	cmdPlansSeen.Unlock()
	q := cmdQuota(context.Background(), Login{User: "u"}, cmdAuth{APIKey: "q-key"})
	if q.Plan != "Go" || len(q.Windows) != 1 || q.Windows[0].Display != "$6.00 / $10.00" {
		t.Fatalf("quota: %+v", q)
	}
	if got := cmdPlanKnown(cmdAuth{APIKey: "q-key"}, ""); got != "Go" {
		t.Fatalf("kept: %q", got)
	}
}
