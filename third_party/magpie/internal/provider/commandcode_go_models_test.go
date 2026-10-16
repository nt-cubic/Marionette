package provider

import (
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"testing"
)

// A Command Code account on the Go plan lists every model Command Code
// lets Go use, not ten (01huadalang on Discord: "command 账号登录只有十个
// 模型，正确的应该是几十个"): its list is the Provider API's, asked without
// the key, less the models Go is refused; the CLI's own table stands in
// before it is fetched. testdata/commandcode_models.json is that list as
// api.commandcode.ai answered it (2026-10-01, 86 models).
func TestCommandCodeGoModels(t *testing.T) {
	home := signIn(t)
	writeFile(t, filepath.Join(home, ".commandcode", "auth.json"), map[string]any{"apiKey": "go-key", "userName": "gouser"})
	list, err := os.ReadFile(filepath.Join("testdata", "commandcode_models.json"))
	if err != nil {
		t.Fatal(err)
	}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/provider/v1/models":
			if r.Header.Get("Authorization") != "" || r.Header.Get("x-api-key") != "" {
				t.Error("the list was asked with the Go key")
			}
			_, _ = w.Write(list)
		case "/alpha/billing/subscriptions":
			_, _ = w.Write([]byte(`{"success":true,"data":{"planId":"individual-go-monthly","status":"active"}}`))
		default:
			w.WriteHeader(404)
		}
	}))
	defer srv.Close()
	oldAPI := cmdAPI
	cmdAPI = srv.URL
	defer func() { cmdAPI = oldAPI }()
	cmdPlansSeen.Lock()
	cmdPlansSeen.m = map[string]cmdSeen{}
	cmdPlansSeen.Unlock()

	p, found := find(All(), CommandCodePlanID)
	if !found {
		t.Fatal("no Command Code account")
	}
	// before a fetch, once the plan is read: the CLI's table, far more
	// than ten
	if _, _, ok := CommandCodeGenerate(context.Background(), p); !ok {
		t.Fatal("not on Go")
	}
	p, _ = find(All(), CommandCodePlanID)
	before := p.Available()
	if len(before) != len(cmdGoModels) || len(before) < 40 {
		t.Fatalf("before a fetch: %d models", len(before))
	}
	for _, m := range cmdGoModels {
		if cmdGoRefused[m.ID] {
			t.Errorf("the table offers %s, which Go is refused", m.ID)
		}
	}

	ms, err := p.Fetch(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	var got []string
	for _, m := range ms {
		got = append(got, m.ID)
	}
	if len(got) != 56 {
		t.Fatalf("Go lists %d models: %v", len(got), got)
	}
	for _, id := range []string{"gpt-6-luna", "deepseek/deepseek-v4.1-flash", "moonshotai/Kimi-K2.6", "zai-org/GLM-5.2", "Qwen/Qwen3.7-Max", "stepfun/Step-5-Preview", "tencent/hy4-preview", "google/gemini-3.6-flash", "xai/grok-4.5"} {
		if !slices.Contains(got, id) {
			t.Errorf("%s missing from %v", id, got)
		}
	}
	for _, id := range []string{"claude-opus-5-5", "claude-sonnet-5-5", "gpt-6-sol", "gpt-5.6-sol", "xai/grok-4.7", "google/gemini-3.8-flash", "xiaomi/mimo-v2.6-pro-ultraspeed"} {
		if slices.Contains(got, id) {
			t.Errorf("%s, which Go is refused, is listed", id)
		}
	}
	// kept, and shown with the table's reasoning levels and pictures
	p, _ = find(All(), CommandCodePlanID)
	after := p.Available()
	if len(after) != 56 {
		t.Fatalf("after a fetch: %d models", len(after))
	}
	if e := p.Efforts("moonshotai/Kimi-K3"); !slices.Equal(e, []string{"low", "high", "max"}) {
		t.Errorf("Kimi K3 efforts: %v", e)
	}
	for _, m := range after {
		if m.ID == "moonshotai/Kimi-K3" && !m.Images {
			t.Error("Kimi K3 takes no pictures")
		}
	}
}
