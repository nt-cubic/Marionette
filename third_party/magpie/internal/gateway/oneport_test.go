package gateway

import (
	"net/http"
	"net/http/httptest"
	"os"
	"regexp"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/provider"
)

// Every route Handler registers is either a client route `magpie web`'s
// port hands to the gateway (ClientPath), or one named here that stays on
// the gateway's port alone: its info page at /, the /api/* routes (the
// page's own namespace there) and the Claude helper's callbacks under
// /_magpie/. A route added to Handler and to neither fails this, so the
// one-port page doesn't miss it, or take it, by accident.
func TestClientPathCoversHandler(t *testing.T) {
	src, err := os.ReadFile("gateway.go")
	if err != nil {
		t.Fatal(err)
	}
	start := strings.Index(string(src), "func (s *Server) Handler() http.Handler {")
	end := strings.Index(string(src)[start:], "\n}\n")
	body := string(src)[start : start+end]
	consts := map[string]string{
		"claudeMCPPath": claudeMCPPath, "CodexPath": CodexPath, "CodexCatalogPath": CodexCatalogPath,
		"AlphaSearchPath": AlphaSearchPath, "HeapPath": HeapPath, "CPUPath": CPUPath,
		"GoroutinesPath": GoroutinesPath, "ClaudeRunsPath": ClaudeRunsPath, "SearchMCPPath": SearchMCPPath, "VisionMCPPath": VisionMCPPath,
		"ImageMCPPath": ImageMCPPath, "ImageMCPFiles": ImageMCPFiles,
		"ToolsMCPPath": ToolsMCPPath, "provider.RemoteCardsPath": provider.RemoteCardsPath,
		"provider.RemoteRefreshPath": provider.RemoteRefreshPath,
	}
	re := regexp.MustCompile(`mux\.HandleFunc\(((?:"[^"]*"|[A-Za-z.]+)(?:\s*\+\s*(?:"[^"]*"|[A-Za-z.]+))*),`)
	n := 0
	for _, m := range re.FindAllStringSubmatch(body, -1) {
		pat := ""
		for _, part := range strings.Split(m[1], "+") {
			part = strings.TrimSpace(part)
			if strings.HasPrefix(part, `"`) {
				pat += strings.Trim(part, `"`)
			} else if v, ok := consts[part]; ok {
				pat += v
			} else {
				t.Fatalf("Handler names %s, which this test doesn't know: add it to consts", part)
			}
		}
		if i := strings.IndexByte(pat, ' '); i >= 0 {
			pat = pat[i+1:]
		}
		p := strings.NewReplacer("{$}", "", "{id...}", "x", "{call...}", "x", "{id}", "x", "{token}", "x", "{name}", "x").Replace(pat)
		n++
		stays := p == "/" || strings.HasPrefix(p, "/api/") || strings.HasPrefix(p, "/_magpie/")
		if ClientPath(p) == stays {
			t.Errorf("%s (%s): ClientPath %v", pat, p, ClientPath(p))
		}
	}
	if n < 60 {
		t.Fatalf("found %d routes in Handler: the pattern no longer reads it", n)
	}
}

// A path that isn't clean stays behind the page's key: the gateway's mux
// would redirect it elsewhere, and "/v1/../api/state" is the page's API.
func TestClientPathOnlyClean(t *testing.T) {
	for _, p := range []string{"", "/", "/api/state", "/api/v1/models", "/v1/../api/state", "/v1//models", "/v1/./models", "/v1x", "/v1betax/models", "/boot.js", "/_magpie/sign-in", "/mcpx", "/v1/models/../../api/settings"} {
		if ClientPath(p) {
			t.Errorf("%q is a client path", p)
		}
	}
	for _, p := range []string{"/v1", "/v1/", "/v1/models", "/v1/chat/completions", "/v1beta/models/gemini:generateContent", "/mcp/magpie/tools", "/backend-api/codex/responses", "/messages"} {
		if !ClientPath(p) {
			t.Errorf("%q is not a client path", p)
		}
	}
}

// A request the page's port marked (FromPagePort, or the header toGateway
// passes on) is never local, and lanGuard takes the header off before any
// handler, so nothing passes it on upstream. One without the mark, from
// loopback, is an agent here as before.
func TestPagePortMarkIsNeverLocal(t *testing.T) {
	fresh(t)
	t.Setenv("MAGPIE_TRUST_PROXY", "1")
	var sawLocal, reached, sawHeader bool
	h := lanGuard(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		sawLocal, reached, sawHeader = local(r), true, r.Header.Get(PagePortHeader) != ""
	}))
	r := httptest.NewRequest("GET", "/v1/models", nil)
	r.RemoteAddr = "127.0.0.1:50001"
	h.ServeHTTP(httptest.NewRecorder(), r)
	if !sawLocal {
		t.Fatal("an agent here, unmarked: not local")
	}
	if local(FromPagePort(r)) {
		t.Fatal("marked by the page's port: local")
	}
	for _, m := range []*http.Request{FromPagePort(r), takePagePort(FromPagePort(r))} {
		if !viaPagePort(m) {
			t.Fatal("the mark was lost")
		}
	}
	if takePagePort(FromPagePort(r)).Header.Get(PagePortHeader) != "" {
		t.Fatal("takePagePort left the header on")
	}
	reached = false
	w := httptest.NewRecorder()
	h.ServeHTTP(w, FromPagePort(r))
	if w.Code != http.StatusForbidden || reached {
		t.Fatalf("marked, not shared: %d, handler reached %v", w.Code, reached)
	}
	// shared, with a key: answered, the mark kept in the context alone
	t.Setenv("MAGPIE_ADDR", "0.0.0.0:3499")
	secret, err := access.Update("add-key", access.Change{Name: "Laptop"})
	if err != nil {
		t.Fatal(err)
	}
	k := FromPagePort(r)
	k.Header.Set("Authorization", "Bearer "+secret)
	w = httptest.NewRecorder()
	h.ServeHTTP(w, k)
	if !reached || sawLocal || sawHeader {
		t.Fatalf("marked, with a key: %d, handler reached %v, saw local %v, the header %v", w.Code, reached, sawLocal, sawHeader)
	}
}
