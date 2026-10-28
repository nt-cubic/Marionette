package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/usage"
)

// The Usage page's "…" beside a deleted provider (CyberCaius on Discord):
// the providers it can be merged into, and a POST that merges, names and
// parts it, the request log left as it was.
func TestUsageDeletedProviderRoutes(t *testing.T) {
	sandboxHome(t)
	for _, p := range []provider.Provider{
		{ID: "relay", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"gpt-9"}},
		{ID: "main", Name: "Main", Key: "k2", Chat: "http://127.0.0.1:2/v1", Models: []string{"gpt-9"}},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	usage.Append(usage.Record{Time: time.Now().Add(-time.Hour), Agent: "codex", Provider: "relay", Model: "gpt-9", Input: 10, Status: 200})
	if err := provider.Delete("relay"); err != nil {
		t.Fatal(err)
	}
	mux := http.NewServeMux()
	usageRoutes(mux, folderOnly{})
	do := func(method, path, body string, code int) string {
		t.Helper()
		w := httptest.NewRecorder()
		mux.ServeHTTP(w, httptest.NewRequest(method, path, strings.NewReader(body)))
		if w.Code != code {
			t.Fatalf("%s %s: %d %s", method, path, w.Code, w.Body)
		}
		return w.Body.String()
	}
	var into []struct{ ID, Name string }
	if err := json.Unmarshal([]byte(do("GET", "/api/usage/deleted/into", "", 200)), &into); err != nil {
		t.Fatal(err)
	}
	if len(into) != 1 || into[0].ID != "main" || into[0].Name != "Main" {
		t.Fatalf("into: %+v", into)
	}
	do("POST", "/api/usage/deleted", `{"id":"relay","name":"Old relay","into":"main"}`, 200)
	gone, _ := provider.UsageRenames().Gone("relay")
	if gone.Label != "Old relay" || gone.Into != "main" {
		t.Fatalf("after the POST: %+v", gone)
	}
	// a field not sent stays as it is
	do("POST", "/api/usage/deleted", `{"id":"relay","into":""}`, 200)
	if gone, _ := provider.UsageRenames().Gone("relay"); gone.Label != "Old relay" || gone.Into != "" {
		t.Fatalf("parted: %+v", gone)
	}
	do("POST", "/api/usage/deleted", `{"id":"relay","into":"nobody"}`, http.StatusBadRequest)
	do("POST", "/api/usage/deleted", `{"id":"main","name":"x"}`, http.StatusBadRequest)
}
