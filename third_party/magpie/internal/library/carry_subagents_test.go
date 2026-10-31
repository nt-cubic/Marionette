package library

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// carriedSubagent is a library with a server, a skill and a subagent that
// names both, given to Claude Code and Codex, as Collect carries it.
func carriedSubagent(t *testing.T) (string, string) {
	t.Helper()
	h := sandbox(t)
	ok(t)(SaveServer("", Server{Name: "github", Transport: "stdio", Command: "npx", Agents: []string{"claude"}}))
	skill(t, filepath.Join(h, "src/tdd"), "tdd", "Tests first")
	ok(t)(InstallSkills(filepath.Join(h, "src"), []string{"tdd"}, []string{"claude"}))
	s := reviewer
	s.Agents = []string{"codex", "claude"}
	ok(t)(SaveSubagent("", s))
	if _, err := os.Stat(filepath.Join(h, ".claude/agents/reviewer.md")); err != nil {
		t.Fatalf("not written on the first computer: %v", err)
	}
	b, err := Collect()
	if err != nil {
		t.Fatal(err)
	}
	return h, mustJSON(t, b)
}

func putJSON(t *testing.T, j string) *Result {
	t.Helper()
	var in Bundle
	if err := json.Unmarshal([]byte(j), &in); err != nil {
		t.Fatal(err)
	}
	r, err := Put(&in)
	if err != nil {
		t.Fatal(err)
	}
	return r
}

// A subagent goes with the library to another computer, which writes it
// into its agents; what magpie wrote into the first computer's agents
// stays out of the bundle; taking it out on one takes it out on the other.
func TestCarrySubagents(t *testing.T) {
	_, carried := carriedSubagent(t)
	var raw struct {
		Subagents []map[string]any `json:"subagents"`
	}
	json.Unmarshal([]byte(carried), &raw)
	if len(raw.Subagents) != 1 {
		t.Fatalf("subagents carried: %s", carried)
	}
	got := raw.Subagents[0]
	if got["name"] != "reviewer" || got["prompt"] == "" || got["description"] == "" ||
		mustJSON(t, got["skills"]) != `["gone","tdd"]` || mustJSON(t, got["mcp"]) != `["github","gone"]` || mustJSON(t, got["agents"]) != `["claude","codex"]` {
		t.Errorf("the subagent carried: %v", got)
	}
	// this computer's own: which files magpie wrote, and their hashes
	if strings.Contains(carried, "applied") || strings.Contains(carried, "reviewer.md") || strings.Contains(carried, "reviewer.toml") || strings.Contains(carried, `"hash"`) {
		t.Errorf("what magpie wrote here is carried:\n%s", carried)
	}

	h2 := sandbox(t)
	r := putJSON(t, carried)
	for _, p := range r.Problems {
		if strings.HasPrefix(p.What, "subagent:") {
			t.Errorf("a problem: %+v", p)
		}
	}
	md := read(t, filepath.Join(h2, ".claude/agents/reviewer.md"))
	if !strings.Contains(md, `skills: ["tdd"]`) || !strings.Contains(md, `mcpServers: ["github"]`) || !strings.Contains(md, "Report bugs only.") {
		t.Errorf("claude's subagent on the second computer:\n%s", md)
	}
	if s := read(t, filepath.Join(h2, ".codex/agents/reviewer.toml")); !strings.Contains(s, "developer_instructions") {
		t.Errorf("codex's subagent on the second computer:\n%s", s)
	}
	// the second computer records what it wrote itself, by its own paths
	l, err := load()
	if err != nil {
		t.Fatal(err)
	}
	if fs := l.applied("claude").Subagents; len(fs) != 1 || !strings.HasPrefix(fs[0].Path, h2) {
		t.Errorf("applied here: %+v", fs)
	}
	// carried back as it came; put again, nothing is written
	back, _ := Collect()
	if mustJSON(t, back) != carried {
		t.Errorf("carried back differs:\n%s\n%s", mustJSON(t, back), carried)
	}
	if r := putJSON(t, carried); len(r.Changed) != 0 {
		t.Errorf("putting the same again wrote %v", r.Changed)
	}

	// taken out on the first computer: the bundle says none, and the
	// second computer's files go
	var m map[string]json.RawMessage
	json.Unmarshal([]byte(carried), &m)
	m["subagents"] = json.RawMessage("[]")
	none := mustJSON(t, m)
	var in Bundle
	json.Unmarshal([]byte(none), &in)
	if in.Subagents == nil || len(*in.Subagents) != 0 {
		t.Fatalf("an empty list read as none known: %s", none)
	}
	putJSON(t, none)
	for _, p := range []string{".claude/agents/reviewer.md", ".codex/agents/reviewer.toml"} {
		if _, err := os.Stat(filepath.Join(h2, p)); !os.IsNotExist(err) {
			t.Errorf("%s is still there after the subagent went: %v", p, err)
		}
	}
	if l, _ := load(); len(l.Subagents) != 0 {
		t.Errorf("subagents left: %+v", l.Subagents)
	}
}

// A bundle from a magpie that didn't carry subagents knows nothing of
// them: the ones here and their files stay.
func TestCarryOldBundleKeepsSubagents(t *testing.T) {
	h, carried := carriedSubagent(t)
	var m map[string]json.RawMessage
	json.Unmarshal([]byte(carried), &m)
	delete(m, "subagents")
	old := mustJSON(t, m)
	putJSON(t, old)
	l, _ := load()
	if len(l.Subagents) != 1 || l.Subagents[0].Name != "reviewer" {
		t.Errorf("an old bundle took the subagents: %+v", l.Subagents)
	}
	if _, err := os.Stat(filepath.Join(h, ".claude/agents/reviewer.md")); err != nil {
		t.Errorf("an old bundle took the file: %v", err)
	}
}

// A file the user edited on the receiving computer stays theirs, and so
// does a file of their own by the subagent's name.
func TestCarrySubagentKeepsUsersFiles(t *testing.T) {
	_, carried := carriedSubagent(t)
	h2 := sandbox(t)
	write(t, filepath.Join(h2, ".codex/agents/reviewer.toml"), "name = \"mine\"\n")
	putJSON(t, carried)
	if s := read(t, filepath.Join(h2, ".codex/agents/reviewer.toml")); s != "name = \"mine\"\n" {
		t.Errorf("the user's own codex file was written over:\n%s", s)
	}
	md := filepath.Join(h2, ".claude/agents/reviewer.md")
	write(t, md, read(t, md)+"\nMy own line.\n")
	edited := read(t, md)
	r := putJSON(t, strings.Replace(carried, "Report bugs only.", "Report every bug.", 1))
	if read(t, md) != edited {
		t.Errorf("the user's edit was written over:\n%s", read(t, md))
	}
	found := false
	for _, p := range r.Problems {
		found = found || p.Agent == "claude" && p.What == "subagent:reviewer"
	}
	if !found {
		t.Errorf("no problem said for the edited file: %+v", r.Problems)
	}
}

// A newer magpie's bundle: a field this one doesn't know, and an agent it
// can't give subagents to, which stays ticked to be carried on as it came.
// A name that isn't one is refused: it is a file's name in every agent.
func TestCarryNewerSubagents(t *testing.T) {
	_, carried := carriedSubagent(t)
	newer := strings.Replace(carried, `"agents":["claude","codex"]`, `"agents":["claude","codex","kimi"],"model":"fast"`, 1)
	if newer == carried {
		t.Fatal("no agents to change")
	}
	h2 := sandbox(t)
	putJSON(t, newer)
	if _, err := os.Stat(filepath.Join(h2, ".claude/agents/reviewer.md")); err != nil {
		t.Errorf("not written: %v", err)
	}
	if l, _ := load(); len(l.Subagents) != 1 || !strings.Contains(strings.Join(l.Subagents[0].Agents, ","), "kimi") {
		t.Errorf("the newer magpie's tick went: %+v", l.Subagents)
	}
	var in Bundle
	json.Unmarshal([]byte(strings.Replace(carried, `"name":"reviewer"`, `"name":"../../evil"`, 1)), &in)
	if _, err := Put(&in); err == nil {
		t.Error("a subagent named ../../evil was put")
	}
}
