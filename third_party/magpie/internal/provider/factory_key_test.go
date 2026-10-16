package provider

import (
	"context"
	"io"
	"net/http"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/testenv"
)

// #506: a Factory account added by its API key, as droid takes
// FACTORY_API_KEY: whoami asked with the key says whose it is; the account's
// requests carry the key as the bearer with droid's headers and no active
// org (droid's comes from a sign-in), and nothing is renewed with WorkOS. A
// key whoami refuses isn't added; the same key again is "exists".
func TestFactoryAPIKey(t *testing.T) {
	signIn(t)
	const good, bad = "fk-good_0123456789abcdef", "fk-bad_0123456789abcdef"
	var mu sync.Mutex
	var sent []http.Header
	factorySite(t, func(w http.ResponseWriter, r *http.Request) {
		auth := r.Header.Get("Authorization")
		switch {
		case strings.HasPrefix(r.URL.Path, "/wos/"):
			t.Errorf("WorkOS asked for a key account: %s", r.URL.Path)
			w.WriteHeader(400)
		case r.URL.Path == "/api/cli/whoami":
			if auth != "Bearer "+good {
				factoryJSON(w, 401, map[string]any{"detail": "Invalid API key"})
				return
			}
			factoryJSON(w, 200, map[string]any{"userId": "user_k", "orgId": "fac_K", "email": "key@example.com"})
		case r.URL.Path == "/api/llm/a/v1/messages":
			mu.Lock()
			sent = append(sent, r.Header.Clone())
			mu.Unlock()
			// a key's org isn't one droid sends: the header refused, as
			// Factory refuses an org the caller didn't pick
			if auth != "Bearer "+good || r.Header.Get("X-Factory-Org-Id") != "" {
				w.WriteHeader(403)
				io.WriteString(w, `{"detail":"Forbidden"}`)
				return
			}
			io.WriteString(w, `{"type":"message"}`)
		default:
			t.Errorf("unexpected %s %s", r.Method, r.URL.Path)
			w.WriteHeader(404)
		}
	})

	res, err := ImportFactoryKeys(context.Background(), []string{"FACTORY_API_KEY=" + good + "\n" + bad + "\n" + good})
	if err != nil {
		t.Fatal(err)
	}
	if len(res) != 2 || res[0].User != "key@example.com" || res[0].Status != "added" || res[1].Status != "failed" || strings.Contains(res[1].User, "0123456789") {
		t.Fatalf("imported %+v", res)
	}
	if c := factoryKept(t, "key@example.com"); !c.Key || c.Access != good || c.Refresh != "" || c.Active != "" {
		t.Fatalf("kept %+v", c)
	}
	p, ok := find(Accounts(), "factory")
	if !ok || p.Account.User != "key@example.com" {
		t.Fatalf("account: %v %+v", ok, p.Account)
	}
	if code, b := factorySend(t, p); code != 200 {
		t.Fatalf("a key account's request: %d %s", code, b)
	}
	mu.Lock()
	h := sent[len(sent)-1]
	mu.Unlock()
	if h.Get("Authorization") != "Bearer "+good || h.Get("X-Factory-Client") != "cli" || h.Get("User-Agent") != "factory-cli/"+factoryVersion || h.Get("X-Factory-Org-Id") != "" {
		t.Fatalf("sent %v", h)
	}
	// still no org after the request, and no whoami asked for one
	if c := factoryKept(t, "key@example.com"); c.Active != "" {
		t.Fatalf("a key account got an org: %q", c.Active)
	}

	res, err = ImportFactoryKeys(context.Background(), []string{good})
	if err != nil || len(res) != 1 || res[0].Status != "exists" {
		t.Fatalf("again: %+v %v", res, err)
	}
	if _, err := ImportFactoryKeys(context.Background(), []string{"sk-not-factory"}); err == nil {
		t.Fatal("no key in it, and no error")
	}
}

// A moved Factory's keys are the plugin's accounts: an imported key is kept
// by the plugin as its own "Factory API key" sign-in keeps one, an api
// account with whoami's metadata, and not as a built-in account the moved
// Factory never reads. The same key again is "exists".
func TestFactoryAPIKeyMoved(t *testing.T) {
	signIn(t)
	bun := testenv.Bun(t)
	t.Setenv("MAGPIE_BUN", bun)
	t.Setenv("FAKE_ID", "factory")
	t.Cleanup(plugin.Settle)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if err := setMigration("factory", func(m *Migration) { m.State, m.Package = MovePlugin, abs }); err != nil {
		t.Fatal(err)
	}
	const good = "fk-good_0123456789abcdef"
	factorySite(t, func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/cli/whoami" || r.Header.Get("Authorization") != "Bearer "+good {
			t.Errorf("unexpected %s %s", r.Method, r.URL.Path)
			w.WriteHeader(404)
			return
		}
		factoryJSON(w, 200, map[string]any{"userId": "user_k", "orgId": "fac_K", "email": "key@example.com"})
	})
	res, err := ImportFactoryKeys(ctx, []string{good})
	if err != nil || len(res) != 1 || res[0].Status != "added" || res[0].User != "key@example.com" {
		t.Fatalf("imported %+v, %v", res, err)
	}
	var kept []map[string]any
	for _, a := range plugin.Auths("factory") {
		kept = append(kept, a)
	}
	if len(kept) != 1 || kept[0]["type"] != "api" || kept[0]["key"] != good {
		t.Fatalf("the plugin keeps %v", kept)
	}
	if md, _ := kept[0]["metadata"].(map[string]any); md["email"] != "key@example.com" || md["userId"] != "user_k" {
		t.Fatalf("the key's metadata %v", kept[0]["metadata"])
	}
	if _, ok := factoryLookup("key@example.com"); ok {
		t.Fatal("the key was kept by the built-in too")
	}
	if res, err = ImportFactoryKeys(ctx, []string{good}); err != nil || len(res) != 1 || res[0].Status != "exists" {
		t.Fatalf("again: %+v, %v", res, err)
	}
	if n := len(plugin.Auths("factory")); n != 1 {
		t.Fatalf("again: the plugin keeps %d accounts", n)
	}
}
