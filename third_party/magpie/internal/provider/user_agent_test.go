package provider

import (
	"path/filepath"
	"testing"
)

// Only a key+URL provider passes the agent's User-Agent on (#1851): a
// signed-in account, a plugin's, and one that goes as a client of its
// vendor's own (OpenCode, Cline, Kilo, another magpie) keep the identity
// their signing gives, set or not.
func TestPassesUserAgent(t *testing.T) {
	for _, tt := range []struct {
		name string
		p    Provider
		want bool
	}{
		{"key", Provider{PassUserAgent: true, Chat: "https://api.example.com/v1"}, true},
		{"key, off", Provider{Chat: "https://api.example.com/v1"}, false},
		{"an account", Provider{PassUserAgent: true, Chat: "https://api.z.ai/api/coding/paas/v4", Account: &Account{Agent: "zcode"}}, false},
		{"a plugin's", Provider{PassUserAgent: true, Chat: "https://x.example/v1", Account: &Account{Agent: "plugin:x"}}, false},
		{"opencode", Provider{PassUserAgent: true, Chat: "https://opencode.ai/zen/v1"}, false},
		{"cline", Provider{PassUserAgent: true, Preset: "clinepass", Chat: "https://api.cline.bot/api/v1"}, false},
		{"kilo", Provider{PassUserAgent: true, Preset: "kilo", Chat: "https://api.kilo.ai/api/gateway"}, false},
		{"another magpie", Provider{PassUserAgent: true, Preset: RemoteMagpiePreset, Chat: "http://10.0.0.2:3425/v1"}, false},
	} {
		if got := tt.p.PassesUserAgent(); got != tt.want {
			t.Errorf("%s: %v, want %v", tt.name, got, tt.want)
		}
	}
}

// A copy of a provider that passes the agent's User-Agent on passes it on
// too, the form not carrying it.
func TestAddCopyPassUserAgent(t *testing.T) {
	isolate(t)
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	if err := Save(Provider{ID: "plan", Name: "Plan", Chat: "https://plan.example/v1", Key: "sk-1", PassUserAgent: true}); err != nil {
		t.Fatal(err)
	}
	id, err := AddCopy(Provider{Name: "Plan copy", Chat: "https://plan.example/v1", Models: []string{"m"}}, "plan")
	if err != nil {
		t.Fatal(err)
	}
	c, err := Find(id)
	if err != nil {
		t.Fatal(err)
	}
	if !c.PassUserAgent {
		t.Fatal("the copy goes as magpie")
	}
}
