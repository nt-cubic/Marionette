package library

import (
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"testing"
)

// at makes the sandbox h this computer again, after another sandbox was.
func at(t *testing.T, h string) {
	t.Helper()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("PATH", filepath.Join(h, "bin"))
}

// carry is the library here as a sync takes it to another computer.
func carry(t *testing.T) Bundle {
	t.Helper()
	b, err := Collect()
	if err != nil || b == nil {
		t.Fatalf("collect: %v %v", b, err)
	}
	var out Bundle
	if err := json.Unmarshal([]byte(mustJSON(t, b)), &out); err != nil {
		t.Fatal(err)
	}
	return out
}

func offView(t *testing.T, name string) SkillView {
	t.Helper()
	v, err := Read(nil)
	if err != nil {
		t.Fatal(err)
	}
	for _, s := range v.Skills {
		if s.Name == name {
			return s
		}
	}
	t.Fatalf("no skill %s in %+v", name, v.Skills)
	return SkillView{}
}

func given(h, agentDir, name string) bool {
	_, err := os.Lstat(filepath.Join(h, agentDir, "skills", name))
	return err == nil
}

// A skill turned off on one computer (hanslin1666 on Discord, "只在这台电脑
// 上关掉") is off there alone: the off isn't carried to the others, and a
// sync from a computer that has it on, or gives it to more agents, doesn't
// give it to an agent here again. Turned on again here, it is as the
// others have it.
func TestSkillOffHereStaysHere(t *testing.T) {
	a := sandbox(t)
	setUpLibrary(t, a) // pdf, given to Claude Code
	fromA := carry(t)

	b := sandbox(t)
	ok(t)(Put(&fromA))
	if !given(b, ".claude", "pdf") {
		t.Fatal("the other computer's sync didn't give pdf to claude")
	}
	before := mustJSON(t, carry(t))

	// B turns it off here
	ok(t)(SkillHere("pdf", false))
	if given(b, ".claude", "pdf") {
		t.Fatal("off here, claude still has pdf")
	}
	if v := offView(t, "pdf"); !v.OffHere || !slices.Equal(v.Agents, []string{"claude"}) {
		t.Fatalf("off here: %+v", v)
	}
	// nothing a sync carries changed: B's off doesn't go up to the others
	if after := mustJSON(t, carry(t)); after != before {
		t.Fatalf("turning it off here changed what a sync carries:\n%s\n%s", before, after)
	}

	// A, which has it on, gives it to Codex too and syncs
	at(t, a)
	ok(t)(SkillAgents("pdf", []string{"claude", "codex"}))
	if !given(a, ".claude", "pdf") || !given(a, ".codex", "pdf") {
		t.Fatal("A's own agents lost pdf")
	}
	fromA = carry(t)

	// B takes A's: still off here, with A's ticks kept for the others
	at(t, b)
	ok(t)(Put(&fromA))
	if given(b, ".claude", "pdf") || given(b, ".codex", "pdf") {
		t.Fatal("a sync from a computer with it on gave pdf to an agent here again")
	}
	if v := offView(t, "pdf"); !v.OffHere || !slices.Equal(v.Agents, []string{"claude", "codex"}) {
		t.Fatalf("after a sync: %+v", v)
	}

	// the reverse: B's library, with pdf off here, goes to A, where it is on
	fromB := carry(t)
	at(t, a)
	ok(t)(Put(&fromB))
	if !given(a, ".claude", "pdf") || !given(a, ".codex", "pdf") {
		t.Fatal("a sync from the computer it is off on took it from this one")
	}
	if v := offView(t, "pdf"); v.OffHere {
		t.Fatalf("B's off reached A: %+v", v)
	}

	// on again on B: its agents get it as the others have it
	at(t, b)
	ok(t)(SkillHere("pdf", true))
	if !given(b, ".claude", "pdf") || !given(b, ".codex", "pdf") {
		t.Fatal("on again here, the agents didn't get pdf back")
	}
}

// A library.json from before, with no offHere, reads as on: nothing
// changes for a user who never turned a skill off here.
func TestSkillOffHereOldLibraryIsOn(t *testing.T) {
	h := sandbox(t)
	setUpLibrary(t, h)
	raw, err := os.ReadFile(path())
	if err != nil {
		t.Fatal(err)
	}
	var m map[string]any
	if err := json.Unmarshal(raw, &m); err != nil {
		t.Fatal(err)
	}
	for _, s := range m["skills"].([]any) {
		if _, has := s.(map[string]any)["offHere"]; has {
			t.Fatalf("a skill never turned off here writes offHere: %s", raw)
		}
	}
	ok(t)(SkillAgents("pdf", []string{"claude"}))
	if !given(h, ".claude", "pdf") || offView(t, "pdf").OffHere {
		t.Fatal("an old library's skill isn't on")
	}
	if _, err := SkillHere("nope", false); err == nil {
		t.Fatal("turned off a skill the library hasn't")
	}
}
