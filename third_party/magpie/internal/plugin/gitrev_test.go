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

// ARNO on Discord: 我安装了 "opencode-codearts-auth": "github:yuweihao17/codearts"，
// 插件页面一直有更新按钮，我点击更新更新到最新版了，还是一直提示有更新. A plugin
// from a git repository is out of date only when its repository is at
// another commit than the one bun installed: right after it is added, and
// right after it is updated, nothing waits; a new commit to the repository
// (its package.json's version left as it was, as that repository's is) is
// an update, and updating takes it away. Each way of naming a repository
// bun takes: GitHub's (asked of GitHub's API), with a branch, and a git URL
// (asked with git ls-remote).
func TestGitPluginUpdateOnlyWhenItsRepositoryMoved(t *testing.T) {
	for _, form := range []string{
		"github:yuweihao17/codearts", // ARNO's
		"github:yuweihao17/codearts#main",
		"yuweihao17/codearts",
		"https://github.com/yuweihao17/codearts",
		"git+file",
		"git+file#main",
	} {
		// (a # in the name would be in t.TempDir()'s path, and so the URL)
		t.Run(strings.ReplaceAll(form, "#", " at "), func(t *testing.T) {
			github, _ := gitSandbox(t)
			bare := filepath.Join(github, "yuweihao17", "codearts.git")
			repo := newGitRepo(t, bare)
			spec := form
			if strings.HasPrefix(form, "git+file") {
				u := "git+file://" + filepath.ToSlash(bare)
				if !strings.HasPrefix(u, "git+file:///") {
					u = "git+file:///" + strings.TrimPrefix(u, "git+file://")
				}
				spec = u + strings.TrimPrefix(form, "git+file")
			}
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
			defer cancel()
			if _, err := Add(ctx, spec); err != nil {
				t.Fatal(err)
			}
			head := func() string {
				out, err := exec.Command("git", "-C", bare, "rev-parse", "HEAD").Output()
				if err != nil {
					t.Fatal(err)
				}
				return strings.TrimSpace(string(out))
			}
			if c := GitCommit(spec); c == "" || !SameCommit(c, head()) {
				t.Fatalf("GitCommit = %q, want the repository's %s", c, head())
			}
			check := func(want string) VersionCheck {
				t.Helper()
				cs := CheckNow(ctx).Plugins
				if len(cs) != 1 || cs[0].Status != want {
					t.Fatalf("Check for updates said %+v, want %s", cs, want)
				}
				return cs[0]
			}
			waiting := func(want int) {
				t.Helper()
				if w := PendingUpdates().Waiting; len(w) != want {
					t.Fatalf("waiting = %+v, want %d", w, want)
				}
			}

			// just added: up to date, to the page and to the hourly look
			if c := check("current"); c.Head != short(head()) || c.Commit != short(head()) {
				t.Fatalf("check = %+v, want commit and head %s", c, short(head()))
			}
			if _, err := CheckUpdates(ctx); err != nil {
				t.Fatal(err)
			}
			waiting(0)

			// a new commit, the version in package.json the same
			repo.commit("README.md", "# my-oc-plugin\n\nA fix.\n")
			if c := check("update"); c.Head != short(head()) || SameCommit(c.Commit, c.Head) {
				t.Fatalf("check = %+v, want an update to %s", c, short(head()))
			}
			waiting(1)
			if u, err := CheckUpdates(ctx); err != nil || len(u.Waiting) != 1 || !u.Waiting[0].Git || u.Waiting[0].Latest != short(head()) {
				t.Fatalf("CheckUpdates = %+v, %v", u, err)
			}

			// updated: nothing waits, before anyone asks again and after
			if err := Upgrade(ctx, Name(spec)); err != nil {
				t.Fatal(err)
			}
			if c := GitCommit(spec); !SameCommit(c, head()) {
				t.Fatalf("after Update, GitCommit = %q, want %s", c, head())
			}
			waiting(0)
			check("current")
			if _, err := CheckUpdates(ctx); err != nil {
				t.Fatal(err)
			}
			waiting(0)
		})
	}
}

// commit changes a file of the plugin's repository and pushes it.
func (r gitRepo) commit(file, text string) {
	r.t.Helper()
	if err := os.WriteFile(filepath.Join(r.work, file), []byte(text), 0o644); err != nil {
		r.t.Fatal(err)
	}
	r.git(r.work, "add", "-A")
	r.git(r.work, "commit", "-qm", "change "+file)
	r.git(r.work, "push", "-q", r.bare, "HEAD:main")
}

// The commit bun installed is read as bun 1.4 writes it, from ARNO's own
// install: the package's .bun-tag, else bun.lock.
func TestGitCommitAsBunWritesIt(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("HOME", dir)
	t.Setenv("USERPROFILE", dir)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(dir, "config"))
	const spec = "github:yuweihao17/codearts"
	write := func(p, s string) {
		t.Helper()
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(s), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write(filepath.Join(Dir(), "package.json"), `{
  "name": "magpie-plugins",
  "private": true,
  "dependencies": {
    "opencode-codearts-auth": "github:yuweihao17/codearts"
  }
}`)
	write(filepath.Join(Dir(), "bun.lock"), `{
  "lockfileVersion": 2,
  "configVersion": 1,
  "workspaces": {
    "": {
      "name": "magpie-plugins",
      "dependencies": {
        "opencode-codearts-auth": "github:yuweihao17/codearts",
      },
    },
  },
  "packages": {
    "opencode-codearts-auth": ["opencode-codearts-auth@github:yuweihao17/codearts#c594984", {}, "yuweihao17-codearts-c594984", "sha512-uKnSoZPhr6F9hTkoND/T/ovUho5xCv1FEnaPuL36ZRyzaYbGIwTESjuMF1dHJXE/NkNwPxvTNCw7MSkff7CTxQ=="],
  }
}
`)
	write(filepath.Join(Dir(), "node_modules", "opencode-codearts-auth", "package.json"), `{"name": "opencode-codearts-auth", "version": "0.1.0"}`)
	if c := GitCommit(spec); c != "c594984" {
		t.Fatalf("from bun.lock: %q", c)
	}
	write(filepath.Join(Dir(), "node_modules", "opencode-codearts-auth", ".bun-tag"), "yuweihao17-codearts-8fd2d9a")
	if c := GitCommit(spec); c != "8fd2d9a" {
		t.Fatalf("from .bun-tag: %q", c)
	}
	if !SameCommit("c594984", "c594984218aa") || SameCommit("c594984", "8fd2d9a88d") || SameCommit("", "c594984") {
		t.Fatal("SameCommit")
	}
	if GitBehind(spec, "8fd2d9a88d0000000000000000000000000000ff") || !GitBehind(spec, "c594984218") {
		t.Fatal("GitBehind")
	}
}

// Where each spec's repository is asked, and a spec pinned to a commit is
// never asked at all.
func TestGitSource(t *testing.T) {
	for spec, want := range map[string][3]string{
		"github:yuweihao17/codearts":                         {"yuweihao17/codearts", "", ""},
		"github:yuweihao17/codearts#v1.2.0":                  {"yuweihao17/codearts", "", "v1.2.0"},
		"yuweihao17/codearts#main":                           {"yuweihao17/codearts", "", "main"},
		"https://github.com/yuweihao17/codearts":             {"yuweihao17/codearts", "", ""},
		"https://github.com/yuweihao17/codearts.git":         {"yuweihao17/codearts", "", ""},
		"git+https://github.com/yuweihao17/codearts.git#dev": {"yuweihao17/codearts", "", "dev"},
		"git+ssh://git@github.com/yuweihao17/codearts.git":   {"yuweihao17/codearts", "", ""},
		"git://github.com/yuweihao17/codearts.git":           {"yuweihao17/codearts", "", ""},
		"gitlab:owner/repo":                                  {"", "https://gitlab.com/owner/repo", ""},
		"bitbucket:owner/repo#x":                             {"", "https://bitbucket.org/owner/repo", "x"},
		"https://gitlab.com/owner/repo":                      {"", "https://gitlab.com/owner/repo", ""},
		"git+https://example.com/a/b.git#v2":                 {"", "https://example.com/a/b.git", "v2"},
		"git+file:///tmp/repo.git":                           {"", "file:///tmp/repo.git", ""},
	} {
		gh, remote, ref := gitSource(spec)
		if got := [3]string{gh, remote, ref}; got != want {
			t.Errorf("gitSource(%q) = %q, want %q", spec, got, want)
		}
	}
	// pinned to a commit: that commit, nobody asked (GitHub isn't there)
	api := githubAPI
	githubAPI = "http://127.0.0.1:1"
	defer func() { githubAPI = api }()
	if sha, err := remoteHead(context.Background(), "github:yuweihao17/codearts#9BF77F96F2"); err != nil || sha != "9bf77f96f2" {
		t.Fatalf("pinned: %q %v", sha, err)
	}
}
