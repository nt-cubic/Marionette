package plugin

import (
	"os"
	"path/filepath"
	"testing"
)

// Every icon a listing or an older plugin names is a picture the GUI has:
// one it lacks shows the plugin's provider with no logo at all (Trae CN's
// "trae" did).
func TestMarketIconsAreBundled(t *testing.T) {
	l, err := parseMarket(builtinMarket)
	if err != nil {
		t.Fatal(err)
	}
	names := map[string]string{}
	for _, x := range l {
		// a middleware's card may go without: it has no provider's logo
		if x.Icon == "" && x.Kind == "middleware" {
			continue
		}
		names[x.Icon] = x.Package
	}
	for k, ic := range otherIcons {
		names[ic] = k
	}
	extra := []string{"trae"} // the registry's, past the built-in list
	for _, ic := range extra {
		names[ic] = "registry"
	}
	for ic, from := range names {
		found := false
		for _, ext := range []string{".svg", ".png"} {
			if _, err := os.Stat(filepath.Join("..", "gui", "assets", "icons", ic+ext)); err == nil {
				found = true
			}
		}
		if !found {
			t.Errorf("%s: icon %q isn't in internal/gui/assets/icons", from, ic)
		}
	}
}
