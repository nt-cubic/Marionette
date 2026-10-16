package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"regexp"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// The kinds of personal data the user turned on or off in settings
// (redactKinds) are what the vendor doesn't see, the others as their
// defaults say; what it answers with a placeholder comes back as the value.
func TestRedactedKinds(t *testing.T) {
	const ssn, ip, bucket, user = "536-22-1847", "34.117.59.81", "acme-prod-logs", "rinakato"
	var got []byte
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		got, _ = io.ReadAll(r.Body)
		p := regexp.MustCompile(`\{\{IP_[a-z2-7]{8}\}\}`).FindString(string(got))
		chunk := func(s string) string {
			b, _ := json.Marshal(map[string]any{"id": "c1", "model": "m1", "choices": []any{map[string]any{"index": 0, "delta": map[string]any{"content": s}}}})
			return "data: " + string(b)
		}
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(chunk("ping "+p), `data: {"id":"c1","model":"m1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}}`, "data: [DONE]"))
	}))
	defer up.Close()
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := provider.Save(provider.Provider{ID: "fake", Name: "Fake", Key: "k", Models: []string{"m1"}, Chat: up.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := settings.Save(settings.Settings{Redact: true, RedactPersonal: true, RedactKinds: map[string]bool{"ip": true}}); err != nil {
		t.Fatal(err)
	}
	req := `{"model":"fake/m1","max_tokens":10,"stream":true,"messages":[{"role":"user","content":"SSN ` + ssn + `, server ` + ip + `, logs in s3://` + bucket + `/x, cwd /Users/` + user + `/src"}]}`
	code, body := post(t, "/v1/messages", req)
	if code != 200 {
		t.Fatalf("%d %s", code, body)
	}
	sent := string(got)
	if strings.Contains(sent, ssn) || strings.Contains(sent, ip) || !strings.Contains(sent, "{{SSN_") || !strings.Contains(sent, "{{IP_") {
		t.Fatalf("SSN (on by default) or IP (turned on) went out: %s", sent)
	}
	if !strings.Contains(sent, "s3://"+bucket+"/x") || !strings.Contains(sent, "/Users/"+user+"/src") {
		t.Fatalf("a bucket or a home folder (off by default) masked: %s", sent)
	}
	var text strings.Builder
	for _, e := range events(body) {
		if d, ok := e["delta"].(map[string]any); ok {
			s, _ := d["text"].(string)
			text.WriteString(s)
		}
	}
	if text.String() != "ping "+ip {
		t.Fatalf("text %q in:\n%s", text.String(), body)
	}
}
