package gateway

import (
	"encoding/json"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// An image model's picture reaches a Gemini client's generateContent: the
// upstream's inlineData part comes back, and a thought image (a draft)
// stays out (#620).
func TestGeminiImageModelReturnsItsImage(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	t.Setenv("XDG_DATA_HOME", filepath.Join(home, ".local", "share"))

	f := &fake{t: t, reply: `data: {"candidates":[{"content":{"parts":[` +
		`{"thought":true,"inlineData":{"mimeType":"image/png","data":"ZHJhZnQ="}},` +
		`{"text":"Here it is."},` +
		`{"inlineData":{"mimeType":"image/png","data":"aGVsbG8="},"thoughtSignature":"c2ln"}` +
		`]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":1567}}` + "\n\n"}
	up := httptest.NewServer(f)
	t.Cleanup(up.Close)
	t.Cleanup(provider.FactoryBaseForTest(up.URL, up.URL+"/eu"))

	auth, _ := json.Marshal(map[string]any{
		"accessToken": "tok", "refreshToken": "r",
		"expiresAt": time.Now().Add(time.Hour).UnixMilli(),
		"orgId":     "org_D", "activeOrganizationId": "fac_D", "email": "d@example.com",
	})
	dir := filepath.Dir(provider.Path())
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	raw, _ := json.Marshal([]map[string]any{{
		"agent": "factory", "user": "d@example.com", "plan": "pro", "on": true, "auth": json.RawMessage(auth),
	}})
	if err := os.WriteFile(filepath.Join(dir, "logins.json"), raw, 0o600); err != nil {
		t.Fatal(err)
	}

	const ask = `{"contents":[{"role":"user","parts":[{"text":"draw a cat"}]}]}`
	check := func(kind, body string) {
		t.Helper()
		if !strings.Contains(body, `"inlineData":{"data":"aGVsbG8=","mimeType":"image/png"}`) {
			t.Errorf("%s: image dropped: %s", kind, body)
		}
		if strings.Contains(body, "ZHJhZnQ=") {
			t.Errorf("%s: thought image sent: %s", kind, body)
		}
		if i, j := strings.Index(body, "Here it is."), strings.Index(body, "aGVsbG8="); i < 0 || j < i {
			t.Errorf("%s: text and image out of order: %s", kind, body)
		}
	}

	// Factory's Gemini route decodes with Antigravity's decoder.
	code, body := post(t, "/v1beta/models/factory/gemini-3.1-pro-preview:generateContent", ask)
	if code != 200 || strings.HasPrefix(body, "data:") {
		t.Fatalf("json %d %s", code, body)
	}
	if f.path != "/api/llm/g/v1/generate" {
		t.Fatalf("upstream %s", f.path)
	}
	check("generateContent", body)
}

// Antigravity and Gemini CLI wrap each chunk in {response}; streamed to a
// Gemini client, the picture goes out as an inlineData chunk.
func TestCodeAssistStreamsImages(t *testing.T) {
	rec := httptest.NewRecorder()
	enc := &geminiEncoder{w: newSSEWriter(rec), model: "antigravity/gemini-3.1-flash-image"}
	d := &codeAssistDecoder{}
	for _, line := range []string{
		`{"response":{"candidates":[{"content":{"role":"model","parts":[{"thought":true,"inlineData":{"mimeType":"image/png","data":"ZHJhZnQ="}}]}}]}}`,
		`{"response":{"candidates":[{"content":{"role":"model","parts":[{"inlineData":{"mimeType":"image/jpeg","data":"aGk="}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":1567}}}`,
	} {
		d.decode(line, enc.event)
	}
	enc.finish()
	body := rec.Body.String()
	if !strings.Contains(body, `"inlineData":{"data":"aGk=","mimeType":"image/jpeg"}`) {
		t.Errorf("image dropped: %s", body)
	}
	if strings.Contains(body, "ZHJhZnQ=") {
		t.Errorf("thought image sent: %s", body)
	}
}
