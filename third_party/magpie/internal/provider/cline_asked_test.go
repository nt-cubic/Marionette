package provider

import (
	"testing"

	"github.com/yetone/magpie/internal/plugin"
)

// ClinePassAsked says what a ClinePass DeepSeek request asks the Cline API
// for (zRain on Discord, 10-24): the built-in ClinePass only with the
// Upstream tick, the Cline plugin's provider by default (the plugin pins
// it itself), and nothing for a model with no maker to pin to.
func TestClinePassAsked(t *testing.T) {
	const ds = "cline-pass/deepseek-v4.1-flash-1017"
	builtin := Provider{ID: "clinepass", Preset: "clinepass"}
	ticked := builtin
	ticked.PinUpstream = true
	plug := Provider{ID: "cline", Account: &Account{plugin: &plugin.Provider{ID: "cline"}}}
	other := Provider{ID: "grok", Account: &Account{plugin: &plugin.Provider{ID: "grok"}}}
	for _, c := range []struct {
		name  string
		p     Provider
		model string
		want  string
	}{
		{"built-in, tick off", builtin, ds, ""},
		{"built-in, tick on", ticked, ds, "deepseek"},
		{"built-in, tick on, GLM", ticked, "cline-pass/glm-5.3", "zai"},
		{"plugin by default", plug, ds, "deepseek"},
		{"plugin, a model it doesn't pin", plug, "cline-pass/kimi-k3", ""},
		{"another plugin", other, ds, ""},
	} {
		if got := c.p.ClinePassAsked(c.model); got != c.want {
			t.Errorf("%s: ClinePassAsked(%s) = %q; want %q", c.name, c.model, got, c.want)
		}
	}
}
