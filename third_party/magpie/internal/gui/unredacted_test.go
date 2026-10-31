package gui

import (
	"context"
	"encoding/json"
	"maps"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/netproxy"
	"github.com/yetone/magpie/internal/provider"
)

// lc on Discord: the editor's "Send requests unmasked" is saved on a
// preset added afresh (Ollama) and on one saved again, and /api/providers
// gives it back for the editor to open with.
func TestProviderSaveKeepsUnredacted(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	mux := http.NewServeMux()
	providerRoutes(mux, nil)
	do := func(method, path, body string) string {
		t.Helper()
		w := httptest.NewRecorder()
		mux.ServeHTTP(w, httptest.NewRequest(method, path, strings.NewReader(body)))
		if w.Code != 200 {
			t.Fatalf("%s: %d %s", path, w.Code, w.Body)
		}
		return w.Body.String()
	}
	do("POST", "/api/provider/save", `{"id":"ollama","name":"Ollama","preset":"ollama","unredacted":true,"new":true}`)
	if p, err := provider.Find("ollama"); err != nil || !p.Unredacted || !p.SkipsRedaction() {
		t.Fatalf("a new Ollama: %+v %v", p, err)
	}
	if !strings.Contains(do("GET", "/api/providers", ""), `"unredacted":true`) {
		t.Fatal("the editor isn't told it is unmasked")
	}
	do("POST", "/api/provider/save", `{"id":"box","name":"Box","chat":"http://192.168.1.20:8000/v1","key":"k","unredacted":true,"new":true}`)
	do("POST", "/api/provider/save", `{"id":"box","from":"box","name":"Box","chat":"http://192.168.1.20:8000/v1","unredacted":false}`)
	if p, err := provider.Find("box"); err != nil || p.Unredacted {
		t.Fatalf("saved masked again: %+v %v", p, err)
	}
}

// AcMilan on Discord: the editor can't resolve a name of the user's own
// DNS, so it asks /api/local-hosts whether to caution under "Send
// requests unmasked": a name resolving only to private addresses is local, one
// resolving to a public address or not at all isn't, and an IP or
// localhost is judged by what it is.
func TestLocalHostsAPI(t *testing.T) {
	t.Cleanup(netproxy.SetLookup(func(_ context.Context, host string) ([]net.IPAddr, error) {
		switch host {
		case "llm.corp.lan":
			return []net.IPAddr{{IP: net.ParseIP("192.168.1.20")}}, nil
		case "api.vendor.example":
			return []net.IPAddr{{IP: net.ParseIP("203.0.113.7")}}, nil
		}
		return nil, &net.DNSError{Err: "no such host", Name: host, IsNotFound: true}
	}))
	mux := http.NewServeMux()
	localHostsRoutes(mux)
	w := httptest.NewRecorder()
	mux.ServeHTTP(w, httptest.NewRequest("POST", "/api/local-hosts", strings.NewReader(`{"hosts":["LLM.corp.lan","api.vendor.example","gone.lan","localhost","[::1]","8.8.8.8","10.1.2.3"]}`)))
	var got struct{ Local map[string]bool }
	if err := json.Unmarshal(w.Body.Bytes(), &got); err != nil || w.Code != 200 {
		t.Fatalf("%d %s %v", w.Code, w.Body, err)
	}
	want := map[string]bool{"llm.corp.lan": true, "api.vendor.example": false, "gone.lan": false, "localhost": true, "[::1]": true, "8.8.8.8": false, "10.1.2.3": true}
	if !maps.Equal(got.Local, want) {
		t.Fatalf("%v, want %v", got.Local, want)
	}
}
