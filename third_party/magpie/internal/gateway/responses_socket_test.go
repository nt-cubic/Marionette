package gateway

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A client opening Responses over a WebSocket (Pi's Responses WS
// extension, #1005) is told 426 with the reason, not the 404 of an unknown
// path, and nothing goes upstream; a plain GET is told to POST.
func TestResponsesWebSocketUpgradeRequired(t *testing.T) {
	setup(t, provider.Responses, &fake{t: t})
	for _, path := range []string{"/v1/responses", "/responses"} {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest("GET", path, nil)
		req.Header.Set("Upgrade", "websocket")
		req.Header.Set("Connection", "Upgrade")
		req.Header.Set("Sec-WebSocket-Version", "13")
		req.Header.Set("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
		New().Handler().ServeHTTP(rec, req)
		if rec.Code != http.StatusUpgradeRequired || !strings.Contains(rec.Body.String(), "POST /v1/responses") {
			t.Errorf("%s upgrade: %d %s", path, rec.Code, rec.Body)
		}
		rec = httptest.NewRecorder()
		New().Handler().ServeHTTP(rec, httptest.NewRequest("GET", path, nil))
		if rec.Code != http.StatusMethodNotAllowed || rec.Header().Get("Allow") != "POST" {
			t.Errorf("%s GET: %d %q", path, rec.Code, rec.Header().Get("Allow"))
		}
	}
}
