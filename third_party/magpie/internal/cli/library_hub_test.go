package cli

import (
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

// magpie library mcp hub on|off (#2679): on, the agent is given magpie's
// one MCP server in place of the library's; off, each server's own again.
// Any other word is refused, and writes nothing.
func TestLibraryMCPHub(t *testing.T) {
	h := t.TempDir()
	for k, v := range map[string]string{"HOME": h, "USERPROFILE": h, "XDG_CONFIG_HOME": filepath.Join(h, ".config"), "CLAUDE_CONFIG_DIR": "", "CODEX_HOME": "", "APPDATA": "", "LOCALAPPDATA": ""} {
		t.Setenv(k, v)
	}
	if err := os.MkdirAll(filepath.Join(h, ".claude"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(h, ".claude/settings.json"), []byte("{}"), 0o644); err != nil {
		t.Fatal(err)
	}
	names := func() []string {
		t.Helper()
		b, err := os.ReadFile(filepath.Join(h, ".claude.json"))
		if err != nil {
			t.Fatal(err)
		}
		var c struct {
			MCPServers map[string]map[string]any `json:"mcpServers"`
		}
		if err := json.Unmarshal(b, &c); err != nil {
			t.Fatal(err)
		}
		var out []string
		for k := range c.MCPServers {
			out = append(out, k)
		}
		slices.Sort(out)
		return out
	}
	if err := libraryCmd([]string{"library", "mcp", "add", "fs", "npx", "-y", "fs", "agents=claude"}); err != nil {
		t.Fatal(err)
	}
	if got := names(); !slices.Equal(got, []string{"fs"}) {
		t.Fatalf("before: %v", got)
	}
	if err := libraryCmd([]string{"library", "mcp", "hub", "on"}); err != nil {
		t.Fatal(err)
	}
	if got := names(); !slices.Equal(got, []string{"magpie"}) {
		t.Fatalf("hub on: %v", got)
	}
	if err := libraryCmd([]string{"library", "mcp", "hub", "yes"}); err == nil || !strings.Contains(err.Error(), "usage") {
		t.Fatalf("hub yes: %v", err)
	}
	if got := names(); !slices.Equal(got, []string{"magpie"}) {
		t.Fatalf("after a word it doesn't know: %v", got)
	}
	if err := libraryCmd([]string{"library", "mcp", "hub", "off"}); err != nil {
		t.Fatal(err)
	}
	if got := names(); !slices.Equal(got, []string{"fs"}) {
		t.Fatalf("hub off: %v", got)
	}
}
