package library

import (
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/agentplug"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/settings"
)

// plugAgents switches on a plugin for each agent module in srcs, in the
// sandbox h.
func plugAgents(t *testing.T, h string, srcs ...string) {
	t.Helper()
	var l plugin.List
	for i, src := range srcs {
		dir := filepath.Join(h, "plugins", "p"+string(rune('a'+i)))
		write(t, filepath.Join(dir, "package.json"), `{"name":"p`+string(rune('a'+i))+`","magpie":{"agent":"./agent.js"}}`)
		write(t, filepath.Join(dir, "agent.js"), src)
		l.Plugins = append(l.Plugins, plugin.Entry{Spec: dir})
	}
	b, _ := json.Marshal(l)
	write(t, filepath.Join(settings.Dir(), "plugins.json"), string(b))
	agentplug.Reload()
	t.Cleanup(agentplug.Reload)
}

const toySrc = `
export const agent = {
  id: "toy", name: "Toy", dir: "~/.toy", config: "~/.toy/config.json", model: "model",
  library: { instructions: "~/.toy/AGENTS.md", mcp: "$HOME/.toy/mcp.json", mcpFormat: "claude", skills: "~/.toy/skills" },
};
`

// An agent a plugin adds is given the Library's instructions, MCP servers
// and skills where its agent.library says, and has only magpie's taken
// away again, the user's own kept.
func TestPluginAgentGetsTheLibrary(t *testing.T) {
	h := sandbox(t)
	plugAgents(t, h, toySrc)
	toy := filepath.Join(h, ".toy")
	write(t, filepath.Join(toy, "config.json"), "{}")
	write(t, filepath.Join(toy, "AGENTS.md"), "# my own rules\n")
	write(t, filepath.Join(toy, "mcp.json"), `{"theme":"dark","mcpServers":{"mine":{"command":"mine-mcp","note":"keep"}}}`)

	tg := targetByID("toy")
	if tg == nil {
		t.Fatalf("no target for the plugin's agent: %v", ids(Targets()))
	}
	if tg.Instructions != filepath.Join(toy, "AGENTS.md") || tg.Skills != filepath.Join(toy, "skills") || tg.MCP == nil || tg.MCP.Path != filepath.Join(toy, "mcp.json") || tg.MCP.Format != fmtClaude {
		t.Fatalf("target: %+v mcp %+v", tg, tg.MCP)
	}
	for _, kind := range []string{"instructions", "mcp", "skills"} {
		if id, err := Takes("Toy", kind); err != nil || id != "toy" {
			t.Errorf("Takes(Toy, %s) = %q, %v", kind, id, err)
		}
	}

	shared := "Use tabs."
	ok(t)(SaveInstructions(InstructionsChange{Shared: &shared, Agents: []string{"toy"}}))
	if s := read(t, filepath.Join(toy, "AGENTS.md")); !strings.HasPrefix(s, "# my own rules\n") || !strings.Contains(s, blockBegin+"\nUse tabs.\n"+blockEnd) {
		t.Errorf("AGENTS.md:\n%s", s)
	}
	srv := Server{Name: "fs", Transport: "stdio", Command: "npx", Args: []string{"-y", "@mcp/fs"}, Agents: []string{"toy"}}
	ok(t)(SaveServer("", srv))
	got, err := targetByID("toy").MCP.read()
	if err != nil || got["fs"] == nil || !got["fs"].same(&srv) || got["mine"] == nil {
		t.Errorf("mcp.json read back %+v, %v\n%s", got, err, read(t, filepath.Join(toy, "mcp.json")))
	}
	src := filepath.Join(h, "src/skills")
	skill(t, filepath.Join(src, "pdf"), "pdf", "Read PDFs")
	ok(t)(InstallSkills(src, []string{"pdf"}, []string{"toy"}))
	if _, err := os.Stat(filepath.Join(toy, "skills", "pdf", "SKILL.md")); err != nil {
		t.Error(err)
	}

	ok(t)(RemoveServer("fs"))
	ok(t)(RemoveSkill("pdf"))
	empty := ""
	ok(t)(SaveInstructions(InstructionsChange{Shared: &empty, Agents: []string{"toy"}}))
	s := read(t, filepath.Join(toy, "mcp.json"))
	if strings.Contains(s, `"fs"`) || !strings.Contains(s, `"theme"`) || !strings.Contains(s, `"note"`) {
		t.Errorf("mcp.json after:\n%s", s)
	}
	if _, err := os.Lstat(filepath.Join(toy, "skills", "pdf")); !os.IsNotExist(err) {
		t.Error("the skill is still there")
	}
	if s := read(t, filepath.Join(toy, "AGENTS.md")); !strings.HasPrefix(s, "# my own rules") || strings.Contains(s, "Use tabs.") {
		t.Errorf("AGENTS.md after:\n%s", s)
	}
}

// One that names only some places takes only those; the others are
// refused with what its plugin would need to say.
func TestPluginAgentWithoutAPlaceIsRefused(t *testing.T) {
	h := sandbox(t)
	plugAgents(t, h,
		`export const agent = { id: "half", name: "Half", dir: "~/.half", config: "~/.half/c.json", model: "m", library: { skills: "~/.half/skills" } };`,
		`export const agent = { id: "bare", name: "Bare", dir: "~/.bare", config: "~/.bare/c.json", model: "m" };`)
	write(t, filepath.Join(h, ".half", "c.json"), "{}")
	write(t, filepath.Join(h, ".bare", "c.json"), "{}")

	if id, err := Takes("half", "skills"); err != nil || id != "half" {
		t.Errorf("Takes(half, skills) = %q, %v", id, err)
	}
	for _, c := range []struct{ q, kind, key string }{{"half", "mcp", "agent.library.mcp"}, {"half", "instructions", "agent.library.instructions"}, {"bare", "skills", "agent.library.skills"}} {
		if _, err := Takes(c.q, c.kind); err == nil || !strings.Contains(err.Error(), c.key) {
			t.Errorf("Takes(%s, %s) = %v, want it to name %s", c.q, c.kind, err, c.key)
		}
	}
	if got := ids(Targets()); slices.Contains(got, "bare") || !slices.Contains(got, "half") {
		t.Errorf("targets: %v", got)
	}
	ok(t)(SaveServer("", Server{Name: "fs", Transport: "stdio", Command: "npx", Agents: ids(Targets())}))
	if _, err := os.Stat(filepath.Join(h, ".half", "mcp.json")); !os.IsNotExist(err) {
		t.Error("an MCP file was written for an agent that names none")
	}
}

// An MCP file in a format magpie doesn't write, or none named, keeps the
// plugin from loading, rather than servers written in a shape its agent
// can't read.
func TestPluginAgentMCPFormat(t *testing.T) {
	h := sandbox(t)
	plugAgents(t, h,
		`export const agent = { id: "a1", config: "~/.a1.json", model: "m", library: { mcp: "~/.a1/mcp.json", mcpFormat: "nope" } };`,
		`export const agent = { id: "a2", config: "~/.a2.json", model: "m", library: { mcp: "~/.a2/mcp.json" } };`,
		`export const agent = { id: "a3", config: "~/.a3.json", model: "m", library: { mcpFormat: "claude" } };`,
		`export const agent = { id: "a4", config: "~/.a4.json", model: "m", library: { mcp: "~/.a4/mcp.json", mcpFormat: "Codex" } };`)
	list, errs := agentplug.List()
	if len(list) != 1 || list[0].Desc.ID != "a4" || list[0].Desc.Library.MCPFormat != "codex" {
		t.Errorf("loaded %d: %+v", len(list), list)
	}
	if len(errs) != 3 {
		t.Errorf("errors: %v", errs)
	}
	for _, e := range errs {
		if !strings.Contains(e, "agent.library.mcp") {
			t.Errorf("error: %s", e)
		}
	}
}

// Every format an agent plugin may name is one magpie writes.
func TestPluginMCPFormatsAreWritten(t *testing.T) {
	for _, n := range agentplug.MCPFormats {
		if _, ok := pluginMCP[n]; !ok {
			t.Errorf("%s has no mcpFormat", n)
		}
	}
	if len(pluginMCP) != len(agentplug.MCPFormats) {
		t.Errorf("pluginMCP has %d, agentplug.MCPFormats %d", len(pluginMCP), len(agentplug.MCPFormats))
	}
}
