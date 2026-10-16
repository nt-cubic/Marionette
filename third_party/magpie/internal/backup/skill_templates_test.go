package backup

import (
	"bytes"
	"math/rand/v2"
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/library"
)

// Crispin on Discord: a skill's PowerPoint template didn't come with a
// backup or a sync. It comes now, bytes and all; and however big the
// skills' files are, the backup stays one a sync reads whole: davsync
// reads at most 64 MiB of it (readAll, dav.go's get).
func TestSkillTemplatesTravel(t *testing.T) {
	home(t)
	r := rand.New(rand.NewPCG(3, 4))
	noise := func(n int) []byte { // a template's pictures: nothing to squeeze
		b := make([]byte, n)
		for i := range b {
			b[i] = byte(r.Uint32())
		}
		return b
	}
	deck := noise(6 << 20)
	src := filepath.Join(os.Getenv("HOME"), "skills")
	for _, name := range []string{"slides", "posters", "reports"} {
		dir := filepath.Join(src, name)
		os.MkdirAll(filepath.Join(dir, "templates"), 0o755)
		os.WriteFile(filepath.Join(dir, "SKILL.md"), []byte("---\nname: "+name+"\ndescription: Decks\n---\n"), 0o644)
		big := deck
		if name != "slides" {
			big = noise(15 << 20) // past what a backup may hold, all three together
		}
		os.WriteFile(filepath.Join(dir, "templates", "brand.pptx"), big, 0o644)
	}
	if _, err := library.InstallSkills(src, []string{"slides", "posters", "reports"}, nil); err != nil {
		t.Fatal(err)
	}
	b, err := Collect(true, "test")
	if err != nil {
		t.Fatal(err)
	}
	data, err := Seal(b, "pw")
	if err != nil {
		t.Fatal(err)
	}
	if len(data) >= 64<<20 {
		t.Fatalf("the backup is %d MiB, more than a sync reads", len(data)>>20)
	}
	os.RemoveAll(src)

	home(t) // another computer
	got, err := Open(data, "pw")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := Restore(got, All); err != nil {
		t.Fatal(err)
	}
	have, err := os.ReadFile(filepath.Join(library.SkillPath("slides"), "templates", "brand.pptx"))
	if err != nil || !bytes.Equal(have, deck) {
		t.Fatalf("the template: %d bytes of %d, %v", len(have), len(deck), err)
	}
	for _, name := range []string{"posters", "reports"} {
		if _, err := os.Stat(filepath.Join(library.SkillPath(name), "SKILL.md")); err != nil {
			t.Errorf("%s: %v", name, err)
		}
	}
}
