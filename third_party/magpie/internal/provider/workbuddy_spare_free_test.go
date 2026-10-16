package provider

import (
	"context"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/catalog"
)

// magpie-community/plugins#138 (hheei), the built-in's side: an account
// at 560 / 560 credits is still served deepseek-v4.1-flash (x0.00), so its
// Credits window counts every model but the free ones. Without a fetched
// model list, every model is counted, as before.
func TestWorkBuddyCreditsSpareFreeModels(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/billing/meter/get-user-resource-summary" {
			w.WriteHeader(404)
			return
		}
		w.Write([]byte(`{"code":0,"msg":"OK","data":{"IsPaidUser":false,"Packages":[{"CycleTotalCapacity":"560","CycleUsedCapacity":"560"}]}}`))
	}))
	t.Cleanup(srv.Close)
	old := wbEndpoint
	wbEndpoint = srv.URL
	t.Cleanup(func() { wbEndpoint = old })
	a := wbAccount{Login: Login{User: "spent"}, site: wbCN, creds: wbCreds{Access: "a", UID: "u", ExpiresAt: time.Now().Add(24 * time.Hour).UnixMilli()}}
	now := time.Now()
	used := func(model string) float64 {
		q := wbQuota(context.Background(), a)
		if q.Error != "" || len(q.Windows) != 1 || q.Windows[0].Used != 100 {
			t.Fatalf("card: %+v", q)
		}
		u, _ := allowanceOf(q.Windows, now).For(model, now)
		return u
	}

	// no list fetched yet: every model counted
	if u := used("deepseek-v4.1-flash"); u != 100 {
		t.Fatalf("with no list, a free model is %v%% used, want 100", u)
	}

	if err := catalog.SaveLive(wbCN.id, srv.URL+"/v2", []catalog.Model{
		{ID: "deepseek-v4.1-flash", Free: true},
		{ID: "deepseek-v4.1-flash-sg", Rate: 0.03},
		{ID: "gpt-5.5", Rate: 1},
	}); err != nil {
		t.Fatal(err)
	}
	for model, want := range map[string]float64{"deepseek-v4.1-flash": 0, "DeepSeek-V4.1-Flash": 0, "deepseek-v4.1-flash-sg": 100, "gpt-5.5": 100} {
		if u := used(model); u != want {
			t.Errorf("%s: %v%% used, want %v", model, u, want)
		}
	}
}
