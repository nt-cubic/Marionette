package davsync

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/library"
	"github.com/yetone/magpie/internal/provider"
)

// A server kept to one computer (huhuy72219405 on X: "有些mcp属于本地的，
// 同步之后其他设备不应该读取到") goes nowhere through a real sync: the
// other computer never gets it, its own server by that name neither takes
// its place nor is taken from it, and syncs either way leave both as they
// are.
func TestLocalServerThroughSync(t *testing.T) {
	f, srv := newFakeS3(t)
	cfg := f.config(srv)
	cfg.Keys, cfg.Agents = false, false
	a, b := newComputer(t), newComputer(t)
	use := func(c computer) {
		t.Helper()
		c.use(t)
		appdir.UseExecutable("")
		provider.ForgetAccounts()
		p := filepath.Join(string(c), ".claude/settings.json")
		os.MkdirAll(filepath.Dir(p), 0o755)
		if _, err := os.Stat(p); err != nil {
			os.WriteFile(p, nil, 0o644)
		}
	}
	t.Cleanup(provider.ForgetAccounts)
	now := func() {
		t.Helper()
		if err := Now(context.Background()); err != nil {
			t.Fatal(err)
		}
	}
	ok := func(_ *library.Result, err error) {
		t.Helper()
		if err != nil {
			t.Fatal(err)
		}
	}
	server := func(name string) *library.Server {
		t.Helper()
		v, err := library.Read(nil)
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
	// Claude Code's servers, under CLAUDE_CONFIG_DIR as use sets it
	claude := func(c computer) string {
		raw, _ := os.ReadFile(filepath.Join(string(c), ".claude", ".claude.json"))
		return string(raw)
	}

	// A has gh to share and db kept to itself, and syncs first
	use(a)
	ok(library.SaveServer("", library.Server{Name: "gh", Transport: "stdio", Command: "gh-mcp", Agents: []string{"claude"}}))
	ok(library.SaveServer("", library.Server{Name: "db", Transport: "stdio", Command: "/opt/a-db", Agents: []string{"claude"}, Local: true}))
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	now()

	// B joins: gh, and no db
	use(b)
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	now()
	if server("gh") == nil || server("db") != nil || strings.Contains(claude(b), "/opt/a-db") || !strings.Contains(claude(b), "gh-mcp") {
		t.Fatalf("B after joining: gh %v, db %+v; claude %s", server("gh"), server("db"), claude(b))
	}
	// B has a db of its own, shared, and syncs
	ok(library.SaveServer("", library.Server{Name: "db", Transport: "stdio", Command: "b-db", Agents: []string{"claude"}}))
	now()

	// A syncs: its db stays its own, B's isn't given to Claude Code here
	use(a)
	now()
	if s := server("db"); s == nil || !s.Local || s.Command != "/opt/a-db" {
		t.Fatalf("A's db after B's came: %+v", s)
	}
	if c := claude(a); !strings.Contains(c, "/opt/a-db") || strings.Contains(c, "b-db") {
		t.Fatalf("A's Claude Code: %s", c)
	}
	// A changes gh, which syncs up with B's db carried on
	ok(library.ServerAgents("gh", []string{}))
	now()

	// B syncs: A's change came, and B's db wasn't taken away by it
	use(b)
	now()
	if s := server("gh"); s == nil || len(s.Agents) != 0 {
		t.Fatalf("A's gh change didn't reach B: %+v", s)
	}
	if s := server("db"); s == nil || s.Command != "b-db" || s.Local {
		t.Fatalf("B's db after A's sync: %+v", s)
	}
	if strings.Contains(claude(b), "/opt/a-db") {
		t.Fatalf("A's local db reached B: %s", claude(b))
	}
}
