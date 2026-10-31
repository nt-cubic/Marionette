package agent

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/provider"
	"gopkg.in/yaml.v3"
)

// omp asks each model on the API its provider serves it on, as Pi does: a
// Claude over Chat lost its thinking's signatures between tool turns. A
// Messages-only provider's models go on anthropic-messages at the gateway,
// a Responses-only one's on openai-responses, and a Chat provider's stay on
// the provider's openai-completions, keyless. A group goes on
// openai-responses when all its members are best there (#1594), else stays.
func TestOmpModelsAskedOnTheirNativeAPI(t *testing.T) {
	home := syncHome(t)
	was := ompVersion
	ompVersion = func() string { return "16.3.5" }
	t.Cleanup(func() { ompVersion = was })
	for _, p := range []provider.Provider{
		{ID: "resp", Name: "Resp", Key: "k", Responses: "http://127.0.0.1:1/v1", Models: []string{"grok-5", "gpt-5.5"}},
		{ID: "openai", Name: "OpenAI", Key: "k", Chat: "https://api.openai.com/v1", Responses: "https://api.openai.com/v1", Models: []string{"gpt-5.5"}},
		{ID: "anth", Name: "Anth", Key: "k", Anthropic: "http://127.0.0.1:1", Models: []string{"claude-sonnet-5", "claude-sonnet-4-5", "kimi-k3"}},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	if err := provider.SaveGroup(provider.Group{ID: "mixed", Name: "Mixed", Members: []string{"resp/grok-5", "relay/glm-4.6"}}); err != nil {
		t.Fatal(err)
	}
	var live []catalog.Model
	for _, id := range []string{"claude-sonnet-5", "claude-sonnet-4-5", "kimi-k3"} {
		live = append(live, catalog.Model{ID: id, Efforts: []string{"low", "high", "max"}})
	}
	if err := catalog.SaveLive("anth", "http://127.0.0.1:1", live); err != nil {
		t.Fatal(err)
	}
	if err := omp(home).Field("model").Set("magpie/anth/claude-sonnet-5"); err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(filepath.Join(home, ".omp", "agent", "models.yml"))
	if err != nil {
		t.Fatal(err)
	}
	var file map[string]any
	if err := yaml.Unmarshal(b, &file); err != nil {
		t.Fatal(err)
	}
	pm := file["providers"].(map[string]any)["magpie"].(map[string]any)
	if pm["api"] != "openai-completions" || pm["baseUrl"] != gatewayV1() || pm["auth"] != "none" {
		t.Fatalf("provider: %v", pm)
	}
	entries := map[string]map[string]any{}
	for _, raw := range pm["models"].([]any) {
		m := raw.(map[string]any)
		entries[m["id"].(string)] = m
	}
	for id, want := range map[string]any{
		"resp/grok-5":            "openai-responses",
		"relay/glm-4.6":          nil,
		"anth/claude-sonnet-5":   "anthropic-messages",
		"anth/claude-sonnet-4-5": "anthropic-messages",
		"anth/kimi-k3":           "anthropic-messages",
		"auto/gpt-5.5":           "openai-responses",
		"group/mixed":            nil,
	} {
		e, ok := entries[id]
		if !ok {
			t.Errorf("%s not listed: %s", id, b)
		} else if e["api"] != want {
			t.Errorf("%s: api %v, want %v", id, e["api"], want)
		}
	}
	// omp posts to baseUrl + /v1/messages; the Claude that refuses a
	// thinking budget thinks adaptively, the others on a budget alone
	for id, mode := range map[string]string{"anth/claude-sonnet-5": "anthropic-adaptive", "anth/claude-sonnet-4-5": "budget", "anth/kimi-k3": "budget"} {
		e := entries[id]
		if e["baseUrl"] != gateway.URL() {
			t.Errorf("%s: baseUrl %v, want %s", id, e["baseUrl"], gateway.URL())
		}
		// and, for omp 16.3.5, its efforts stop at xhigh, the gateway's way
		// to max: it turns the whole models.yml away over a max
		th, _ := e["thinking"].(map[string]any)
		if th["mode"] != mode || fmt.Sprint(th["efforts"]) != "[low high xhigh]" {
			t.Errorf("%s: thinking %v, want mode %s, efforts [low high xhigh]", id, e["thinking"], mode)
		}
	}
	for _, id := range []string{"resp/grok-5", "relay/glm-4.6"} {
		if _, ok := entries[id]["baseUrl"]; ok {
			t.Errorf("%s keeps the provider's baseUrl: %v", id, entries[id])
		}
	}
}

// #4277: omp asks a Gemini model on Gemini's own API, as OmO does: its
// google-generative-ai posts to baseUrl/models/<id>:streamGenerateContent,
// so baseUrl is the gateway's /v1beta. Under auth: none omp hands the
// Google client its "N/A" no-auth key, which it sends as x-goog-api-key and
// loopback takes, so the provider stays keyless. Gemini 3 thinks by
// thinkingLevel (google-level), 2.5 by a budget, which refuses a level. A
// model the provider serves on Chat, and a group, stay on Chat.
func TestOmpGeminiModelsAskedOnGeminisAPI(t *testing.T) {
	home := syncHome(t)
	was := ompVersion
	ompVersion = func() string { return "18.14.0" }
	t.Cleanup(func() { ompVersion = was })
	for _, p := range []provider.Provider{
		{ID: "gem", Name: "Gemini", Key: "k", Gemini: "http://127.0.0.1:1/v1beta", Models: []string{"gemini-3-flash", "gemini-2.5-pro"}},
		{ID: "hybrid", Name: "Hybrid", Key: "k", Chat: "http://127.0.0.1:1/v1", Gemini: "http://127.0.0.1:1/v1beta", Models: []string{"gemini-3-flash", "gemini-2.5-pro"}},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	if err := provider.SetModelAPI("hybrid/gemini-3-flash", "gemini"); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "geminis", Name: "Geminis", Members: []string{"gem/gemini-3-flash"}}); err != nil {
		t.Fatal(err)
	}
	var live []catalog.Model
	for _, id := range []string{"gemini-3-flash", "gemini-2.5-pro"} {
		live = append(live, catalog.Model{ID: id, Efforts: []string{"minimal", "low", "medium", "high"}})
	}
	if err := catalog.SaveLive("gem", "http://127.0.0.1:1/v1beta", live); err != nil {
		t.Fatal(err)
	}
	if err := omp(home).Field("model").Set("magpie/gem/gemini-3-flash"); err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(filepath.Join(home, ".omp", "agent", "models.yml"))
	if err != nil {
		t.Fatal(err)
	}
	var file map[string]any
	if err := yaml.Unmarshal(b, &file); err != nil {
		t.Fatal(err)
	}
	pm := file["providers"].(map[string]any)["magpie"].(map[string]any)
	if pm["api"] != "openai-completions" || pm["baseUrl"] != gatewayV1() || pm["auth"] != "none" || pm["apiKey"] != nil {
		t.Fatalf("provider: %v", pm)
	}
	entries := map[string]map[string]any{}
	for _, raw := range pm["models"].([]any) {
		m := raw.(map[string]any)
		entries[m["id"].(string)] = m
	}
	for id, want := range map[string]any{
		"gem/gemini-3-flash":    "google-generative-ai",
		"gem/gemini-2.5-pro":    "google-generative-ai",
		"hybrid/gemini-3-flash": "google-generative-ai",
		"hybrid/gemini-2.5-pro": nil,
		"group/geminis":         nil,
	} {
		e, ok := entries[id]
		if !ok {
			t.Errorf("%s not listed: %s", id, b)
			continue
		}
		if e["api"] != want {
			t.Errorf("%s: api %v, want %v", id, e["api"], want)
		}
		if want == nil {
			if _, ok := e["baseUrl"]; ok {
				t.Errorf("%s keeps the provider's baseUrl: %v", id, e)
			}
		} else if e["baseUrl"] != gateway.URL()+"/v1beta" {
			t.Errorf("%s: baseUrl %v, want %s/v1beta", id, e["baseUrl"], gateway.URL())
		}
	}
	for id, mode := range map[string]string{"gem/gemini-3-flash": "google-level", "gem/gemini-2.5-pro": "budget"} {
		th, _ := entries[id]["thinking"].(map[string]any)
		if th["mode"] != mode || fmt.Sprint(th["efforts"]) != "[minimal low medium high]" {
			t.Errorf("%s: thinking %v, want mode %s", id, entries[id]["thinking"], mode)
		}
	}
}

// The thinking mode follows the Gemini generation whatever the provider
// or plugin prefix, and a model of a later or unnamed generation thinks by
// level, as Gemini 3 does.
func TestOmpGeminiMode(t *testing.T) {
	for id, want := range map[string]string{
		"gem/gemini-3-flash":            "google-level",
		"google-plugin/gemini-3.1-pro":  "google-level",
		"codeassist/Gemini-2.5-Pro":     "budget",
		"gem/gemini-2.0-flash":          "budget",
		"gem/gemini-1.5-pro":            "budget",
		"gemini-4-ultra":                "google-level",
		"relay/vendor/gemini-2.5-flash": "budget",
	} {
		if got := ompGeminiMode(id); got != want {
			t.Errorf("%s: %s, want %s", id, got, want)
		}
	}
}
