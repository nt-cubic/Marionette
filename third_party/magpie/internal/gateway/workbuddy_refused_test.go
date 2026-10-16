package gateway

import (
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// WorkBuddy AI refusing Codex's chat "from an unapproved channel" (its
// system prompt, #182) reaches Codex, and the routing page, with what to do
// about it; the refusal still hands a group's request to the next member.
func TestWorkBuddyRefusalSaysWhatToDo(t *testing.T) {
	fresh(t)
	home := os.Getenv("HOME")
	t.Setenv("USERPROFILE", home)
	base := filepath.Join(home, ".local", "share")
	switch runtime.GOOS {
	case "darwin":
		base = filepath.Join(home, "Library", "Application Support")
	case "windows":
		base = filepath.Join(home, "AppData", "Local")
	}
	future := time.Now().Add(24 * time.Hour).UnixMilli()
	info := `{"account":{"uid":"u1","nickname":"me"},"auth":{"accessToken":"a","refreshToken":"r","expiresAt":` +
		strconv.FormatInt(future, 10) + `,"refreshExpiresAt":` + strconv.FormatInt(future, 10) + `,"domain":"www.codebuddy.ai"}}`
	auth := filepath.Join(base, "CodeBuddyExtension", "Data", "Public", "auth", "workbuddy-desktop-ai.info")
	if err := os.MkdirAll(filepath.Dir(auth), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(auth, []byte(info), 0o600); err != nil {
		t.Fatal(err)
	}

	const refusal = `{"code":11101,"msg":"Illegal API invocation from an unapproved channel"}`
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(400)
		w.Write([]byte(refusal))
	}))
	defer up.Close()
	var p provider.Provider
	for _, x := range provider.All() {
		if x.ID == provider.WorkBuddyAIID {
			p = x
		}
	}
	if p.Account == nil {
		t.Fatal("WorkBuddy AI isn't signed in")
	}
	p.Chat = up.URL + "/v2" // never the real endpoint

	s := New()
	w := httptest.NewRecorder()
	r := httptest.NewRequest(http.MethodPost, "/v1/responses", nil)
	body := []byte(`{"model":"deepseek-v4.1-flash","instructions":"You are Codex, based on GPT-5.","input":"hi","stream":true}`)
	var u Usage
	status, msg := s.translate(w, r, p, provider.Responses, provider.Chat, "deepseek-v4.1-flash", body, &u)
	if status != 400 || !strings.Contains(msg, "unapproved channel") || !strings.HasSuffix(msg, provider.WBRefusedHint) {
		t.Fatalf("routing page gets %d %q", status, msg)
	}
	if got := w.Body.String(); !strings.Contains(got, provider.WBRefusedHint) {
		t.Fatalf("Codex gets %s", got)
	}
	// what the group's fallback reads: the refusal, hint and all, is still
	// one the next member may serve
	if !retryable(400, w.Body.Bytes()) {
		t.Fatal("the refusal no longer falls over to the next member")
	}
}
