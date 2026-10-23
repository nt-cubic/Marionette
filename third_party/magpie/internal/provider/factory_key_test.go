package provider

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/testenv"
)

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
	if ls := factoryLogins(); len(ls) != 0 {
		t.Fatalf("the key was kept by the built-in too: %+v", ls)
	}
	if res, err = ImportFactoryKeys(ctx, []string{good}); err != nil || len(res) != 1 || res[0].Status != "exists" {
		t.Fatalf("again: %+v, %v", res, err)
	}
	if n := len(plugin.Auths("factory")); n != 1 {
		t.Fatalf("again: the plugin keeps %d accounts", n)
	}
}

// factorySite is a fake Factory API at the root, its EU region under /eu.
func factorySite(t *testing.T, h http.HandlerFunc) *httptest.Server {
	t.Helper()
	srv := httptest.NewServer(h)
	t.Cleanup(srv.Close)
	t.Cleanup(FactoryBaseForTest(srv.URL, srv.URL+"/eu"))
	return srv
}

func factoryJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(v)
}
