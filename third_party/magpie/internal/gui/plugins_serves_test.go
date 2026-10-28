package gui

import (
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/plugin"
)

// TestPluginServesItsProviders: the Plugins row's "Serves" chip names the
// providers a plugin registers, not only the built-ins moved onto it
// (#3902): a plugin magpie never had as a built-in, signing in to two
// sites of one vendor, names both; a lone provider says nothing beside the
// row's title; a moved built-in is named once, by the page.
func TestPluginServesItsProviders(t *testing.T) {
	two := []plugin.Provider{{ID: "letsmagic", Name: "超级麦吉"}, {ID: "magicrew", Name: "MagiCrew"}}
	for _, c := range []struct {
		name  string
		moved []string
		ps    []plugin.Provider
		want  []string
	}{
		{"two sites, nothing moved", nil, two, []string{"超级麦吉", "MagiCrew"}},
		{"one provider", nil, two[:1], nil},
		{"none", nil, nil, nil},
		{"same name twice", nil, []plugin.Provider{{ID: "a", Name: "Magic"}, {ID: "b", Name: "magic "}}, nil},
		{"a nameless one by its id", nil, []plugin.Provider{{ID: "a", Name: "A"}, {ID: "b"}}, []string{"A", "b"}},
		{"both moved", []string{"qoder", "qoder-cn"}, []plugin.Provider{{ID: "qoder", Name: "Qoder"}, {ID: "qoder-cn", Name: "Qoder CN"}}, nil},
		{"one moved, one not", []string{"qoder"}, []plugin.Provider{{ID: "qoder", Name: "Qoder"}, {ID: "qoder-cn", Name: "Qoder CN"}}, []string{"Qoder CN"}},
	} {
		if got := servesBeside(c.moved, c.ps); !slices.Equal(got, c.want) {
			t.Errorf("%s: %q, want %q", c.name, got, c.want)
		}
	}
}
