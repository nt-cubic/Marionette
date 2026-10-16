package provider

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

const zcodeBuiltinJSON = `{"revision":%d,"config":{
 "providerConfigRules":{"providerRules":[
  {"providerId":"account:zai-individual-coding-plan","builtinModelIds":["GLM-5.3"]},
  {"providerId":"account:zai-start-plan","builtinModelIds":["GLM-5-Turbo"]}]},
 "modelConfigRules":{
  "modelRules":[
   {"modelMatch":".*","config":{"properties":{"contextWindow":200000},"optionSpecs":{"maxOutputTokens":{"max":32000},"reasoningLevel":{"values":["disabled","enabled"]}}}},
   {"modelMatch":".*glm-5\\.3(?:-flash)?(?:[.\\-:/\\[].*)?","config":{"properties":{"contextWindow":1000000},"optionSpecs":{"maxOutputTokens":{"max":128000},"reasoningLevel":{"values":["low","high","max"]}}}},
   {"modelMatch":".*glm-5\\.3-flash(?:[.\\-:/\\[].*)?","config":{"properties":{"inputFormat":{"supportsImage":true}}}}],
  "builtinProviderModelRules":[
   {"providerId":"account:zai-individual-coding-plan","modelId":"GLM-5.3-Flash","config":{"enabled":true}},
   {"providerId":"account:zai-individual-coding-plan","modelId":"GLM-5.1","config":{"enabled":false}},
   {"providerId":"account:zai-individual-coding-plan","modelId":"GLM-5-Turbo","config":{"enabled":true}},
   {"providerId":"account:bigmodel-individual-coding-plan","modelId":"GLM-X","config":{"enabled":true}}]}}}`

func zcodeBuiltinAt(rev int) string { return fmt.Sprintf(zcodeBuiltinJSON, rev) }

// A coding plan's models are its own in ZCode's built-in config, with what
// the rules say of each, from the later of the served and installed releases.
func TestZCodeFetchModels(t *testing.T) {
	var platform string
	srv := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/api/v1/client/configs":
			platform = r.URL.Query().Get("platform")
			json.NewEncoder(w).Encode(map[string]any{"code": 0, "data": map[string]any{"configs": map[string]string{"builtin_provider_config_json": "https://" + r.Host + "/cdn.json"}}})
		case "/cdn.json":
			w.Write([]byte(zcodeBuiltinAt(23)))
		default:
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()
	oldAPI, oldFiles, oldClient := zcodeAPI, zcodeBuiltinFiles, http.DefaultClient
	defer func() { zcodeAPI, zcodeBuiltinFiles, http.DefaultClient = oldAPI, oldFiles, oldClient }()
	zcodeAPI, http.DefaultClient = srv.URL, srv.Client()
	zcodeBuiltinFiles = func() []string { return nil }

	got := func() string {
		ms, err := zcodeFetchModels(context.Background(), ZCodeZaiBase)
		if err != nil {
			t.Fatal(err)
		}
		var s []string
		for _, m := range ms {
			b, _ := json.Marshal([]any{m.ID, m.Context, m.Output, m.Images, m.Efforts})
			s = append(s, string(b))
		}
		return strings.Join(s, " ")
	}
	want := `["GLM-5.3",1000000,128000,false,["low","high","max"]] ["GLM-5.3-Flash",1000000,128000,true,["low","high","max"]] ["GLM-5-Turbo",200000,32000,false,["none","high"]]`
	if g := got(); g != want {
		t.Fatalf("got  %s\nwant %s", g, want)
	}
	if !strings.Contains(platform, "-") {
		t.Errorf("platform = %q", platform)
	}

	// an installed ZCode's later release wins; an earlier one doesn't
	dir := t.TempDir()
	f := filepath.Join(dir, "zcode-builtin.json")
	zcodeBuiltinFiles = func() []string { return []string{f} }
	os.WriteFile(f, []byte(strings.Replace(zcodeBuiltinAt(30), `"GLM-5.3"]`, `"GLM-5.3","GLM-6"]`, 1)), 0o600)
	if g := got(); !strings.Contains(g, `"GLM-6"`) {
		t.Fatalf("later local release not used: %s", g)
	}
	os.WriteFile(f, []byte(strings.Replace(zcodeBuiltinAt(10), `"GLM-5.3"]`, `"GLM-5.3","GLM-6"]`, 1)), 0o600)
	if g := got(); g != want {
		t.Fatalf("earlier local release used: %s", g)
	}

	// bigmodel.cn's plan is its own
	ms, err := zcodeFetchModels(context.Background(), ZCodeBigModelBase)
	if err != nil || len(ms) != 1 || ms[0].ID != "GLM-X" {
		t.Fatalf("%v %v", ms, err)
	}
}

// A WorkBuddy plan's models are its product config's CLI agent's, asked as
// WorkBuddy's CLI asks.
func TestWorkBuddyFetchModels(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v3/config" || r.Header.Get("Authorization") != "Bearer tok" || !strings.HasPrefix(r.Header.Get("User-Agent"), "CLI/") {
			w.Write([]byte(`{"code":0,"data":{"agents":[{"name":"craft","models":["gpt"]}],"models":[{"id":"gpt"}]}}`))
			return
		}
		w.Write([]byte(`{"code":0,"msg":"OK","data":{
			"agents":[{"name":"craft","models":["gpt"]},{"name":"cli","models":["auto","kimi-k2.8-preview","deepseek-v4-pro","bare"]}],
			"models":[{"id":"gpt","name":"GPT"},
			 {"id":"auto","name":"Auto","maxInputTokens":256000,"maxOutputTokens":32000,"supportsImages":true,"onlyReasoning":true,"reasoning":{"effort":"high"}},
			 {"id":"kimi-k2.8-preview","name":"Kimi-K2.8-Preview","maxInputTokens":1000000,"maxOutputTokens":64000,"supportsImages":false,
			  "onlyReasoning":true,"reasoning":{"canDisableThinking":true,"defaultEffort":"high","supportedEfforts":["low","high","max"]}},
			 {"id":"deepseek-v4-pro","name":"Deepseek-V4-Pro","onlyReasoning":false,"reasoning":{"canDisableThinking":true,"supportedEfforts":["high","xhigh"]}}]}}`))
	}))
	defer srv.Close()
	old := wbEndpoint
	wbEndpoint = srv.URL
	defer func() { wbEndpoint = old }()
	sign := func(ctx context.Context, req *http.Request, body []byte) error {
		req.Header.Set("Authorization", "Bearer tok")
		req.Header.Set("User-Agent", "WorkBuddy/"+wbUAVersion)
		return nil
	}
	ms, err := wbFetchModels(context.Background(), wbCN, sign)
	if err != nil {
		t.Fatal(err)
	}
	b, _ := json.Marshal(ms)
	want := `[{"ID":"auto","Name":"Auto","Provider":"","Released":"","Efforts":null,"Temperature":null,"Price":null,"Images":true,"ImageInput":true,"Context":256000,"Output":32000},` +
		`{"ID":"kimi-k2.8-preview","Name":"Kimi-K2.8-Preview","Provider":"","Released":"","Efforts":["low","high","max"],"Temperature":null,"Price":null,"ImageInput":false,"Context":1000000,"Output":64000},` +
		`{"ID":"deepseek-v4-pro","Name":"Deepseek-V4-Pro","Provider":"","Released":"","Efforts":["none","high","xhigh"],"Temperature":null,"Price":null},` +
		`{"ID":"bare","Name":"bare","Provider":"","Released":"","Efforts":null,"Temperature":null,"Price":null}]`
	if string(b) != want {
		t.Fatalf("got  %s\nwant %s", b, want)
	}
}
