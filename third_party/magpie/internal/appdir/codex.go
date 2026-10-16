package appdir

import (
	"os"
	"path/filepath"
)

// CodexHome is the folder Codex CLI keeps its files in: config.toml,
// auth.json, the model catalog magpie writes, sessions, AGENTS.md and
// skills. It is $CODEX_HOME when that is set, as codex-rs's find_codex_home
// reads it, else ~/.codex. Every place magpie reads or writes Codex's files
// goes through here (Wakkana on Discord: with CODEX_HOME set, a model picked
// in magpie went to ~/.codex/config.toml, which Codex never read).
// CODEX_HOME is read through LookupEnv, so the value a login shell lends the
// desktop app (proc.UserPath) counts, and a relative one, which Codex would
// take from whatever folder it runs in, reads as unset.
func CodexHome() string {
	home, _ := os.UserHomeDir()
	return CodexHomeIn(home)
}

// CodexHomeIn is CodexHome with home as the user's home folder.
func CodexHomeIn(home string) string {
	if d := Getenv("CODEX_HOME"); d != "" {
		return d
	}
	return filepath.Join(home, ".codex")
}
