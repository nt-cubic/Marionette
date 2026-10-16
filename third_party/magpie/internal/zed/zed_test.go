package zed

import (
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"net/url"
	"strings"
	"testing"
)

// The sign-in's key and token round trip as Zed's page and app make them:
// the page encrypts to the PKCS#1 key it is given, OAEP SHA-256, and hands
// the ciphertext back in URL-safe base64.
func TestSignInKey(t *testing.T) {
	k, pub, err := NewKey()
	if err != nil {
		t.Fatal(err)
	}
	der, err := base64.URLEncoding.DecodeString(pub)
	if err != nil {
		t.Fatalf("the public key isn't padded URL-safe base64: %v", err)
	}
	pk, err := x509.ParsePKCS1PublicKey(der)
	if err != nil {
		t.Fatalf("the public key isn't PKCS#1: %v", err)
	}
	ct, err := rsa.EncryptOAEP(sha256.New(), rand.Reader, pk, []byte("secret-token"), nil)
	if err != nil {
		t.Fatal(err)
	}
	for _, enc := range []*base64.Encoding{base64.URLEncoding, base64.RawURLEncoding} {
		got, err := Decrypt(k, enc.EncodeToString(ct))
		if err != nil || got != "secret-token" {
			t.Fatalf("Decrypt = %q, %v", got, err)
		}
	}
	// the older padding Zed's page used before OAEP
	ct, _ = rsa.EncryptPKCS1v15(rand.Reader, pk, []byte("old-token"))
	if got, err := Decrypt(k, base64.URLEncoding.EncodeToString(ct)); err != nil || got != "old-token" {
		t.Fatalf("PKCS#1 v1.5: %q, %v", got, err)
	}
	if _, err := Decrypt(k, "not base64!"); err == nil {
		t.Fatal("a mangled token was taken")
	}

	u, _ := url.Parse(SignInURL("https://zed.dev", 4242, pub, "sys"))
	q := u.Query()
	if u.Path != "/native_app_signin" || q.Get("native_app_port") != "4242" || q.Get("native_app_public_key") != pub || q.Get("system_id") != "sys" {
		t.Fatalf("sign-in URL %s", u)
	}
	if id := NewSystemID(); len(id) != 36 || id[14] != '4' {
		t.Fatalf("system id %q isn't a UUID v4", id)
	}
}

func TestParseModels(t *testing.T) {
	raw := []byte(`{"models":[
		{"provider":"anthropic","id":"claude-sonnet-4-5","display_name":"Claude Sonnet 4.5","max_token_count":200000,"max_output_tokens":64000,"supports_images":true,
		 "supported_effort_levels":[{"name":"Low","value":"low"},{"name":"High","value":"high","is_default":true}]},
		{"provider":"open_ai","id":"gpt-5","display_name":"GPT-5","max_token_count":400000},
		{"provider":"google","id":"gemini-2.5-pro","display_name":"Gemini 2.5 Pro"},
		{"provider":"x_ai","id":"grok-4","display_name":"Grok 4"},
		{"provider":"anthropic","id":"claude-old","is_disabled":true},
		{"provider":"mystery","id":"m-1"}
	],"default_model":"claude-sonnet-4-5"}`)
	ms, err := ParseModels(raw)
	if err != nil {
		t.Fatal(err)
	}
	var ids []string
	for _, m := range ms {
		ids = append(ids, m.Provider+"/"+m.ID)
	}
	if got := strings.Join(ids, " "); got != "anthropic/claude-sonnet-4-5 openai/gpt-5 google/gemini-2.5-pro xai/grok-4" {
		t.Fatalf("models %s", got)
	}
	if m := ms[0]; m.Name != "Claude Sonnet 4.5" || m.Context != 200000 || m.Output != 64000 || !m.Images || strings.Join(m.Efforts, ",") != "low,high" {
		t.Fatalf("claude %+v", m)
	}
	if ProviderOf(raw, "grok-4") != "x_ai" || ProviderOf(raw, "nope") != "" {
		t.Fatal("ProviderOf")
	}
	if _, err := ParseModels([]byte(`{"models":[]}`)); err != ErrNoModels {
		t.Fatalf("an empty list: %v", err)
	}
	for id, want := range map[string]string{"claude-x": "anthropic", "gemini-3": "google", "grok-5": "x_ai", "gpt-6": "open_ai"} {
		if GuessProvider(id) != want {
			t.Errorf("GuessProvider(%s)", id)
		}
	}
}

func TestReadLines(t *testing.T) {
	var got []string
	collect := func(l Line) error {
		switch {
		case l.Ended:
			got = append(got, "ended")
		case l.Failed != nil:
			got = append(got, "failed:"+l.Failed.Code)
		default:
			got = append(got, string(l.Event))
		}
		return nil
	}
	body := `{"status":"started"}
{"status":{"queued":{"position":2}}}
{"event":{"type":"a"}}

{"event":{"type":"b"}}
{"status":{"failed":{"code":"upstream_http_429","message":"slow down","retry_after":3}}}
{"status":"stream_ended"}
`
	if err := ReadLines(strings.NewReader(body), true, collect); err != nil {
		t.Fatal(err)
	}
	if s := strings.Join(got, "|"); s != `{"type":"a"}|{"type":"b"}|failed:upstream_http_429|ended` {
		t.Fatalf("wrapped: %s", s)
	}
	got = nil
	if err := ReadLines(strings.NewReader("{\"type\":\"a\"}\n{\"type\":\"b\"}\n"), false, collect); err != nil {
		t.Fatal(err)
	}
	if s := strings.Join(got, "|"); s != `{"type":"a"}|{"type":"b"}` {
		t.Fatalf("bare: %s", s)
	}
	for code, want := range map[string]int{"upstream_http_429": 429, "http_503": 503, "upstream_http_error": 502, "rate_limit_exceeded": 429, "overloaded": 529, "payment_required": 402} {
		if n := (&Failed{Code: code}).Status(); n != want {
			t.Errorf("%s → %d, want %d", code, n, want)
		}
	}
}
