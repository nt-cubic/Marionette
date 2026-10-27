package cli

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/agentplug"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/settings"
)

// goneAgentSrc is an agent plugin as agent-kilo is: a JSONC file, the
// model under "model", a provider block of its own.
const goneAgentSrc = `
export const agent = { id: "kilo", name: "Kilo", config: "~/.config/kilo/kilo.jsonc", model: "model", prefix: "magpie/" };
export function connect({ gateway }) {
  return { "provider.magpie": { npm: "@ai-sdk/openai-compatible", options: { baseURL: gateway.v1, apiKey: gateway.key } } };
}
`

const goneAgentMine = "{\n  // mine\n  \"model\": \"anthropic/claude-sonnet-4-5\", // mine\n  \"provider\": {\n    \"anthropic\": {}\n  }\n}\n"

// goneAgentHome has the kilo plugin, its kilo.jsonc as the user wrote it,
// and kilo on magpie's a/m through `magpie kilo a/m`; it returns the file
// and the plugin's folder.
func goneAgentHome(t *testing.T) (string, string) {
	t.Helper()
	groupsHome(t)
	home := os.Getenv("HOME")
	dir := filepath.Join(home, "plugins", "kilo")
	os.MkdirAll(dir, 0o755)
	os.WriteFile(filepath.Join(dir, "package.json"), []byte(`{"name":"agent-kilo","magpie":{"agent":"./agent.js"}}`), 0o644)
	os.WriteFile(filepath.Join(dir, "agent.js"), []byte(goneAgentSrc), 0o644)
	b, _ := json.Marshal(plugin.List{Plugins: []plugin.Entry{{Spec: dir}}})
	os.MkdirAll(settings.Dir(), 0o755)
	if err := os.WriteFile(filepath.Join(settings.Dir(), "plugins.json"), b, 0o644); err != nil {
		t.Fatal(err)
	}
	agentplug.Reload()
	t.Cleanup(agentplug.Reload)
	path := filepath.Join(home, ".config", "kilo", "kilo.jsonc")
	os.MkdirAll(filepath.Dir(path), 0o755)
	os.WriteFile(path, []byte(goneAgentMine), 0o644)
	if err := run([]string{"kilo", "a/m"}); err != nil {
		t.Fatal(err)
	}
	if b, _ := os.ReadFile(path); string(b) == goneAgentMine {
		t.Fatal("not wired")
	}
	return path, dir
}

func fileIs(t *testing.T, path, want string) {
	t.Helper()
	if b, _ := os.ReadFile(path); string(b) != want {
		t.Fatalf("%s isn't as the user had it:\n%s", path, b)
	}
}

// `magpie plugin off` and `magpie plugin rm` put the agent's config back
// byte for byte before the plugin goes (#3810).
func TestPluginOffAndRemovePutTheAgentBack(t *testing.T) {
	for _, op := range []string{"off", "rm"} {
		t.Run(op, func(t *testing.T) {
			path, dir := goneAgentHome(t)
			if err := run([]string{"plugin", op, dir}); err != nil {
				t.Fatal(err)
			}
			fileIs(t, path, goneAgentMine)
		})
	}
}

// With the plugin deleted on disk, `magpie kilo default` puts kilo.jsonc
// back from what magpie stashed, where it said "unknown agent".
func TestGoneAgentDefault(t *testing.T) {
	path, dir := goneAgentHome(t)
	os.RemoveAll(dir)
	os.WriteFile(filepath.Join(settings.Dir(), "plugins.json"), []byte(`{"plugins":[]}`), 0o644)
	agentplug.Reload()
	if err := run([]string{"kilo", "default"}); err != nil {
		t.Fatal(err)
	}
	fileIs(t, path, goneAgentMine)
	// nothing left to put back: it is an unknown agent again
	if err := run([]string{"kilo", "default"}); err == nil {
		t.Fatal("kilo is known with nothing of it left")
	}
}
