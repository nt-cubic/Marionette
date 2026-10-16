package cli

import (
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/settings"
)

// magpie model compact off runs Codex's and Claude Code's conversations to
// the model's whole window, on compacts them at the working window again
// (Max on Discord found no way to it but the app's Settings); anything else
// is refused.
func TestModelCompactCmd(t *testing.T) {
	groupsHome(t)
	catalog.Changed = nil // no agent's files are rewritten here
	if err := modelCmd([]string{"compact"}); err != nil {
		t.Fatal(err)
	}
	if err := modelCmd([]string{"compact", "off"}); err != nil || !settings.Load().FullContext {
		t.Fatal(err, settings.Load().FullContext)
	}
	if err := modelCmd([]string{"compact"}); err != nil {
		t.Fatal(err)
	}
	if err := modelCmd([]string{"compact", "on"}); err != nil || settings.Load().FullContext {
		t.Fatal(err, settings.Load().FullContext)
	}
	if err := modelCmd([]string{"full-context", "full"}); err != nil || !settings.Load().FullContext {
		t.Fatal(err, settings.Load().FullContext)
	}
	if err := modelCmd([]string{"compact", "maybe"}); err == nil {
		t.Fatal("took maybe")
	}
}
