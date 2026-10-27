package agent

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/yetone/magpie/internal/agentplug"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// kiloGoneSrc is an agent plugin as agent-kilo is: a JSONC file, the model
// under "model" with the provider's prefix, and a provider block of its own.
const kiloGoneSrc = `
export const agent = { id: "kilo", name: "Kilo", config: "~/.config/kilo/kilo.jsonc", model: "model", prefix: "magpie/" };
export function connect({ gateway, models }) {
  return { "provider.magpie": { npm: "@ai-sdk/openai-compatible", options: { baseURL: gateway.v1, apiKey: gateway.key }, models: Object.fromEntries(models.map(m => [m.id, { name: m.name }])) } };
}
`

// kiloMine is a kilo.jsonc as its user wrote it: comments, a model of
// their own and a provider block of their own.
const kiloMine = `{
  // my Kilo
  "$schema": "https://app.kilo.ai/config.json",
  "model": "anthropic/claude-sonnet-4-5", // mine
  "provider": {
    "anthropic": { "options": { "apiKey": "sk-mine" } }
  }
}
`

// kiloWired is a home with the kilo plugin and kilo.jsonc as its user
// wrote it, put on one of magpie's models; it returns the file and the
// plugin's spec.
func kiloWired(t *testing.T) (string, string) {
	t.Helper()
	home := pluggedHome(t, kiloGoneSrc)
	path := filepath.Join(home, ".config", "kilo", "kilo.jsonc")
	os.MkdirAll(filepath.Dir(path), 0o755)
	if err := os.WriteFile(path, []byte(kiloMine), 0o644); err != nil {
		t.Fatal(err)
	}
	a, err := Find("kilo")
	if err != nil {
		t.Fatal(err)
	}
	if err := a.Field("model").Set("magpie/deepseek/pro"); err != nil {
		t.Fatal(err)
	}
	if b, _ := os.ReadFile(path); string(b) == kiloMine || !a.Wired() {
		t.Fatalf("not wired:\n%s", b)
	}
	return path, a.Plugin
}

func fileAsMine(t *testing.T, path, want string) {
	t.Helper()
	if b, err := os.ReadFile(path); err != nil || string(b) != want {
		t.Fatalf("%s is not as the user had it (%v):\n%s", path, err, b)
	}
}

// Turning an agent plugin off puts back the agent's config byte for byte
// (#3810), and on again the agent's switch puts back the model it was on.
func TestPluginOffPutsItsAgentBack(t *testing.T) {
	path, spec := kiloWired(t)
	if err := provider.SetPluginOff(context.Background(), spec, true); err != nil {
		t.Fatal(err)
	}
	fileAsMine(t, path, kiloMine)
	if _, ok := stashLoad()["plugin:kilo:"+path]; ok {
		t.Fatal("the stash outlived the wiring")
	}
	agentplug.Reload()
	if _, err := Find("kilo"); err == nil {
		t.Fatal("kilo is still an agent with its plugin off")
	}
	// `magpie kilo default` with the plugin off says so, not "unknown agent"
	if g, ok, err := RestoreGone("kilo"); !ok || err != nil || g.PluginOff != "Kilo" || len(g.Put)+len(g.Off) != 0 {
		t.Fatalf("kilo default with its plugin off: %+v, %v, %v", g, ok, err)
	}
	fileAsMine(t, path, kiloMine)
	if err := provider.SetPluginOff(context.Background(), spec, false); err != nil {
		t.Fatal(err)
	}
	agentplug.Reload()
	a, err := Find("kilo")
	if err != nil {
		t.Fatal(err)
	}
	// on again it stays as the user had it, till they switch it on
	fileAsMine(t, path, kiloMine)
	if a.Wired() {
		t.Fatal("on again, magpie wired kilo by itself")
	}
	c, err := a.ConnectHow()
	if err != nil {
		t.Fatal(err)
	}
	if c.How != "again" || a.Field("model").Get() != "magpie/deepseek/pro" {
		t.Fatalf("switched on: %+v, model %q", c, a.Field("model").Get())
	}
}

// Removing it does the same.
func TestPluginRemovePutsItsAgentBack(t *testing.T) {
	path, spec := kiloWired(t)
	if err := provider.RemovePlugin(context.Background(), spec); err != nil {
		t.Fatal(err)
	}
	fileAsMine(t, path, kiloMine)
	if len(plugin.Load().Plugins) != 0 {
		t.Fatal("not removed")
	}
}

// A plugin switched off before magpie did this (its stash from then, with
// no spec or format) and then removed: its module, read as it is, puts the
// agent back.
func TestPluginRemovedWhileOffPutsItsAgentBack(t *testing.T) {
	path, spec := kiloWired(t)
	oldStash(t, "plugin:kilo:"+path)
	if err := plugin.SetOff(spec, true); err != nil {
		t.Fatal(err)
	}
	agentplug.Reload()
	if err := provider.RemovePlugin(context.Background(), spec); err != nil {
		t.Fatal(err)
	}
	fileAsMine(t, path, kiloMine)
}

// A plugin whose module is gone from disk, still listed: what magpie
// stashed of the file puts it back.
func TestPluginWithItsModuleGonePutsItsAgentBack(t *testing.T) {
	path, spec := kiloWired(t)
	if err := os.Remove(filepath.Join(spec, "agent.js")); err != nil {
		t.Fatal(err)
	}
	agentplug.Reload()
	if err := provider.SetPluginOff(context.Background(), spec, true); err != nil {
		t.Fatal(err)
	}
	fileAsMine(t, path, kiloMine)
}

// A plugin deleted on disk and off plugins.json: `magpie kilo default`
// (RestoreGone) puts the file back from the stash alone, an old stash
// with no format by its file's name.
func TestRestoreGoneAgent(t *testing.T) {
	path, spec := kiloWired(t)
	oldStash(t, "plugin:kilo:"+path)
	os.RemoveAll(spec)
	os.WriteFile(filepath.Join(settings.Dir(), "plugins.json"), []byte(`{"plugins":[]}`), 0o644)
	agentplug.Reload()
	if _, err := Find("kilo"); err == nil {
		t.Fatal("kilo is still an agent")
	}
	g, ok, err := RestoreGone("kilo")
	if err != nil || !ok || len(g.Put) != 1 || g.Put[0] != path {
		t.Fatalf("restored %+v, %v, %v", g, ok, err)
	}
	fileAsMine(t, path, kiloMine)
	// once is all: nothing is left for it, nor for an agent never wired,
	// nor one here
	for _, id := range []string{"kilo", "nobody", "codex"} {
		if g, ok, err := RestoreGone(id); ok || err != nil {
			t.Fatalf("%s: %+v, %v, %v", id, g, ok, err)
		}
	}
}

// A file the user took off magpie by hand since is theirs: RestoreGone
// leaves it and forgets the stash.
func TestRestoreGoneLeavesAFileOffMagpie(t *testing.T) {
	path, spec := kiloWired(t)
	os.RemoveAll(spec)
	agentplug.Reload()
	mine := `{"model": "openai/gpt-5"}`
	if err := os.WriteFile(path, []byte(mine), 0o644); err != nil {
		t.Fatal(err)
	}
	g, ok, err := RestoreGone("kilo")
	if err != nil || !ok || len(g.Off) != 1 || len(g.Put) != 0 {
		t.Fatalf("restored %+v, %v, %v", g, ok, err)
	}
	fileAsMine(t, path, mine)
	if _, ok := stashLoad()["plugin:kilo:"+path]; ok {
		t.Fatal("the stash is kept")
	}
}

// oldStash takes the spec and format out of the stash under key, as a
// magpie before #3810 wrote it.
func oldStash(t *testing.T, key string) {
	t.Helper()
	var st map[string]any
	if err := json.Unmarshal([]byte(stashLoad()[key]), &st); err != nil {
		t.Fatal(err)
	}
	if st["spec"] == nil || st["format"] == nil {
		t.Fatalf("stash has no spec or format: %v", st)
	}
	delete(st, "spec")
	delete(st, "format")
	b, _ := json.Marshal(st)
	stash(map[string]string{key: string(b)})
}

// A file that can't be read is unknown: nothing is put back over it, and
// its stash is kept for when it can be.
func TestRestoreGoneKeepsAnUnreadableFile(t *testing.T) {
	if runtime.GOOS == "windows" || os.Geteuid() == 0 {
		t.Skip("a file's mode doesn't keep it from being read here")
	}
	path, spec := kiloWired(t)
	os.RemoveAll(spec)
	agentplug.Reload()
	wired, _ := os.ReadFile(path)
	os.Chmod(path, 0o000)
	t.Cleanup(func() { os.Chmod(path, 0o644) })
	if _, ok, err := RestoreGone("kilo"); !ok || err == nil {
		t.Fatalf("an unreadable file: %v, %v", ok, err)
	}
	if _, ok := stashLoad()["plugin:kilo:"+path]; !ok {
		t.Fatal("the stash is gone")
	}
	os.Chmod(path, 0o644)
	fileAsMine(t, path, string(wired))
	if g, ok, err := RestoreGone("kilo"); !ok || err != nil || len(g.Put) != 1 {
		t.Fatalf("readable again: %+v, %v, %v", g, ok, err)
	}
	fileAsMine(t, path, kiloMine)
}
