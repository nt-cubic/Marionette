package agent

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/provider"
)

// droidUser is a settings.json as droid leaves it for someone with a BYOK
// model of their own on a Factory model, and settings magpie has nothing
// to do with.
const droidUser = `{
  "customModels": [
    {
      "model": "kimi-k2-instruct",
      "id": "custom:Kimi-K2-[Groq]-0",
      "index": 0,
      "baseUrl": "https://api.groq.com/openai/v1",
      "apiKey": "${GROQ_API_KEY}",
      "displayName": "Kimi K2 [Groq]",
      "provider": "generic-chat-completion-api"
    }
  ],
  "sessionDefaultSettings": {
    "model": "claude-opus-4-7",
    "autonomyLevel": "low"
  },
  "diffMode": "unified"
}
`

func droidHome(t *testing.T) (home, path string) {
	t.Helper()
	home = syncHome(t)
	t.Setenv("FACTORY_HOME_OVERRIDE", "")
	for _, p := range []provider.Provider{
		{ID: "deepseek", Name: "DeepSeek", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"pro", "flash"}},
		{ID: "resp", Name: "Resp", Key: "k", Responses: "http://127.0.0.1:1/v1", Models: []string{"grok-5"}},
		{ID: "anth", Name: "Anth", Key: "k", Anthropic: "http://127.0.0.1:1", Models: []string{"claude-sonnet-5"}},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	path = filepath.Join(home, ".factory", "settings.json")
	os.MkdirAll(filepath.Dir(path), 0o755)
	return home, path
}

// droidFile is settings.json read back: its custom models by id, and the
// model sessions start on.
func droidFile(t *testing.T, path string) (map[string]map[string]any, []string, map[string]any) {
	t.Helper()
	var f map[string]any
	if err := json.Unmarshal([]byte(readFile(path)), &f); err != nil {
		t.Fatalf("%v\n%s", err, readFile(path))
	}
	byID := map[string]map[string]any{}
	var order []string
	ms, _ := f["customModels"].([]any)
	for _, m := range ms {
		e := m.(map[string]any)
		id, _ := e["id"].(string)
		byID[id] = e
		order = append(order, id)
	}
	return byID, order, f
}

func TestDroid(t *testing.T) {
	home, path := droidHome(t)
	os.WriteFile(path, []byte(droidUser), 0o644)
	a, err := Find("droid")
	if err != nil || a.Path != path {
		t.Fatalf("find: %v %v", a, err)
	}
	if !a.Detected() {
		t.Fatal("not detected with ~/.factory")
	}
	if got := a.Values()["model"]; got != "claude-opus-4-7" {
		t.Fatalf("model: %q", got)
	}
	// the picker: the user's own BYOK model by the id droid knows it by,
	// the Factory model they're on, then magpie's catalog
	opts := a.Field("model").Options(a.Values())
	if opts[0].Value != "claude-opus-4-7" || opts[1].Value != "custom:Kimi-K2-[Groq]-0" || opts[1].Label != "Kimi K2 [Groq]" || opts[1].Group != "Droid" {
		t.Fatalf("own options: %+v", opts[:2])
	}
	var via bool
	for _, o := range opts {
		via = via || o.Value == "magpie/deepseek/pro"
	}
	if !via {
		t.Fatalf("no magpie/deepseek/pro in %+v", opts)
	}

	if err := a.Apply("model", "magpie/deepseek/pro"); err != nil {
		t.Fatal(err)
	}
	ms, order, f := droidFile(t, path)
	if order[0] != "custom:Kimi-K2-[Groq]-0" {
		t.Fatalf("the user's model moved: %v", order)
	}
	// the user's entry is as it was, byte for byte
	if !strings.Contains(readFile(path), droidUser[strings.Index(droidUser, "    {"):strings.Index(droidUser, "    }\n")+5]) {
		t.Fatalf("user's entry rewritten:\n%s", readFile(path))
	}
	sd := f["sessionDefaultSettings"].(map[string]any)
	if sd["model"] != "custom:magpie/deepseek/pro" || sd["autonomyLevel"] != "low" || f["diffMode"] != "unified" {
		t.Fatalf("settings: %v", f)
	}
	if got := a.Values()["model"]; got != "magpie/deepseek/pro" {
		t.Fatalf("model read back: %q", got)
	}
	// each on the API its provider speaks, so the gateway relays it as it is
	for id, want := range map[string][2]string{
		"custom:magpie/deepseek/pro":         {"generic-chat-completion-api", gatewayV1()},
		"custom:magpie/deepseek/flash":       {"generic-chat-completion-api", gatewayV1()},
		"custom:magpie/resp/grok-5":          {"openai", gatewayV1()},
		"custom:magpie/anth/claude-sonnet-5": {"anthropic", gateway.URL()},
	} {
		e := ms[id]
		if e == nil {
			t.Errorf("%s missing: %v", id, order)
			continue
		}
		if e["provider"] != want[0] || e["baseUrl"] != want[1] || e["apiKey"] != gateway.Token {
			t.Errorf("%s: %v", id, e)
		}
		if _, ok := e["noImageSupport"]; !ok {
			t.Errorf("%s: noImageSupport left to droid's default: %v", id, e)
		}
		if m := strings.TrimPrefix(id, "custom:magpie/"); e["model"] != m {
			t.Errorf("%s: model %v", id, e["model"])
		}
	}
	if n := ms["custom:magpie/deepseek/pro"]["displayName"]; n != "pro · DeepSeek" {
		t.Errorf("displayName %v", n)
	}
	if d := a.Check(); d != "" {
		t.Fatalf("check: %s", d)
	}
	// another of magpie's: the stash keeps the user's own
	if err := a.Apply("model", "magpie/resp/grok-5"); err != nil {
		t.Fatal(err)
	}
	// a provider added since joins the picker on Sync
	provider.Save(provider.Provider{ID: "later", Name: "Later", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"m1"}})
	if err := a.Sync(); err != nil {
		t.Fatal(err)
	}
	if ms, _, _ = droidFile(t, path); ms["custom:magpie/later/m1"] == nil {
		t.Fatalf("sync: %v", ms)
	}
	// something else rewrote magpie's entry
	cur := readFile(path)
	at := strings.Index(cur, `"custom:magpie/resp/grok-5"`)
	edited := cur[:at] + strings.Replace(cur[at:], gatewayV1(), "http://127.0.0.1:9/v1", 1)
	os.WriteFile(path, []byte(edited), 0o644)
	if d := a.Check(); !strings.Contains(d, "baseUrl") {
		t.Fatalf("check after a moved baseUrl: %q", d)
	}
	a.Sync()
	if d := a.Check(); d != "" {
		t.Fatalf("check after sync: %s", d)
	}

	// reset: the user's model back, magpie's entries gone, the rest as it was
	if err := a.Apply("model", ""); err != nil {
		t.Fatal(err)
	}
	if got := readFile(path); got != droidUser {
		t.Fatalf("not restored:\n%s", got)
	}
	_ = home
}

// Picking one of the user's own models while on magpie's takes magpie out.
func TestDroidOwnModel(t *testing.T) {
	_, path := droidHome(t)
	os.WriteFile(path, []byte(droidUser), 0o644)
	a := droid(os.Getenv("HOME"))
	if err := a.Apply("model", "magpie/anth/claude-sonnet-5"); err != nil {
		t.Fatal(err)
	}
	if err := a.Apply("model", "custom:Kimi-K2-[Groq]-0"); err != nil {
		t.Fatal(err)
	}
	ms, order, f := droidFile(t, path)
	if len(order) != 1 || ms["custom:Kimi-K2-[Groq]-0"] == nil {
		t.Fatalf("models: %v", order)
	}
	if got := f["sessionDefaultSettings"].(map[string]any)["model"]; got != "custom:Kimi-K2-[Groq]-0" {
		t.Fatalf("model: %v", got)
	}
	// and a reset after that leaves the user's choice, not the stash
	if err := a.Apply("model", "magpie/deepseek/pro"); err != nil {
		t.Fatal(err)
	}
	a.Apply("model", "")
	if got := a.Values()["model"]; got != "custom:Kimi-K2-[Groq]-0" {
		t.Fatalf("reset: %q", got)
	}
}

// No settings.json at all, or one with droid's documented top-level
// "model": magpie adds only its own, and a reset leaves the file as it was.
func TestDroidFresh(t *testing.T) {
	home, path := droidHome(t)
	a := droid(home)
	if a.Values()["model"] != "" {
		t.Fatal("model without a file")
	}
	if err := a.Apply("model", "magpie/deepseek/flash"); err != nil {
		t.Fatal(err)
	}
	if _, order, _ := droidFile(t, path); len(order) != 5 {
		t.Fatalf("models: %v", order)
	}
	if err := a.Apply("model", ""); err != nil {
		t.Fatal(err)
	}
	if got := strings.TrimSpace(readFile(path)); got != "{}" {
		t.Fatalf("left: %s", got)
	}

	top := "{\n  \"model\": \"gpt-5.5\",\n  \"customModels\": []\n}\n"
	os.WriteFile(path, []byte(top), 0o644)
	if a.Values()["model"] != "gpt-5.5" {
		t.Fatalf("top-level model: %q", a.Values()["model"])
	}
	if err := a.Apply("model", "magpie/deepseek/pro"); err != nil {
		t.Fatal(err)
	}
	// droid reads sessionDefaultSettings.model before the top-level one
	if _, _, f := droidFile(t, path); f["model"] != "gpt-5.5" || f["sessionDefaultSettings"].(map[string]any)["model"] != "custom:magpie/deepseek/pro" {
		t.Fatalf("settings: %v", f)
	}
	if err := a.Apply("model", ""); err != nil {
		t.Fatal(err)
	}
	if got := readFile(path); got != "{\n  \"model\": \"gpt-5.5\"\n}\n" || a.Values()["model"] != "gpt-5.5" {
		t.Fatalf("reset:\n%s", got)
	}
}

// $FACTORY_HOME_OVERRIDE stands for the home droid keeps .factory in; the
// legacy config.json's custom_models are offered, never written.
func TestDroidOverrideAndLegacy(t *testing.T) {
	home, _ := droidHome(t)
	other := filepath.Join(home, "elsewhere")
	t.Setenv("FACTORY_HOME_OVERRIDE", other)
	a := droid(home)
	if a.Path != filepath.Join(other, ".factory", "settings.json") {
		t.Fatalf("path: %s", a.Path)
	}
	legacy := filepath.Join(other, ".factory", "config.json")
	os.MkdirAll(filepath.Dir(legacy), 0o755)
	cfg := `{"custom_models":[{"model_display_name":"Local Qwen","model":"qwen3:4b","base_url":"http://localhost:11434/v1","api_key":"x","provider":"generic-chat-completion-api"},{"model":"qwen3:4b","model_display_name":"Local Qwen","base_url":"http://localhost:11435/v1","api_key":"x","provider":"generic-chat-completion-api"}]}`
	os.WriteFile(legacy, []byte(cfg), 0o644)
	var got []string
	for _, o := range a.Field("model").Options(a.Values()) {
		if o.Group == "Droid" {
			got = append(got, o.Value)
		}
	}
	if strings.Join(got, ",") != "custom:Local-Qwen-0,custom:Local-Qwen-1" {
		t.Fatalf("legacy options: %v", got)
	}
	if err := a.Apply("model", "magpie/deepseek/pro"); err != nil {
		t.Fatal(err)
	}
	if readFile(legacy) != cfg {
		t.Fatal("config.json written")
	}
	if _, err := os.Stat(a.Path); err != nil {
		t.Fatal(err)
	}
}
