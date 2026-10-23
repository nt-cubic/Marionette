package davsync

import (
	"context"
	"encoding/json"
	"errors"
	"maps"
	"slices"
	"strconv"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/backup"
	"github.com/yetone/magpie/internal/library"
)

// filled is n bytes that differ from seed to seed and part to part.
func filled(n int, seed uint32) []byte {
	b := make([]byte, n)
	x := seed*2654435761 + 1
	for i := range b {
		x = x*1664525 + 1013904223
		b[i] = byte(x >> 24)
	}
	return b
}

const backupAt = "main|team/magpie/" + file

func partAt(i int) string { return "main|team/magpie/" + file + ".parts/" + strconv.Itoa(i) }

// parts is how many part files the branch holds beside the backup.
func (f *githubFixture) parts() int {
	n := 0
	for k := range f.objects {
		if strings.HasPrefix(k, backupAt+".parts/") {
			n++
		}
	}
	return n
}

// A backup too large for one blob request goes up in parts, in one commit,
// and reads back whole (Zhang-Wei, Discord: a 50.8 MB backup, about 68 MB
// of base64 in one POST /git/blobs, got 422 "Sorry, your input was too
// large to process").
func TestGitHubSplitBackup(t *testing.T) {
	f := newGithubFixture(t)
	g, err := newGitHub(githubConfig())
	if err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	small := []byte("an earlier sealed backup")
	v, err := g.put(ctx, small, "")
	if err != nil {
		t.Fatal(err)
	}
	big := filled(50_800_000, 1)
	before := f.writes
	v, err = g.put(ctx, big, v.ETag)
	if err != nil {
		t.Fatalf("a 50.8 MB backup: %v", err)
	}
	list := f.objects[backupAt]
	if f.writes != before+1 || v.ETag != githubSHA(list) || f.parts() != 4 {
		t.Fatalf("one commit of four parts and their list, versioned by the list: commits %d, %+v, parts %d", f.writes-before, v, f.parts())
	}
	if f.biggestBlob > githubPart {
		t.Fatalf("a blob of %d bytes was sent in one request", f.biggestBlob)
	}
	b, got, err := g.get(ctx, version{})
	if err != nil || got != v || !bytesEqual(b, big) {
		t.Fatalf("reading the 50.8 MB backup back: size %d, %+v, %v", len(b), got, err)
	}
	if _, _, err := g.get(ctx, v); !errors.Is(err, errNotModified) {
		t.Fatalf("unchanged: %v", err)
	}
	if _, err := g.put(ctx, small, "old-sha"); !errors.Is(err, errChanged) || !bytesEqual(f.objects[backupAt], list) {
		t.Fatalf("a stale version must not replace the backup: %v", err)
	}

	// A smaller backup in fewer parts takes the parts it no longer has
	// away in the same commit.
	mid := filled(20<<20, 2)
	before = f.writes
	if v, err = g.put(ctx, mid, v.ETag); err != nil {
		t.Fatal(err)
	}
	if f.writes != before+1 || f.parts() != 2 {
		t.Fatalf("two parts left after one commit: commits %d, parts %d", f.writes-before, f.parts())
	}
	if b, _, err := g.get(ctx, version{}); err != nil || !bytesEqual(b, mid) {
		t.Fatalf("reading the 20 MB backup back: size %d, %v", len(b), err)
	}

	// Back to one file: no part is left behind.
	if v, err = g.put(ctx, small, v.ETag); err != nil {
		t.Fatal(err)
	}
	if f.parts() != 0 || string(f.objects[backupAt]) != string(small) {
		t.Fatalf("parts left beside a backup in one file: %d", f.parts())
	}
	if b, got, err := g.get(ctx, version{}); err != nil || got != v || string(b) != string(small) {
		t.Fatalf("reading the small backup back: %q %v", b, err)
	}
}

// A backup an older magpie wrote as one file, large or small, still reads,
// and a backup in parts can replace it.
func TestGitHubOneFileBackupStillReads(t *testing.T) {
	f := newGithubFixture(t)
	g, err := newGitHub(githubConfig())
	if err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	for _, old := range [][]byte{[]byte(`{"format":"magpie-backup","version":1}`), filled(30<<20, 3)} {
		f.objects[backupAt] = old
		f.advance("main")
		b, v, err := g.get(ctx, version{})
		if err != nil || v.ETag != githubSHA(old) || !bytesEqual(b, old) {
			t.Fatalf("a backup in one file of %d bytes: size %d, %v", len(old), len(b), err)
		}
	}
	big := filled(40<<20, 4)
	v, err := g.put(ctx, big, githubSHA(f.objects[backupAt]))
	if err != nil || f.parts() != 3 {
		t.Fatalf("parts over a backup in one file: %v, parts %d", err, f.parts())
	}
	if b, got, err := g.get(ctx, version{}); err != nil || got != v || !bytesEqual(b, big) {
		t.Fatalf("reading it back: size %d, %v", len(b), err)
	}
}

// A magpie from before parts reads the list as a newer magpie's backup and
// stops: never a damaged file to rebuild, nor one offered for replacing.
func TestGitHubPartsOlderMagpie(t *testing.T) {
	f := newGithubFixture(t)
	g, err := newGitHub(githubConfig())
	if err != nil {
		t.Fatal(err)
	}
	if _, err := g.put(context.Background(), filled(githubPart+1, 5), ""); err != nil {
		t.Fatal(err)
	}
	_, err = backup.Open(f.objects[backupAt], githubConfig().Passphrase)
	if err == nil || errors.Is(err, backup.ErrCorrupt) || errors.Is(err, backup.ErrNotBackup) || !strings.Contains(err.Error(), "newer magpie") {
		t.Fatalf("an older magpie must read the list as a newer magpie's: %v", err)
	}
	if _, ok := described(githubConfig(), f.objects[backupAt], err).(*notBackup); ok {
		t.Fatal("the list must not be offered for replacing")
	}
}

// A list that doesn't add up, or a part that isn't the one it names, is an
// error, never a backup put together wrong.
func TestGitHubPartsDamaged(t *testing.T) {
	setup := func(t *testing.T) (*githubFixture, *github, githubParts) {
		f := newGithubFixture(t)
		g, err := newGitHub(githubConfig())
		if err != nil {
			t.Fatal(err)
		}
		if _, err := g.put(context.Background(), filled(githubPart*2+10, 6), ""); err != nil {
			t.Fatal(err)
		}
		var m githubParts
		if err := json.Unmarshal(f.objects[backupAt], &m); err != nil || len(m.Parts) != 3 {
			t.Fatalf("the list: %s %v", f.objects[backupAt], err)
		}
		return f, g, m
	}
	write := func(f *githubFixture, m githubParts) {
		b, _ := json.Marshal(m)
		f.objects[backupAt] = b
		f.advance("main")
	}
	t.Run("a part changed", func(t *testing.T) {
		f, g, m := setup(t)
		f.blobs[m.Parts[1].SHA] = filled(int(m.Parts[1].Size), 7)
		if b, _, err := g.get(context.Background(), version{}); err == nil || b != nil || !strings.Contains(err.Error(), "part 2") {
			t.Fatalf("a changed part: size %d, %v", len(b), err)
		}
	})
	t.Run("a part missing", func(t *testing.T) {
		f, g, m := setup(t)
		delete(f.blobs, m.Parts[2].SHA)
		delete(f.objects, partAt(3))
		if b, _, err := g.get(context.Background(), version{}); err == nil || b != nil {
			t.Fatalf("a missing part: size %d, %v", len(b), err)
		}
	})
	t.Run("sizes", func(t *testing.T) {
		f, g, m := setup(t)
		m.Size++
		write(f, m)
		if b, _, err := g.get(context.Background(), version{}); err == nil || !strings.Contains(err.Error(), "damaged") {
			t.Fatalf("sizes that don't add up: size %d, %v", len(b), err)
		}
	})
	t.Run("newer", func(t *testing.T) {
		f, g, m := setup(t)
		m.Split = "github-parts-2"
		write(f, m)
		if _, _, err := g.get(context.Background(), version{}); err == nil || !strings.Contains(err.Error(), "newer magpie") {
			t.Fatalf("a newer magpie's parts: %v", err)
		}
	})
}

// The head moving meanwhile: a commit of another file is built on again
// with the parts already sent; another computer's backup is the merge.
func TestGitHubPartsBranchMovedMeanwhile(t *testing.T) {
	setup := func(t *testing.T) (*githubFixture, *github, version) {
		f := newGithubFixture(t)
		g, err := newGitHub(githubConfig())
		if err != nil {
			t.Fatal(err)
		}
		v, err := g.put(context.Background(), []byte("first"), "")
		if err != nil {
			t.Fatal(err)
		}
		return f, g, v
	}
	big := filled(githubPart*2+1, 8)
	t.Run("another file", func(t *testing.T) {
		f, g, v := setup(t)
		f.beforeRef = func() {
			f.beforeRef = nil
			f.objects["main|team/magpie/usage/other.magpie-usage"] = []byte("other computer")
			f.advance("main")
		}
		f.requests = nil
		if _, err := g.put(context.Background(), big, v.ETag); err != nil {
			t.Fatal(err)
		}
		posts := 0
		for _, r := range f.requests {
			if r == "POST /repos/alice/private/git/blobs" {
				posts++
			}
		}
		if posts != 4 || f.parts() != 3 || string(f.objects["main|team/magpie/usage/other.magpie-usage"]) != "other computer" {
			t.Fatalf("both commits on the branch, each blob sent once: posts %d, parts %d", posts, f.parts())
		}
		if b, _, err := g.get(context.Background(), version{}); err != nil || !bytesEqual(b, big) {
			t.Fatalf("reading it back: %v", err)
		}
	})
	t.Run("the backup", func(t *testing.T) {
		f, g, v := setup(t)
		f.beforeRef = func() {
			f.beforeRef = nil
			f.objects[backupAt] = []byte("other computer's backup")
			f.advance("main")
		}
		if _, err := g.put(context.Background(), big, v.ETag); !errors.Is(err, errChanged) {
			t.Fatalf("another computer's backup must go through the merge: %v", err)
		}
		if string(f.objects[backupAt]) != "other computer's backup" || f.parts() != 0 {
			t.Fatalf("another computer's backup was written over: parts %d", f.parts())
		}
	})
}

// An empty repository's first backup in parts follows the note that
// creates the default branch; nothing large goes through the Contents API.
func TestGitHubEmptyRepositoryPartsBackup(t *testing.T) {
	f := newGithubFixture(t)
	f.empty = true
	g, err := newGitHub(githubConfig())
	if err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	big := filled(githubPart+100, 9)
	v, err := g.put(ctx, big, "")
	if err != nil {
		t.Fatalf("a first backup in parts: %v", err)
	}
	if f.writes != 2 || len(f.objects["main|team/magpie/README.md"]) == 0 || f.parts() != 2 {
		t.Fatalf("a note, then the backup: commits %d, parts %d", f.writes, f.parts())
	}
	puts := slices.DeleteFunc(slices.Clone(f.requests), func(r string) bool { return !strings.HasPrefix(r, "PUT ") })
	if !slices.Equal(puts, []string{"PUT /repos/alice/private/contents/team/magpie/README.md"}) {
		t.Fatalf("only the note goes through the Contents API: %v", puts)
	}
	if b, got, err := g.get(ctx, version{}); err != nil || got != v || !bytesEqual(b, big) {
		t.Fatalf("reading it back: %v", err)
	}
}

// The reporter's case end to end: a library that seals to a backup past
// one request syncs, and another computer joins with all of it.
func TestGitHubSyncLargeLibrary(t *testing.T) {
	f := newGithubFixture(t)
	ctx := context.Background()
	c := githubConfig()
	a, b := newComputer(t), newComputer(t)
	a.use(t)
	// Printable, so the text is carried as it is: about 38 MB of text
	// seals to about 51 MB, as the reporter's export.
	raw := filled(28<<20, 10)
	text := string(encodeText(raw))
	if _, err := library.SaveInstructions(library.InstructionsChange{Shared: &text}); err != nil {
		t.Fatal(err)
	}
	if err := Configure(c); err != nil {
		t.Fatal(err)
	}
	if err := Now(ctx); err != nil {
		t.Fatalf("syncing a large library: %v", err)
	}
	if f.parts() < 2 {
		t.Fatalf("the backup should have gone up in parts: %d", f.parts())
	}
	b.use(t)
	if err := Configure(c); err != nil {
		t.Fatal(err)
	}
	if err := Now(ctx); err != nil {
		t.Fatalf("the second computer joins: %v", err)
	}
	got, err := library.Collect()
	if err != nil || got == nil || !slices.Contains(slices.Collect(maps.Values(got.Texts)), text) {
		t.Fatalf("the second computer's library: %v", err)
	}
}

func encodeText(b []byte) []byte {
	const letters = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 \n"
	out := make([]byte, len(b)*4/3)
	for i := range out {
		out[i] = letters[int(b[i*3/4])%len(letters)]
	}
	return out
}
