package agent

import (
	"encoding/base64"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// #1865: a ChatGPT account is offered one GPT Image for Aside's image
// model, as its backend draws the same for any name; Aside's
// magpie/codex/gpt-image-2.5, picked when magpie offered it, is still
// taken as that one, not turned away as no image model.
func TestAsideCodexOneGPTImage(t *testing.T) {
	asideHome(t)
	claims := func(m map[string]any) string {
		b, _ := json.Marshal(m)
		return "h." + base64.RawURLEncoding.EncodeToString(b) + ".s"
	}
	auth, _ := json.Marshal(map[string]any{"auth_mode": "chatgpt", "tokens": map[string]any{
		"id_token":      claims(map[string]any{"email": "me@example.com"}),
		"access_token":  claims(map[string]any{"exp": time.Now().Add(time.Hour).Unix()}),
		"refresh_token": "r", "account_id": "acct-1"}})
	writeFile(t, filepath.Join(os.Getenv("HOME"), ".codex", "auth.json"), string(auth))
	provider.ForgetAccounts()
	t.Cleanup(provider.ForgetAccounts)
	var codex []string
	for _, o := range asideImageOptions() {
		if strings.HasPrefix(o.Ref, "codex/") {
			codex = append(codex, o.Value)
		}
	}
	if len(codex) != 1 || codex[0] != "magpie/codex/gpt-image-2" {
		t.Fatalf("codex image options %v, want only magpie/codex/gpt-image-2", codex)
	}
	for _, v := range []string{"magpie/codex/gpt-image-2", "magpie/codex/gpt-image-2.5"} {
		if !asideImageOffered(v) {
			t.Errorf("%s not offered", v)
		}
	}
	if asideImageOffered("magpie/codex/gpt-6.1-sol") {
		t.Error("a chat model offered as an image model")
	}
}
