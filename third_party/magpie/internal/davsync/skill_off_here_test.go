package davsync

import (
	"context"
	"os"
	"path/filepath"
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/library"
	"github.com/yetone/magpie/internal/provider"
)

// A skill turned off on one computer (hanslin1666 on Discord) stays off
// there through a real sync, and on on the other: a sync carries neither
// the off nor turns it on again, while what each agent is ticked for
// still goes both ways.
func TestSkillOffHereThroughSync(t *testing.T) {
	f, srv := newFakeS3(t)
	cfg := f.config(srv)
	cfg.Keys, cfg.Agents = false, false
	a, b := newComputer(t), newComputer(t)
	use := func(c computer) {
		t.Helper()
		c.use(t)
		appdir.UseExecutable("")
		provider.ForgetAccounts()
		for _, p := range []string{".claude/settings.json", ".codex/config.toml"} {
			p = filepath.Join(string(c), p)
			os.MkdirAll(filepath.Dir(p), 0o755)
			if _, err := os.Stat(p); err != nil {
				os.WriteFile(p, nil, 0o644)
			}
		}
	}
	t.Cleanup(provider.ForgetAccounts)
	now := func() {
		t.Helper()
		if err := Now(context.Background()); err != nil {
			t.Fatal(err)
		}
	}
	has := func(c computer, agent string) bool {
		_, err := os.Lstat(filepath.Join(string(c), agent, "skills", "pdf"))
		return err == nil
	}
	view := func() library.SkillView {
		t.Helper()
		v, err := library.Read(nil)
		if err != nil {
			t.Fatal(err)
		}
		for _, s := range v.Skills {
			if s.Name == "pdf" {
				return s
			}
		}
		t.Fatalf("no pdf: %+v", v.Skills)
		return library.SkillView{}
	}
	ok := func(_ *library.Result, err error) {
		t.Helper()
		if err != nil {
			t.Fatal(err)
		}
	}

	// A has pdf on Claude Code, and syncs first
	use(a)
	src := filepath.Join(string(a), "src", "pdf")
	os.MkdirAll(src, 0o755)
	os.WriteFile(filepath.Join(src, "SKILL.md"), []byte("---\nname: pdf\ndescription: Read PDFs\n---\nRead them.\n"), 0o644)
	ok(library.InstallSkills(filepath.Join(string(a), "src"), []string{"pdf"}, []string{"claude"}))
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	now()

	// B takes it, then turns it off on B alone and syncs
	use(b)
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	now()
	if !has(b, ".claude") {
		t.Fatal("B's first sync didn't give pdf to claude")
	}
	ok(library.SkillHere("pdf", false))
	now()
	if has(b, ".claude") || !view().OffHere {
		t.Fatal("off on B, claude still has pdf")
	}

	// A still has it on, gives it to Codex too, and syncs
	use(a)
	now()
	if v := view(); v.OffHere || !has(a, ".claude") {
		t.Fatalf("B's off reached A: %+v", v)
	}
	ok(library.SkillAgents("pdf", []string{"claude", "codex"}))
	now()

	// B syncs: still off here, A's ticks taken in for when it is on again
	use(b)
	now()
	if has(b, ".claude") || has(b, ".codex") {
		t.Fatal("a sync from A, with pdf on, gave it to an agent on B again")
	}
	if v := view(); !v.OffHere || !slices.Equal(v.Agents, []string{"claude", "codex"}) {
		t.Fatalf("B after the sync: %+v", v)
	}
	ok(library.SkillHere("pdf", true))
	if !has(b, ".claude") || !has(b, ".codex") {
		t.Fatal("on again on B, its agents didn't get pdf back")
	}
}
