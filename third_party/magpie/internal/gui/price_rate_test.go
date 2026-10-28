package gui

import (
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// A save's priceRate (#819): left out keeps the rate, null clears it, a
// number in range is taken and anything else is refused.
func TestPriceRateOf(t *testing.T) {
	for _, c := range []struct {
		raw  string
		r    float64
		keep bool
		bad  bool
	}{
		{"", 0, true, false},
		{"null", 0, false, false},
		{"0.8", 0.8, false, false},
		{"1.5", 1.5, false, false},
		{"0.1234", 0, false, true},
		{"-1", 0, false, true},
		{"1001", 0, false, true},
		{`"x"`, 0, false, true},
	} {
		r, keep, err := priceRateOf(json.RawMessage(c.raw))
		if (err != nil) != c.bad || r != c.r || keep != c.keep {
			t.Errorf("%q: %v %v %v", c.raw, r, keep, err)
		}
	}
}

// The Codex editor's Save, as the GUI posts it, keeps its Price rate and
// /api/providers gives it back (PaulT on Discord, v0.1.1805: Codex's 价格倍率
// couldn't be saved). A signed-in account's save kept its picks and
// limits only, so the rate was written away and the field came back empty.
func TestAccountSaveKeepsPriceRate(t *testing.T) {
	home := sandboxHome(t)
	codexHome := filepath.Join(home, ".codex")
	t.Setenv("CODEX_HOME", codexHome)
	jwt := func(c any) string {
		b, _ := json.Marshal(c)
		return "e30." + base64.RawURLEncoding.EncodeToString(b) + ".sig"
	}
	auth, _ := json.Marshal(map[string]any{"auth_mode": "chatgpt", "tokens": map[string]any{
		"id_token":     jwt(map[string]any{"email": "a@x.y", "https://api.openai.com/auth": map[string]any{"chatgpt_plan_type": "plus", "chatgpt_account_id": "acc"}}),
		"access_token": jwt(map[string]any{"exp": time.Now().Add(24 * time.Hour).Unix()}), "refresh_token": "r", "account_id": "acc"}})
	if err := os.MkdirAll(codexHome, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(codexHome, "auth.json"), auth, 0o600); err != nil {
		t.Fatal(err)
	}
	mux := http.NewServeMux()
	providerRoutes(mux, nil)
	rate := func() any {
		t.Helper()
		w := httptest.NewRecorder()
		mux.ServeHTTP(w, httptest.NewRequest("GET", "/api/providers", nil))
		var got struct {
			Providers []struct {
				ID        string  `json:"id"`
				PriceRate float64 `json:"priceRate"`
			} `json:"providers"`
		}
		if err := json.Unmarshal(w.Body.Bytes(), &got); err != nil {
			t.Fatalf("%v: %s", err, w.Body)
		}
		for _, p := range got.Providers {
			if p.ID == "codex" {
				return p.PriceRate
			}
		}
		return "no codex"
	}
	for _, c := range []struct {
		send string
		want float64
	}{{"0.8", 0.8}, {"1.5", 1.5}, {"null", 0}} {
		w := httptest.NewRecorder()
		body := `{"id":"codex","models":[],"noModels":false,"unlisted":false,"fallback":[],"contexts":{},"outputs":{},"compacts":{},"proxy":"","accountProxies":{},"maxConcurrency":null,"queueLimit":0,"queueWait":0,"maxRPM":0,"priceRate":` + c.send + `,"modelPrefs":{},"pinUpstream":false}`
		mux.ServeHTTP(w, httptest.NewRequest("POST", "/api/provider/save", strings.NewReader(body)))
		if w.Code != 200 {
			t.Fatalf("%s: %d %s", c.send, w.Code, w.Body)
		}
		if got := rate(); got != c.want {
			t.Fatalf("saved %s, read back %v", c.send, got)
		}
	}
}
