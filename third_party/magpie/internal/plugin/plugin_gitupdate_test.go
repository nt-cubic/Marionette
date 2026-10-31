package plugin

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// gitFileURL is a bare repository's git+file:// URL, as a plugin spec.
func gitFileURL(bare string) string {
	u := "git+file://" + filepath.ToSlash(bare)
	if !strings.HasPrefix(u, "git+file:///") {
		u = "git+file:///" + strings.TrimPrefix(u, "git+file://")
	}
	return u
}

// realBunSandbox is a HOME of the test's own where bun is the one on PATH,
// not the fake: how bun resolves a git spec (through the clone of the
// repository it keeps in its install cache) is the one thing the fake
// can't stand in for.
func realBunSandbox(t *testing.T) {
	t.Helper()
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git on PATH")
	}
	bun, err := exec.LookPath("bun")
	if err != nil {
		t.Skip("no bun on PATH")
	}
	dir := t.TempDir()
	t.Setenv("HOME", dir)
	t.Setenv("USERPROFILE", dir)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(dir, "config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(dir, "cache"))
	t.Setenv("MAGPIE_BUN", bun) // never downloaded
	t.Cleanup(Settle)
	forgetHeads()
}

// repoHead is the commit the repository's main is at now.
func repoHead(t *testing.T, repo gitRepo) string {
	t.Helper()
	out, err := exec.Command("git", "--git-dir", repo.bare, "rev-parse", "main").Output()
	if err != nil {
		t.Fatal(err)
	}
	return strings.TrimSpace(string(out))
}

// A plugin added from a git URL that names no commit or tag stayed at the
// commit of its first install: bun resolves the spec's branch through the
// clone of the repository in its install cache, whose refs stay at what
// they were when bun cloned it first, so an update (and an uninstall
// followed by an add again) said it had installed while node_modules kept
// the old commit (#2954). Fetched before bun resolves, the clone's refs
// name the commit the repository is at now.
func TestGitUpdateInstallsTheBranchsCommitNow(t *testing.T) {
	realBunSandbox(t)
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	repo := newGitRepo(t, filepath.Join(t.TempDir(), "repo.git"))
	spec := gitFileURL(repo.bare)
	if _, err := Add(ctx, spec); err != nil {
		t.Fatal(err)
	}
	if v := Installed(spec); v != "1.0.0" {
		t.Fatalf("installed %s, want 1.0.0", v)
	}
	repo.release("2.0.0")
	if err := Update(ctx); err != nil {
		t.Fatalf("update: %v", err)
	}
	if v := Installed(spec); v != "2.0.0" {
		t.Fatalf("the update said it is done, the plugin is still at %s, want 2.0.0", v)
	}
	if c := GitCommit(spec); !SameCommit(c, repoHead(t, repo)) {
		t.Fatalf("installed commit %s, want the repository's %s", c, repoHead(t, repo))
	}

	// an uninstall followed by an add again resolved the old commit just
	// the same (#2954); it resolves the branch's commit now
	repo.release("3.0.0")
	if err := Remove(ctx, "my-oc-plugin"); err != nil {
		t.Fatal(err)
	}
	if _, err := Add(ctx, spec); err != nil {
		t.Fatal(err)
	}
	if v := Installed(spec); v != "3.0.0" {
		t.Fatalf("installed again: %s, want 3.0.0", v)
	}
}

// An update that left the plugin at the commit it was at, though the
// repository's check had said there was a newer one, is not reported as
// done: bun resolving a branch through a stale cache has done exactly
// that, and exited 0 (#2954). The fake bun stands in for that bun: its
// update keeps the commit the lock has.
func TestGitUpdateSaysWhenItDidnt(t *testing.T) {
	github, _ := gitSandbox(t)
	t.Setenv("MAGPIE_FAKE_BUN_STUCK", "1")
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	repo := newGitRepo(t, filepath.Join(github, "owner", "repo.git"))
	const spec = "github:owner/repo"
	if _, err := Add(ctx, spec); err != nil {
		t.Fatal(err)
	}
	repo.release("2.0.0")
	// the page's check asked the repository and found the new commit, as
	// it does before an update is offered
	if h, err := GitHead(ctx, spec, true); err != nil || h == "" {
		t.Fatalf("GitHead = %q, %v", h, err)
	}
	if err := Update(ctx); err == nil {
		t.Fatal("the update said it is done, though the plugin is still at the commit it was at")
	}
}

// A plugin whose spec names the commit to install keeps it: nothing is
// re-resolved for one, and an update moves everything but its pin (#2954).
func TestGitUpdateKeepsACommitPin(t *testing.T) {
	realBunSandbox(t)
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	repo := newGitRepo(t, filepath.Join(t.TempDir(), "repo.git"))
	pin := repoHead(t, repo)
	spec := gitFileURL(repo.bare) + "#" + pin
	if _, err := Add(ctx, spec); err != nil {
		t.Fatal(err)
	}
	if v := Installed(spec); v != "1.0.0" {
		t.Fatalf("installed %s, want 1.0.0", v)
	}
	repo.release("2.0.0")
	if err := Update(ctx); err != nil {
		t.Fatalf("update: %v", err)
	}
	if v := Installed(spec); v != "1.0.0" {
		t.Fatalf("a commit-pinned plugin moved to %s", v)
	}
	if c := GitCommit(spec); !SameCommit(c, pin) {
		t.Fatalf("a commit-pinned plugin is at %s, want its pin %s", c, pin)
	}
	if b, err := os.ReadFile(filepath.Join(Dir(), "bun.lock")); err != nil || !strings.Contains(string(b), "#"+pin) {
		t.Fatalf("bun.lock no longer pins %s: %v", pin, err)
	}
}

func TestRefFixed(t *testing.T) {
	for spec, want := range map[string]bool{
		"github:owner/repo":                                                 false,
		"github:owner/repo#main":                                            false,
		"github:owner/repo#v1.2.0":                                          false,
		"git+https://host/x/y.git":                                          false,
		"git+https://host/x/y.git#semver:>=1.0":                             false,
		"git+file:///tmp/repo.git#MAIN":                                     false,
		"git+https://host/x/y.git#8c49a23":                                  true,
		"git+https://host/x/y.git#8c49a230beef":                             true,
		"git+file:///tmp/repo.git#75068205e8ef0d146deef13b1ca6cf80e6432ab0": true,
	} {
		if got := refFixed(spec); got != want {
			t.Errorf("refFixed(%q) = %v, want %v", spec, got, want)
		}
	}
}

func TestSameRepoAndCloneOrigin(t *testing.T) {
	for _, c := range []struct {
		a, b string
		want bool
	}{
		{"git+https://git.example.com/a/b.git", "https://git.example.com/a/b.git", true},
		{"git+https://git.example.com/a/b", "https://git.example.com/a/b.git", true},
		{"https://GitHub.com/a/b.git", "https://github.com/a/b", true},
		{"git+https://git.example.com/a/b.git", "https://git.example.com/a/c.git", false},
		{"git+file:///tmp/x/repo.git", "file:///tmp/x/repo.git", true},
		{"", "https://git.example.com/a/b.git", false},
	} {
		if got := sameRepo(c.a, c.b); got != c.want {
			t.Errorf("sameRepo(%q, %q) = %v, want %v", c.a, c.b, got, c.want)
		}
	}
	dir := t.TempDir()
	cfg := "[core]\n\trepositoryformatversion = 0\n[remote \"origin\"]\n\turl = https://git.example.com/a/b.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n"
	if err := os.WriteFile(filepath.Join(dir, "config"), []byte(cfg), 0o644); err != nil {
		t.Fatal(err)
	}
	if got, want := cloneOrigin(dir), "https://git.example.com/a/b.git"; got != want {
		t.Errorf("cloneOrigin = %q, want %q", got, want)
	}
	if got := cloneOrigin(t.TempDir()); got != "" {
		t.Errorf("cloneOrigin of a folder with no config = %q", got)
	}
}
