package settings

import (
	"os"
	"path/filepath"
)

// ranBefore are files a magpie (or the dial before it, copied over by
// Migrate) leaves in Dir once it has run: its settings, a provider, an
// account, the version it last ran as, the library, a profile, a plugin,
// an agent's stashed config, a request served, the id stats send.
var ranBefore = []string{
	"settings.json", "providers.json", "logins.json", "last-version",
	"library.json", "profiles.json", "plugins.json", "stash.json",
	"usage.jsonl", "migrations.json", "install-id",
}

// RanBefore is whether a magpie has run here before: one of ranBefore's
// files is in Dir. It is asked at start, after Migrate and before anything
// writes into Dir. A file that can't be looked at counts as there:
// unknown is never new.
func RanBefore() bool {
	for _, name := range ranBefore {
		if _, err := os.Lstat(filepath.Join(Dir(), name)); !os.IsNotExist(err) {
			return true
		}
	}
	return false
}
