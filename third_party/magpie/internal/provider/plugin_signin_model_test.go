package provider

import (
	"context"
	"io"
	"net/http"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/testenv"
)

// A plugin answering 401 for a model the account may not use, and saying
// nothing of the sign-in, leaves the account's sign-in as it is: the
// gateway fails the request over to every account, and each was marked
// as needing a new sign-in for a credential that was fine (plugins#468).
// A 401 about the sign-in still marks it, one naming a model beside its
// token words included, and the body reaches the gateway whole.
func TestPluginModel401KeepsSignIn(t *testing.T) {
	bun := testenv.Bun(t)
	claudeHome(t)
	t.Setenv("MAGPIE_BUN", bun)
	t.Cleanup(plugin.Settle)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	key, err := plugin.Import(ctx, "fakeco", map[string]any{"type": "oauth", "refresh": "r-ok", "access": "a", "expires": 9e15, "accountId": "ok@fake"})
	if err != nil {
		t.Fatal(err)
	}
	saveLogins(t, savedLogin{Agent: "plugin:fakeco", User: "ok@fake", Home: key, On: true})
	if _, err := plugin.Providers(ctx); err != nil {
		t.Fatal(err)
	}
	pp := mustPlugin(t)
	lapsed := func() bool {
		for _, l := range pluginLogins(mustPlugin(t)) {
			return l.Lapsed != ""
		}
		t.Fatal("no account")
		return false
	}
	answer := func(status int, body string) {
		t.Helper()
		resp := &http.Response{StatusCode: status, Header: http.Header{}, Body: io.NopCloser(strings.NewReader(body))}
		notePluginSignIn(pp, key, resp)
		got, err := io.ReadAll(resp.Body)
		if err != nil || string(got) != body {
			t.Fatalf("the body after the check: %q, %v; want %q", got, err, body)
		}
	}
	for _, body := range []string{
		`{"error":{"message":"The model claude-opus-5 is not available on your plan"}}`,
		`{"error":{"code":"model_not_found","message":"unknown model"}}`,
		`{"error":{"message":"模型不支持"}}`, // 模型不支持, escaped as a vendor sends it
	} {
		answer(http.StatusUnauthorized, body)
		if lapsed() {
			t.Fatalf("a 401 about the model marked the sign-in: %s", body)
		}
	}
	// and every 401 body the community plugins' tests hold for a sign-in
	// that is gone (magpie-community/plugins at 95fa761): each one marks
	for _, body := range []string{
		`{"error":{"message":"invalid token"}}`,
		`{"error":{"message":"model gpt-x: invalid api key"}}`,
		``,
		`{"code":"unauthenticated","message":"invalid api key"}`,        // devin
		`{"code":"unauthenticated","message":"bad key"}`,                // devin
		`{"code":"upstream","message":"bad key","upstream_status":401}`, // zed
		`unauthorized`,                                           // zed, raccoon
		`{"error":{"message":"nope"}}`,                           // cline
		`{"success":false,"error":"unauthorized"}`,               // cline
		`{"message":"expired"}`,                                  // cline, search1api
		`the sign-in lapsed — sign in again`,                     // cline
		`{"code":200003,"message":"authorization_verify_error"}`, // raccoon
		`{"code":401,"data":null,"msg":"登录票据已失效"}`,               // joycode
		`private-marker`,                                         // joycode
		`{"code":1001,"msg":"Authorization Token非法"}`,            // zcode
		`{"error":{"message":"token expired or incorrect"}}`,     // zcode
		`{"code":401}`,                                           // mimo
		`{"error":{"message":"stale"}}`,                          // mimo
		`{"error":{"message":"Invalid API Key"}}`,                // mimo
		`no`, // cursor
		`Unauthorized — sign in to Cursor again in magpie`, // cursor
		`{"code":"unauthenticated","message":"Error","details":[{"debug":{"error":"ERROR_NOT_LOGGED_IN","details":{"title":"Authentication error","detail":"If you are logged in, try logging out and back in."}}}]}`, // cursor
	} {
		answer(http.StatusOK, "{}")
		answer(http.StatusUnauthorized, body)
		if !lapsed() {
			t.Fatalf("a 401 about the sign-in didn't mark it: %q", body)
		}
	}
}
