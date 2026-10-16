package agent

// OmO (omo-ai, oh-my-openagent) is a fork of Pi, its engine senpi, with
// Pi's settings.json and models.json in an agent folder of its own:
// ~/.omo/agent, or OMO_CODING_AGENT_DIR's, else SENPI_CODING_AGENT_DIR's
// (bin/lib/agent-dir.js, canonicalAgentDir). magpie wires it as it does Pi:
// defaultProvider/defaultModel/defaultThinkingLevel in settings.json and
// magpie as the provider "magpie" in models.json. Its requests say
// omo/<version>.
//
// OmO reads PI_CODING_AGENT_DIR as well, after its own two; that one is
// Pi's, and a folder both share is Pi's row's to edit.

import (
	"os"
	"path/filepath"
	"strings"
)

func omo(home string) *Agent { return omoIn(here(home)) }

// omoIn is OmO at a place: this machine's home, where its variables move
// it, or a WSL distro's, whose variables magpie can't read.
func omoIn(at place) *Agent {
	a := piLike(at, "omo", "OmO", omoDir(at))
	a.Aliases = []string{"oh-my-openagent", "omo-ai"}
	return a
}

// omoDir is OmO's agent folder at a place.
func omoDir(at place) string {
	if at.spell == nil {
		for _, k := range []string{"OMO_CODING_AGENT_DIR", "SENPI_CODING_AGENT_DIR"} {
			if d := strings.TrimSpace(os.Getenv(k)); filepath.IsAbs(d) {
				return filepath.Clean(d)
			}
		}
	}
	return filepath.Join(at.home, ".omo", "agent")
}
