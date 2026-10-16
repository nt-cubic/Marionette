package agent

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// A plugin's sign-in changing reaches the model lists agents keep in
// files of their own, as a provider saved does: it only dropped what
// requests held, so Pi's models.json went on without the models of a
// plugin signed in to after it was written, and Pi's picker had none of
// them while settings.json named one (#2373, Trae CN). Signed out, they
// go from the file, by the same hook.
func TestPluginSignInReachesPiModels(t *testing.T) {
	home := syncHome(t)
	t.Setenv("MAGPIE_BUN", "")
	catalog.Changed = SyncCatalog
	t.Cleanup(func() { plugin.Told(); catalog.Changed = nil })
	if err := provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Chat: "https://api.deepseek.com/v1", Key: "k", Models: []string{"pro"}}); err != nil {
		t.Fatal(err)
	}
	spec := "@magpie-community/opencode-trae-auth"
	write := func(name string, v any) {
		t.Helper()
		b, _ := json.Marshal(v)
		if err := os.MkdirAll(settings.Dir(), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(settings.Dir(), name), b, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write("plugins.json", map[string]any{"plugins": []map[string]any{{"spec": spec}}})
	write("plugin-auth.json", map[string]map[string]any{})
	plugin.UseCached([]plugin.Provider{{ID: "trae-cn", Spec: spec, Name: "Trae CN",
		Models: []plugin.Model{{ID: "kimi-k3-max", Name: "Kimi-K3 (Max)"}}}})
	t.Cleanup(plugin.Settle)

	p := pi(home)
	if err := p.Pick("model", "magpie/deepseek/pro"); err != nil {
		t.Fatal(err)
	}
	plugin.Told()
	models := filepath.Join(home, ".pi", "agent", "models.json")
	lists := func() bool { return strings.Contains(readFile(models), `"trae-cn/kimi-k3-max"`) }
	if lists() {
		t.Fatalf("Trae CN listed before it was signed in to:\n%s", readFile(models))
	}

	// signed in: the host tells of it (plugin.OnChange) as it saves it,
	// as Restart does here without a host
	write("plugin-auth.json", map[string]map[string]any{"trae-cn": {"type": "oauth", "refresh": "r"}})
	plugin.Restart()
	plugin.Told()
	if !lists() {
		t.Fatalf("models.json hasn't Trae CN's models after its sign-in:\n%s", readFile(models))
	}

	if err := plugin.SignOut(context.Background(), "trae-cn", ""); err != nil {
		t.Fatal(err)
	}
	plugin.Told()
	if lists() {
		t.Fatalf("models.json keeps Trae CN's models after its sign-out:\n%s", readFile(models))
	}
}
