package backup

import (
	"os"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/lastgood"
	"github.com/yetone/magpie/internal/settings"
)

// A settings.json that is there but can't be read stops the backup, as an
// unreadable providers.json does: carried as the defaults, it would put them
// over the settings wherever the backup is put back (#2269).
func TestCollectStopsOnUnreadableSettings(t *testing.T) {
	home(t)
	if err := settings.Save(settings.Settings{Theme: "dark", Lang: "ja", TextSize: 125}); err != nil {
		t.Fatal(err)
	}
	whole, err := os.ReadFile(settings.Path())
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(settings.Path(), whole[:len(whole)/2], 0o600); err != nil {
		t.Fatal(err)
	}
	os.Remove(lastgood.Bak(settings.Path()))
	b, err := Collect(true, "test")
	if err == nil || !strings.Contains(err.Error(), "settings") {
		t.Fatalf("Collect read a settings.json that can't be read as settings: %v %+v", err, b.Settings)
	}

	// a missing file is no settings, not an error
	if err := os.Remove(settings.Path()); err != nil {
		t.Fatal(err)
	}
	if b, err := Collect(true, "test"); err != nil || b.Settings != nil {
		t.Fatalf("no settings.json: %v %+v", err, b.Settings)
	}
}
