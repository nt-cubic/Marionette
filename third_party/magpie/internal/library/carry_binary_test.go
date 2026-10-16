package library

import (
	"archive/zip"
	"bytes"
	"encoding/json"
	"math/rand/v2"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

// pptx is a PowerPoint file as PowerPoint saves one: a zip of the
// presentation's XML parts and its media, which are already compressed
// (the pictures in a template), so the file is about as big as its media.
func pptx(t *testing.T, media int) []byte {
	t.Helper()
	var buf bytes.Buffer
	z := zip.NewWriter(&buf)
	part := func(name string, data []byte, method uint16) {
		w, err := z.CreateHeader(&zip.FileHeader{Name: name, Method: method})
		if err != nil {
			t.Fatal(err)
		}
		w.Write(data)
	}
	part("[Content_Types].xml", []byte(`<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="png" ContentType="image/png"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/></Types>`), zip.Deflate)
	part("ppt/presentation.xml", []byte(`<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"/>`), zip.Deflate)
	img := make([]byte, media)
	r := rand.New(rand.NewPCG(1, 2))
	for i := range img {
		img[i] = byte(r.Uint32())
	}
	part("ppt/media/image1.png", img, zip.Store)
	if err := z.Close(); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}

// Crispin on Discord (Skills内的template文件可以都被同步吗？好像发现无法同步PPT
// 文件诶): a skill's PowerPoint template didn't reach the other computer.
// A backup and a sync left out every file over 2 MB, and a template with
// its pictures is more than that. A skill's template comes with it, bytes
// and all, and the text of every skill still goes before what is big.
func TestCarryTemplates(t *testing.T) {
	h := sandbox(t)
	setUpLibrary(t, h)
	deck := pptx(t, 6<<20)
	write(t, filepath.Join(h, "src/pdf/templates/report.pptx"), string(deck))
	b, err := Collect()
	if err != nil {
		t.Fatal(err)
	}
	s := b.Skills[0]
	if !bytes.Equal(s.Files["templates/report.pptx"], deck) || len(s.Left) != 0 {
		t.Fatalf("the template wasn't carried: %v, left %v", keysOf(s.Files), s.Left)
	}
	raw, err := json.Marshal(b)
	if err != nil {
		t.Fatal(err)
	}

	// another computer, which hasn't the folder the skill came from
	os.RemoveAll(filepath.Join(h, "src"))
	h2 := sandbox(t)
	var in Bundle
	json.Unmarshal(raw, &in)
	ok(t)(Put(&in))
	for _, p := range []string{filepath.Join(SkillPath("pdf"), "templates/report.pptx"), filepath.Join(h2, ".claude/skills/pdf/templates/report.pptx")} {
		if got, err := os.ReadFile(p); err != nil || !bytes.Equal(got, deck) {
			t.Errorf("%s: %d bytes of %d, %v", p, len(got), len(deck), err)
		}
	}
}

// What can't go with a backup is what is bigger than all of it may be. The
// small files go first, whichever skill has them, so one skill's big file
// never keeps another's SKILL.md from coming.
func TestCarrySmallFirst(t *testing.T) {
	h := sandbox(t)
	skill(t, filepath.Join(h, "src/aaa"), "aaa", "First by name")
	skill(t, filepath.Join(h, "src/zzz"), "zzz", "Last by name")
	write(t, filepath.Join(h, "src/aaa/huge.bin"), strings.Repeat("x", maxCarried+1))
	write(t, filepath.Join(h, "src/zzz/notes.md"), strings.Repeat("n", 1024))
	// the big ones come to all a backup may have, less a few bytes
	for i := range maxCarriedAll / maxCarried {
		write(t, filepath.Join(h, "src/aaa/big"+string(rune('a'+i))+".pptx"), strings.Repeat("y", maxCarried-8))
	}
	ok(t)(InstallSkills(filepath.Join(h, "src"), []string{"aaa", "zzz"}, []string{"claude"}))
	b, err := Collect()
	if err != nil {
		t.Fatal(err)
	}
	aaa, zzz := b.Skills[0], b.Skills[1]
	if _, ok := zzz.Files["notes.md"]; !ok || len(zzz.Left) != 0 {
		t.Fatalf("zzz: %v, left %v", keysOf(zzz.Files), zzz.Left)
	}
	if _, ok := aaa.Files["SKILL.md"]; !ok || !slices.Contains(aaa.Left, "huge.bin") || len(aaa.Left) != 2 {
		t.Fatalf("aaa: %v, left %v", keysOf(aaa.Files), aaa.Left)
	}
	total := 0
	for _, s := range b.Skills {
		for _, f := range s.Files {
			total += len(f)
		}
	}
	if total > maxCarriedAll {
		t.Fatalf("carried %d bytes, over %d", total, maxCarriedAll)
	}
}

// A file the other computer couldn't carry is one it has: when this
// computer has that file in the skill too, putting the skill keeps it
// rather than setting the folder aside without it.
func TestCarryKeepsWhatWasLeft(t *testing.T) {
	h := sandbox(t)
	setUpLibrary(t, h)
	write(t, filepath.Join(h, "src/pdf/huge.pptx"), strings.Repeat("x", maxCarried+1))
	b, err := Collect()
	if err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(b.Skills[0].Left, []string{"huge.pptx"}) {
		t.Fatalf("left %v", b.Skills[0].Left)
	}
	// the same library on the other computer, its skill since edited there
	raw, _ := json.Marshal(b)
	b.Skills[0].Files["SKILL.md"] = append(b.Skills[0].Files["SKILL.md"], "\nEdited elsewhere.\n"...)
	edited, _ := json.Marshal(b)
	os.RemoveAll(filepath.Join(h, "src"))
	h2 := sandbox(t)
	var in Bundle
	json.Unmarshal(raw, &in)
	ok(t)(Put(&in))
	here := filepath.Join(SkillPath("pdf"), "huge.pptx")
	write(t, here, strings.Repeat("x", maxCarried+1))
	var again Bundle
	json.Unmarshal(edited, &again)
	ok(t)(Put(&again))
	if s := read(t, filepath.Join(SkillPath("pdf"), "SKILL.md")); !strings.Contains(s, "Edited elsewhere.") {
		t.Fatalf("SKILL.md: %q", s)
	}
	if fi, err := os.Stat(here); err != nil || fi.Size() != maxCarried+1 {
		t.Fatalf("the file this computer had went: %v %v", fi, err)
	}
	if fi, err := os.Stat(filepath.Join(h2, ".claude/skills/pdf/huge.pptx")); err != nil || fi.Size() != maxCarried+1 {
		t.Fatalf("claude's copy: %v %v", fi, err)
	}
	// a left file's name is checked as the files' are
	bad := &Bundle{Skills: []*CarriedSkill{{Name: "x", Files: map[string][]byte{"SKILL.md": nil}, Left: []string{"../../../etc/passwd"}}}}
	if _, err := Put(bad); err == nil {
		t.Fatal("put a bundle whose left file climbs out")
	}
}

// The same template on this computer: every agent gets it whole, the one
// linked to the library, one given copies, and agents in a WSL distro,
// which always get copies.
func TestSkillTemplateReachesEveryAgent(t *testing.T) {
	h := wslSandbox(t)
	src := filepath.Join(home(), "src", "skills")
	skill(t, filepath.Join(src, "pdf"), "pdf", "Read PDFs")
	deck := pptx(t, 6<<20)
	write(t, filepath.Join(src, "pdf/templates/report.pptx"), string(deck))
	ok(t)(SetSkillHow("codex", HowCopy))
	ok(t)(InstallSkills(src, []string{"pdf"}, []string{"claude", "codex", wslCodex, wslClaude, wslPi}))
	for _, p := range []string{
		filepath.Join(home(), ".claude/skills/pdf"), filepath.Join(home(), ".codex/skills/pdf"),
		filepath.Join(h, ".codex/skills/pdf"), filepath.Join(h, ".claude/skills/pdf"), filepath.Join(h, ".pi/agent/skills/pdf"),
	} {
		if got, err := os.ReadFile(filepath.Join(p, "templates/report.pptx")); err != nil || !bytes.Equal(got, deck) {
			t.Errorf("%s: %d bytes of %d, %v", p, len(got), len(deck), err)
		}
	}
}
