package gateway

import (
	"strings"
	"testing"
	"unicode/utf8"

	"github.com/yetone/magpie/internal/plugin"
)

// A tool's name in Chinese is cut to 64 bytes where a character begins,
// and so is one told apart by a number (#2416).
func TestToolNameCutKeepsCharactersWhole(t *testing.T) {
	long := strings.Repeat("工", 30)
	pts := []plugin.PluginTools{{Spec: "demo", Tools: []plugin.Tool{{Name: long}, {Name: long + "二"}}}}
	tools := namedTools(pts)
	if len(tools) != 2 {
		t.Fatalf("tools %d", len(tools))
	}
	for _, tl := range tools {
		if !utf8.ValidString(tl.name) || len(tl.name) > 64 {
			t.Fatalf("name %q (%d bytes)", tl.name, len(tl.name))
		}
	}
	if tools[0].name == tools[1].name || !strings.HasSuffix(tools[1].name, "_2") {
		t.Fatalf("names %q %q", tools[0].name, tools[1].name)
	}
}
