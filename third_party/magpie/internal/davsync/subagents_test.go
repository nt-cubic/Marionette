package davsync

import (
	"context"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/backup"
	"github.com/yetone/magpie/internal/library"
)

// A subagent made on one computer is written into the other's agents by
// its next sync, and taken out of them when the first takes it out.
func TestSyncSubagents(t *testing.T) {
	fake := &fakeDAV{files: map[string][]byte{}, etags: map[string]string{}, dirs: map[string]bool{"/dav": true}}
	srv := httptest.NewServer(fake)
	defer srv.Close()
	ctx := context.Background()
	cfg := Config{URL: srv.URL + "/dav/", User: "me", Password: "pw", Passphrase: "correct horse", Keys: true, Agents: true}
	now := func(t *testing.T) {
		t.Helper()
		if err := Now(ctx); err != nil {
			t.Fatal(err)
		}
		if v := Status(); v.Error != "" {
			t.Fatalf("sync: %+v", v)
		}
	}
	a, b := newComputer(t), newComputer(t)
	a.use(t)
	os.MkdirAll(filepath.Join(string(a), ".claude"), 0o755)
	if _, err := library.SaveSubagent("", library.Subagent{Name: "reviewer", Description: "Reviews a diff",
		Prompt: "Report bugs only.", Agents: []string{"claude", "codex"}}); err != nil {
		t.Fatal(err)
	}
	Configure(cfg)
	now(t)

	b.use(t)
	os.MkdirAll(filepath.Join(string(b), ".claude"), 0o755)
	Configure(cfg)
	now(t)
	files := []string{filepath.Join(string(b), ".claude", "agents", "reviewer.md"), filepath.Join(string(b), ".codex", "agents", "reviewer.toml")}
	for _, f := range files {
		if _, err := os.Stat(f); err != nil {
			t.Fatalf("b's agents weren't given the subagent: %v", err)
		}
	}
	puts := fake.puts
	now(t)
	if fake.puts != puts {
		t.Fatalf("b pushed back the subagents it brought in (%d puts)", fake.puts-puts)
	}

	a.use(t)
	if _, err := library.RemoveSubagents([]string{"reviewer"}); err != nil {
		t.Fatal(err)
	}
	now(t)
	b.use(t)
	now(t)
	for _, f := range files {
		if _, err := os.Stat(f); !os.IsNotExist(err) {
			t.Errorf("%s is still there after a took the subagent out: %v", f, err)
		}
	}
}

// A library with no subagents hashes as it did before they were carried,
// so a magpie that starts carrying them doesn't see a change to sync.
func TestNoSubagentsHashAsBefore(t *testing.T) {
	none := []*library.Subagent{}
	with := backup.Bundle{Library: &library.Bundle{MCP: []*library.Server{}, Skills: []*library.CarriedSkill{}, Subagents: &none}}
	before := backup.Bundle{Library: &library.Bundle{MCP: []*library.Server{}, Skills: []*library.CarriedSkill{}}}
	if hashes(with)["library"] != hashes(before)["library"] {
		t.Error("an empty list of subagents changed the library's hash")
	}
	one := []*library.Subagent{{Name: "reviewer"}}
	with.Library.Subagents = &one
	if hashes(with)["library"] == hashes(before)["library"] {
		t.Error("a subagent didn't change the library's hash")
	}
}
