package library

import (
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"strings"
)

// An agent can sign in to a remote MCP server itself: Claude Code's /mcp
// does, and keeps the tokens in its own .credentials.json. magpie's check
// connects without them, so such a server answers it 401 and its row said
// "needs sign-in" while the agent used it fine (CyberCaius on Discord: the
// server signed in to in Claude Code in WSL). The check says which agents
// hold their own sign-in instead.
//
// Every target is read where it keeps its files, so an agent in a running
// WSL distro is read under the distro's home (wslTargetOf), as its
// settings are; a stopped distro has no target and is never started for
// this. On a Mac Claude Code keeps the tokens in the Keychain, which magpie
// never reads: there only the file, when Claude Code fell back to it.

// ownSignIns are the agents signed in to a server themselves, by
// signInKey of the server's name and URL.
type ownSignIns map[string][]string

func signInKey(name, url string) string {
	return name + "\n" + strings.TrimRight(url, "/")
}

// of are the agents signed in to s on their own; none for a command.
func (o ownSignIns) of(s *Server) []string {
	if s.Transport == "stdio" {
		return nil
	}
	return o[signInKey(s.Name, s.URL)]
}

// claudeCredentials is the part of Claude Code's .credentials.json that
// holds its MCP sign-ins: mcpOAuth, keyed "<server name>|<16 hex of the
// server's type, url and headers>", each entry naming its server and URL
// (Claude Code 2.1.310's td and its token save). An entry with neither
// token is a sign-in started and not finished, which Claude Code itself
// doesn't count as one.
type claudeCredentials struct {
	MCPOAuth map[string]struct {
		ServerName   string `json:"serverName"`
		ServerURL    string `json:"serverUrl"`
		AccessToken  string `json:"accessToken"`
		RefreshToken string `json:"refreshToken"`
	} `json:"mcpOAuth"`
}

// claudeCredentialsOf is Claude Code's .credentials.json for t, beside its
// settings.json (in its config folder, a WSL distro's for one there); ""
// for any other agent.
func claudeCredentialsOf(t *Target) string {
	if id, _, _ := strings.Cut(t.Agent.ID, "@"); id != "claude" || t.Settings == "" {
		return ""
	}
	return filepath.Join(filepath.Dir(t.Settings), ".credentials.json")
}

// readOwnSignIns reads each target's own MCP sign-ins. A file that can't
// be read or parsed says nothing of the agent: it is left out, and the
// check's own answer stands.
func readOwnSignIns() ownSignIns {
	out := ownSignIns{}
	for _, t := range Targets() {
		p := claudeCredentialsOf(t)
		if p == "" {
			continue
		}
		b, err := os.ReadFile(p)
		if err != nil {
			continue
		}
		var c claudeCredentials
		if json.Unmarshal(b, &c) != nil {
			continue
		}
		for k, e := range c.MCPOAuth {
			if e.AccessToken == "" && e.RefreshToken == "" || e.ServerURL == "" {
				continue
			}
			name := e.ServerName
			if name == "" {
				name, _, _ = strings.Cut(k, "|")
			}
			key := signInKey(name, e.ServerURL)
			if !slices.Contains(out[key], t.Agent.ID) {
				out[key] = append(out[key], t.Agent.ID)
			}
		}
	}
	for _, ids := range out {
		slices.Sort(ids)
	}
	return out
}
