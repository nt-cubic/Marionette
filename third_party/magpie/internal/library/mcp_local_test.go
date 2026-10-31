package library

import (
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

func serverIn(t *testing.T, name string) *Server {
	t.Helper()
	v, err := Read(nil)
	if err != nil {
		t.Fatal(err)
	}
	for _, s := range v.Servers {
		if s.Name == name {
			return s.Server
		}
	}
	return nil
}

func carriedNames(b Bundle) []string {
	var out []string
	for _, s := range b.MCP {
		out = append(out, s.Name)
	}
	return out
}

// A server kept to this computer (huhuy72219405 on X: "有些mcp属于本地的，
// 同步之后其他设备不应该读取到") is never in what a sync or a backup
// carries, and adding one changes nothing a sync uploads.
func TestLocalServerNotCarried(t *testing.T) {
	sandbox(t)
	ok(t)(SaveServer("", Server{Name: "gh", Transport: "stdio", Command: "gh-mcp", Agents: []string{"claude"}}))
	before := mustJSON(t, carry(t))
	ok(t)(SaveServer("", Server{Name: "here", Transport: "stdio", Command: "/opt/mine/mcp", Agents: []string{"claude"}, Local: true}))
	if s := serverIn(t, "here"); s == nil || !s.Local {
		t.Fatalf("the local server as saved: %+v", s)
	}
	b := carry(t)
	if got := carriedNames(b); !slices.Equal(got, []string{"gh"}) {
		t.Fatalf("carried %v, want gh alone", got)
	}
	if after := mustJSON(t, b); after != before {
		t.Fatalf("a local server changed what a sync carries:\n%s\n%s", before, after)
	}
	// a shared server turned local leaves what is carried; shared again, it is back
	ok(t)(ServerLocal("gh", true))
	if got := carriedNames(carry(t)); len(got) != 0 {
		t.Fatalf("gh kept here is still carried: %v", got)
	}
	ok(t)(ServerLocal("gh", false))
	if got := mustJSON(t, carry(t)); got != before {
		t.Fatalf("gh shared again isn't carried as before:\n%s\n%s", before, got)
	}
}

// A bundle coming in from another computer, without the server, keeps it
// here, kept to this computer and in the agents it was given to; the
// incoming servers go in beside it.
func TestPutKeepsLocalServer(t *testing.T) {
	sandbox(t)
	ok(t)(SaveServer("", Server{Name: "fs", Transport: "stdio", Command: "fs-mcp", Agents: []string{"claude"}}))
	other := carry(t) // another computer's library: fs alone

	h := sandbox(t)
	ok(t)(SaveServer("", Server{Name: "here", Transport: "stdio", Command: "/opt/mine/mcp", Agents: []string{"claude"}, Local: true}))
	ok(t)(SaveServer("", Server{Name: "old", Transport: "stdio", Command: "old-mcp", Agents: []string{"claude"}}))
	ok(t)(Put(&other))
	if s := serverIn(t, "here"); s == nil || !s.Local || !slices.Equal(s.Agents, []string{"claude"}) {
		t.Fatalf("the local server after a sync from another computer: %+v", s)
	}
	if serverIn(t, "fs") == nil || serverIn(t, "old") != nil {
		t.Fatal("the incoming library wasn't taken in beside it")
	}
	c := read(t, filepath.Join(h, ".claude.json"))
	if !strings.Contains(c, "/opt/mine/mcp") || !strings.Contains(c, "fs-mcp") || strings.Contains(c, "old-mcp") {
		t.Fatalf("Claude Code's servers after the sync: %s", c)
	}
	// a bundle that says it is local (only this computer says that) is taken in as shared
	in := carry(t)
	for _, s := range in.MCP {
		s.Local = true
	}
	ok(t)(Put(&in))
	if s := serverIn(t, "fs"); s == nil || s.Local {
		t.Fatalf("an incoming server's local flag was taken: %+v", s)
	}
}

// Another computer's server by the name of one kept to this computer
// doesn't take its place here: it is given to no agent here, and carried
// on as it came, so a sync from here doesn't take it from the computers
// that have it. Once the one here is shared again, or gone, the name is
// one server again.
func TestPutSameNameAsLocal(t *testing.T) {
	sandbox(t)
	ok(t)(SaveServer("", Server{Name: "db", Transport: "stdio", Command: "their-db", Agents: []string{"codex"}}))
	theirs := carry(t)
	want := mustJSON(t, theirs)

	h := sandbox(t)
	ok(t)(SaveServer("", Server{Name: "db", Transport: "stdio", Command: "/opt/my-db", Agents: []string{"claude"}, Local: true}))
	ok(t)(Put(&theirs))
	s := serverIn(t, "db")
	if s == nil || !s.Local || s.Command != "/opt/my-db" || !slices.Equal(s.Agents, []string{"claude"}) {
		t.Fatalf("this computer's db after theirs came in: %+v", s)
	}
	if c := read(t, filepath.Join(h, ".claude.json")); !strings.Contains(c, "/opt/my-db") || strings.Contains(c, "their-db") {
		t.Fatalf("Claude Code's servers: %s", c)
	}
	if c := read(t, filepath.Join(h, ".codex/config.toml")); strings.Contains(c, "their-db") {
		t.Fatalf("theirs was given to Codex here: %s", c)
	}
	if got := mustJSON(t, carry(t)); got != want {
		t.Fatalf("theirs isn't carried on as it came:\n%s\n%s", want, got)
	}
	// edits here change nothing carried
	ok(t)(ServerAgents("db", []string{"claude", "codex"}))
	if got := mustJSON(t, carry(t)); got != want {
		t.Fatalf("an edit of the local db changed what is carried:\n%s\n%s", want, got)
	}
	// a bundle without db lets theirs go; the local one stays
	none := Bundle{MCP: []*Server{}, Skills: []*CarriedSkill{}}
	ok(t)(Put(&none))
	if s := serverIn(t, "db"); s == nil || s.Command != "/opt/my-db" {
		t.Fatalf("this computer's db after a bundle without it: %+v", s)
	}
	if got := carriedNames(carry(t)); len(got) != 0 {
		t.Fatalf("carried %v after theirs was taken out", got)
	}

	// theirs again; removed here, theirs comes in as a sync would bring it
	ok(t)(Put(&theirs))
	ok(t)(RemoveServer("db"))
	if s := serverIn(t, "db"); s == nil || s.Local || s.Command != "their-db" {
		t.Fatalf("after the local db was removed: %+v", s)
	}
	// the local one shared again is the one carried
	ok(t)(SaveServer("db", Server{Name: "db", Transport: "stdio", Command: "/opt/my-db", Agents: []string{"claude"}, Local: true}))
	ok(t)(Put(&theirs))
	ok(t)(ServerLocal("db", false))
	if b := carry(t); len(b.MCP) != 1 || b.MCP[0].Command != "/opt/my-db" {
		t.Fatalf("shared again, carried %s", mustJSON(t, b))
	}
}
