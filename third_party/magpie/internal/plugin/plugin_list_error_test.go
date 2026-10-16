package plugin

import (
	"context"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"sync/atomic"
	"testing"
	"time"
)

// A plugin that never got its vendor's list says why, for the provider
// and each account: the editor shows it over the plugin's short defaults
// (gnayiab on X: Cursor in WSL listing Auto alone, with nothing saying
// its list had failed). Its list come in, nothing is said.
func TestFallenBackListSaysWhy(t *testing.T) {
	for _, throws := range []string{"", "1", "own"} {
		t.Run("throws="+throws, func(t *testing.T) { listSaysWhy(t, throws) })
	}
}

func listSaysWhy(t *testing.T, throws string) {
	sandbox(t)
	t.Setenv("FAKE_MODELS_THROW", throws)
	// no list told before this one, in memory or on disk
	Restart()
	provMu.Lock()
	provCache = nil
	provMu.Unlock()
	var up atomic.Bool
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !up.Load() {
			c, _, _ := w.(http.Hijacker).Hijack()
			c.Close()
			return
		}
		w.Write([]byte("extra"))
	}))
	defer srv.Close()
	t.Setenv("FAKE_MODELS", srv.URL+"/models")
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("testdata/fake/index.js")
	if _, err := Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	for _, k := range []string{"k1", "k2"} {
		if _, err := APIKey(ctx, "fakeco", 0, nil, k, NewAccount); err != nil {
			t.Fatal(err)
		}
	}
	ps, err := Providers(ctx)
	if err != nil || len(ps) != 1 || len(ps[0].Accounts) != 2 {
		t.Fatalf("Providers = %+v, %v", ps, err)
	}
	if !ps[0].FellBack || ps[0].ListError == "" {
		t.Fatalf("the provider fell back saying %q (fellBack %v)", ps[0].ListError, ps[0].FellBack)
	}
	for _, a := range ps[0].Accounts {
		if !a.FellBack || a.ListError == "" {
			t.Fatalf("account %s fell back saying %q (fellBack %v)", a.Key, a.ListError, a.FellBack)
		}
	}
	// and so as magpie reads it, its accounts read again from the sign-ins
	for _, a := range Cached()[0].Accounts {
		if !a.FellBack || a.ListError == "" {
			t.Fatalf("Cached: account %s fell back saying %q (fellBack %v)", a.Key, a.ListError, a.FellBack)
		}
	}

	up.Store(true)
	if ps, err = Providers(ctx); err != nil || ps[0].FellBack || ps[0].ListError != "" {
		t.Fatalf("with the list in, Providers = %+v, %v", ps, err)
	}
	for _, a := range ps[0].Accounts {
		if a.FellBack || a.ListError != "" {
			t.Fatalf("with the list in, account %s says %q", a.Key, a.ListError)
		}
	}
}
