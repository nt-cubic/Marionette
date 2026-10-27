package sessions

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"sync/atomic"
	"testing"
	"time"
)

// statFiles gives each file what os.Stat (follow) or os.Lstat gives it, and
// leaves out what is gone or no regular file; on macOS it reads each folder
// in one listing and asks after a file alone only for a link it follows:
// the Sessions tab's every 15 s poll stat()ed all ~21k session files.
func TestStatFilesAsStat(t *testing.T) {
	dir, elsewhere := t.TempDir(), t.TempDir()
	at := time.Unix(1_790_000_000, 987_654_321)
	for i, name := range []string{"a.jsonl", "b.jsonl", "c.jsonl"} {
		p := filepath.Join(dir, name)
		if err := os.WriteFile(p, make([]byte, 10*i), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.Chtimes(p, at, at.Add(time.Duration(i))); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.WriteFile(filepath.Join(elsewhere, "real.jsonl"), []byte("linked"), 0o644); err != nil {
		t.Fatal(err)
	}
	names := []string{"a.jsonl", "b.jsonl", "c.jsonl", "gone.jsonl"}
	if runtime.GOOS != "windows" {
		if err := os.Symlink(filepath.Join(elsewhere, "real.jsonl"), filepath.Join(dir, "link.jsonl")); err != nil {
			t.Fatal(err)
		}
		if err := os.Symlink(filepath.Join(elsewhere, "nothing"), filepath.Join(dir, "broken.jsonl")); err != nil {
			t.Fatal(err)
		}
		names = append(names, "link.jsonl", "broken.jsonl")
	}
	if err := os.Mkdir(filepath.Join(dir, "folder.jsonl"), 0o755); err != nil {
		t.Fatal(err)
	}
	names = append(names, "folder.jsonl")

	var alone atomic.Int64
	was := statOne
	statOne = func(path string, follow bool) (os.FileInfo, error) {
		alone.Add(1)
		return was(path, follow)
	}
	t.Cleanup(func() { statOne = was })
	for _, follow := range []bool{true, false} {
		var files []file
		for _, n := range names {
			files = append(files, file{agent: "claude", key: "claude:" + n, path: filepath.Join(dir, n)})
		}
		alone.Store(0)
		got := statFiles(files, follow)
		var want []string
		for _, n := range names {
			info := os.Lstat
			if follow {
				info = os.Stat
			}
			fi, err := info(filepath.Join(dir, n))
			if err != nil || !fi.Mode().IsRegular() {
				continue
			}
			want = append(want, n)
			i := slices.IndexFunc(got, func(f file) bool { return filepath.Base(f.path) == n })
			if i < 0 {
				continue
			}
			if got[i].size != fi.Size() || !got[i].mod.Equal(fi.ModTime()) {
				t.Errorf("follow %v, %s: %d %v, want %d %v", follow, n, got[i].size, got[i].mod, fi.Size(), fi.ModTime())
			}
		}
		var gotNames []string
		for _, f := range got {
			gotNames = append(gotNames, filepath.Base(f.path))
		}
		if !slices.Equal(gotNames, want) {
			t.Errorf("follow %v: kept %v, want %v", follow, gotNames, want)
		}
		if runtime.GOOS == "darwin" {
			links := int64(0)
			if follow {
				links = 2
			}
			if n := alone.Load(); n != links {
				t.Errorf("follow %v: %d files asked after alone, want %d (the links followed)", follow, n, links)
			}
		}
	}
}

// Codex's rollouts read from one listing a folder are the ones walkLinked
// and statFiles read, in the same order with the same sizes and times:
// nested days, a folder or a link named as a rollout, a file that isn't
// one, and a sessions folder that is a link to another drive (#2233).
func TestListedRolloutsAsWalked(t *testing.T) {
	home, elsewhere := t.TempDir(), t.TempDir()
	put := func(p string, n int) {
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, make([]byte, n), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	for i, day := range []string{"2026/09/30", "2026/10/01", "2025/12/31", "2026/10/01/deeper"} {
		for j := range 3 {
			put(filepath.Join(elsewhere, day, fmt.Sprintf("rollout-2026-10-01T10-00-0%d-d%dr%d.jsonl", j, i, j)), i*10+j)
		}
		put(filepath.Join(elsewhere, day, "notes.txt"), 1)
	}
	put(filepath.Join(elsewhere, "2026", "10", "01", "rollout-2026-10-01T10-00-00-z.jsonl.zst"), 7)
	if err := os.Mkdir(filepath.Join(elsewhere, "2026", "rollout-2026-10-01T10-00-00-dir.jsonl"), 0o755); err != nil {
		t.Fatal(err)
	}
	direct := filepath.Join(home, "direct", "sessions")
	if err := os.CopyFS(direct, os.DirFS(elsewhere)); err != nil {
		t.Fatal(err)
	}
	if runtime.GOOS != "windows" {
		for _, top := range []string{elsewhere, direct} {
			if err := os.Symlink(filepath.Join(top, "2026", "09", "30", "rollout-2026-10-01T10-00-00-d0r0.jsonl"), filepath.Join(top, "2026", "rollout-2026-10-01T10-00-00-link.jsonl")); err != nil {
				t.Fatal(err)
			}
		}
	}
	roots := []string{direct}
	if runtime.GOOS != "windows" {
		linked := filepath.Join(home, "linked", "sessions")
		if err := os.MkdirAll(filepath.Dir(linked), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.Symlink(elsewhere, linked); err != nil {
			t.Fatal(err)
		}
		roots = append(roots, linked)
	}
	for _, root := range roots {
		var walked []file
		walkLinked(root, func(p string, d os.DirEntry, err error) error {
			if err == nil && !d.IsDir() {
				if m := rolloutName.FindStringSubmatch(d.Name()); m != nil {
					walked = append(walked, file{agent: "codex", key: "codex:" + m[1], path: p, main: true})
				}
			}
			return nil
		})
		walked = statFiles(walked, false)
		listed, ok := listedRollouts(root)
		if !ok {
			if runtime.GOOS == "darwin" {
				t.Fatalf("%s: not listed on macOS", root)
			}
			continue
		}
		if len(walked) < 13 || len(listed) != len(walked) {
			t.Fatalf("%s: listed %d rollouts, walked %d", root, len(listed), len(walked))
		}
		for i := range walked {
			w, l := walked[i], listed[i]
			if w.path != l.path || w.key != l.key || w.size != l.size || !w.mod.Equal(l.mod) || w.main != l.main {
				t.Errorf("%s: rollout %d listed %+v, walked %+v", root, i, l, w)
			}
		}
	}
	if _, ok := listedRollouts(filepath.Join(home, "none")); ok {
		t.Error("a sessions folder that isn't there was listed")
	}
}
