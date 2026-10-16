package gateway

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// One of Codex's own models, when OpenAI can't be reached, fails saying so
// and what to do, not with the dial error alone (#322).
func TestCodexOwnModelUnreached(t *testing.T) {
	setup(t, provider.Chat, &fake{t: t})
	up := httptest.NewServer(http.HandlerFunc(func(http.ResponseWriter, *http.Request) {}))
	up.Close()
	was := codexAPIBase
	codexAPIBase = up.URL
	t.Cleanup(func() { codexAPIBase = was })
	rec := httptest.NewRecorder()
	req := httptest.NewRequest("POST", CodexPath+"/responses", strings.NewReader(`{"model":"gpt-5.5","input":"hi","stream":true}`))
	req.Header.Set("Authorization", "Bearer sk-relay-key")
	New().Handler().ServeHTTP(rec, req)
	body := rec.Body.String()
	if rec.Code != 502 || !strings.Contains(body, "OpenAI can't be reached") || !strings.Contains(body, "gpt-5.5 is one of Codex's own models") ||
		!strings.Contains(body, "pick one of magpie's models") {
		t.Fatalf("%d %s", rec.Code, body)
	}
}
