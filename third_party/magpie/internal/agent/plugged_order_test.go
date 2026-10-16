package agent

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// kiloSrc is the shape of magpie-community/plugins' Kilo agent (#90): one
// provider block whose models' effort variants go weakest first, the order
// Kilo lists them in.
const kiloSrc = `
export const agent = { id: "kilo", name: "Kilo Code", config: "~/.config/kilo/kilo.jsonc", format: "json", model: "model", prefix: "magpie/" };
export function connect({ gateway, models }) {
  const catalog = {};
  for (const m of models) {
    catalog[m.id] = { name: m.name ?? m.id, variants: { low: { reasoningEffort: "low" }, medium: { reasoningEffort: "medium" }, high: { reasoningEffort: "high" } } };
  }
  return { "provider.magpie": { name: "magpie", npm: "@ai-sdk/openai-compatible", options: { baseURL: gateway.v1, apiKey: gateway.key }, models: catalog } };
}
`

// inOrder fails unless each of words is in s, each after the one before.
func inOrder(t *testing.T, s string, words ...string) {
	t.Helper()
	at := -1
	for _, w := range words {
		i := strings.Index(s[at+1:], w)
		if i < 0 {
			t.Fatalf("%s missing or out of order (want %q):\n%s", w, words, s)
		}
		at += 1 + i
	}
}

// A plugin's keys are written in the order the plugin gives them, not
// sorted (#1890): Kilo showed its variants high, low, medium. The user's
// own keys, comments and order stay as they were.
func TestPluggedKeepsPluginOrder(t *testing.T) {
	home := pluggedHome(t, kiloSrc)
	path := filepath.Join(home, ".config", "kilo", "kilo.jsonc")
	os.MkdirAll(filepath.Dir(path), 0o755)
	mine := `{
  "$schema": "https://kilo.ai/config.json",
  // mine
  "theme": "tokyonight",
  "provider": {
    "mine": { "zeta": 1, "alpha": 2 }
  },
  "autoupdate": false
}
`
	os.WriteFile(path, []byte(mine), 0o644)
	a, err := Find("kilo")
	if err != nil {
		t.Fatal(err)
	}
	if err := a.Field("model").Set("magpie/deepseek/pro"); err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(path)
	s := string(b)
	inOrder(t, s, `"$schema"`, "// mine", `"theme": "tokyonight"`, `"provider"`, `"zeta": 1`, `"alpha": 2`, `"autoupdate": false`)
	inOrder(t, s, `"magpie"`, `"name": "magpie"`, `"npm"`, `"options"`, `"baseURL"`, `"apiKey"`, `"models"`)
	// magpie's catalog order, then each model's variants weakest first
	inOrder(t, s, `"deepseek/pro"`, `"low"`, `"medium"`, `"high"`, `"deepseek/flash"`, `"low"`, `"medium"`, `"high"`)

	// its own model puts the file back as it was
	if err := a.Field("model").Set("anthropic/claude"); err != nil {
		t.Fatal(err)
	}
	b, _ = os.ReadFile(path)
	if s := string(b); strings.Contains(s, `"magpie"`) || !strings.Contains(s, `"mine": { "zeta": 1, "alpha": 2 }`) || !strings.Contains(s, "// mine") {
		t.Fatalf("own:\n%s", s)
	}
}

// The same in a YAML file and an env file: an object a plugin gives keeps
// its order, and a string that reads as another type stays a string.
func TestPluggedKeepsPluginOrderYAMLAndEnv(t *testing.T) {
	home := pluggedHome(t, `
export const agent = { id: "yam", config: "~/.yam.yml", model: "model" };
export function connect({ gateway }) {
  return { "llm": { zeta: "1", alpha: true, mid: [3, "x"], key: gateway.key } };
}`, `
export const agent = { id: "envy", config: "~/.envy", format: "env", model: "MODEL" };
export function connect() {
  return { "EXTRA": { zeta: 1, alpha: 2 }, "N": 5 };
}`)
	yml := filepath.Join(home, ".yam.yml")
	os.WriteFile(yml, []byte("# mine\ntheme: dark\n"), 0o644)
	y, err := Find("yam")
	if err != nil {
		t.Fatal(err)
	}
	if err := y.Field("model").Set("magpie/deepseek/pro"); err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(yml)
	inOrder(t, string(b), "# mine", "theme: dark", "llm:", `zeta: "1"`, "alpha: true", "mid:", "- 3", "- x", "key: ")
	if strings.Contains(string(b), "{") {
		t.Fatalf("flow style:\n%s", b)
	}

	e, err := Find("envy")
	if err != nil {
		t.Fatal(err)
	}
	if err := e.Field("model").Set("magpie/deepseek/pro"); err != nil {
		t.Fatal(err)
	}
	b, _ = os.ReadFile(filepath.Join(home, ".envy"))
	if !strings.Contains(string(b), `EXTRA={"zeta":1,"alpha":2}`) && !strings.Contains(string(b), `EXTRA='{"zeta":1,"alpha":2}'`) {
		t.Fatalf("env:\n%s", b)
	}
	inOrder(t, string(b), "N=5")
}
