package plugin

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// the reporter's (matrix0487, #4802): the look found 3b411a5 with fdaa691
// installed; the repository moved on to ba898e7 twenty seconds later, and
// the reader's Update installed that
const (
	reportSpec = "github:faithleysath/magpie-oh-my-aihub"
	reportPkg  = "@oh-my-aihub/magpie-plugin"
	reportWas  = "fdaa691"
	reportSeen = "3b411a5"
	reportNow  = "ba898e7"
)

// updatesSandbox is a HOME of the test's own.
func updatesSandbox(t *testing.T) {
	t.Helper()
	dir := t.TempDir()
	t.Setenv("HOME", dir)
	t.Setenv("USERPROFILE", dir)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(dir, "config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(dir, "cache"))
	forgetHeads()
	t.Cleanup(forgetHeads)
}

// gitInstalledAt fakes the git plugin spec installed as package pkg at
// commit, as bun leaves it: package.json naming the spec, and the
// package's .bun-tag (owner-repo-<sha> for GitHub's).
func gitInstalledAt(t *testing.T, spec, pkg, tag string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Join(Dir(), "node_modules", filepath.FromSlash(pkg)), 0o755); err != nil {
		t.Fatal(err)
	}
	b, _ := json.Marshal(map[string]any{"dependencies": map[string]string{pkg: spec}})
	if err := os.WriteFile(filepath.Join(Dir(), "package.json"), b, 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(Target(spec), ".bun-tag"), []byte(tag), 0o644); err != nil {
		t.Fatal(err)
	}
	b, _ = json.Marshal(map[string]string{"name": pkg, "version": "0.7.0"})
	if err := os.WriteFile(filepath.Join(Target(spec), "package.json"), b, 0o644); err != nil {
		t.Fatal(err)
	}
}

// writeWaiting writes plugin-updates.json as a look leaves it.
func writeWaiting(t *testing.T, ws ...Waiting) {
	t.Helper()
	b, _ := json.MarshalIndent(Updates{Checked: time.Date(2026, 11, 1, 14, 38, 55, 0, time.UTC), Waiting: ws, Updated: []Updated{}}, "", "  ")
	if err := os.MkdirAll(filepath.Dir(updatesPath()), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(updatesPath(), b, 0o600); err != nil {
		t.Fatal(err)
	}
}

// A git plugin updated to a commit newer than the one the last look found
// waits no more: the dot and Update all stayed up till the next look, an
// hour later, because only the very commit the look found counted as
// updated to (matrix0487, #4802). One still at the commit the look saw
// installed waits.
func TestGitWaitingGoneOnceUpdatedPastTheLook(t *testing.T) {
	report := Waiting{Spec: reportSpec, Package: reportPkg, Version: reportWas, Latest: reportSeen, Git: true}
	for _, c := range []struct {
		name, tag string
		waits     bool
	}{
		{"updated past the look", "faithleysath-magpie-oh-my-aihub-" + reportNow, false},
		{"updated to the look's", "faithleysath-magpie-oh-my-aihub-" + reportSeen, false},
		{"not updated", "faithleysath-magpie-oh-my-aihub-" + reportWas, true},
		// installed again some other way: not the one the look saw installed
		{"another install", "faithleysath-magpie-oh-my-aihub-1234567", false},
	} {
		t.Run(c.name, func(t *testing.T) {
			updatesSandbox(t)
			if err := save(List{Plugins: []Entry{{Spec: reportSpec}}}); err != nil {
				t.Fatal(err)
			}
			gitInstalledAt(t, reportSpec, reportPkg, c.tag)
			writeWaiting(t, report)
			if got := PendingUpdates().Waiting; (len(got) == 1) != c.waits {
				t.Fatalf("installed %s (look: %s installed, %s found): Waiting = %+v, want waiting %v", c.tag, reportWas, reportSeen, got, c.waits)
			}
		})
	}

	// a commit pinned in the spec: once installed at the pin it waits no more
	t.Run("pinned", func(t *testing.T) {
		updatesSandbox(t)
		const pinned = reportSpec + "#" + reportNow
		if err := save(List{Plugins: []Entry{{Spec: pinned}}}); err != nil {
			t.Fatal(err)
		}
		gitInstalledAt(t, pinned, reportPkg, "faithleysath-magpie-oh-my-aihub-"+reportNow)
		writeWaiting(t, Waiting{Spec: pinned, Package: reportPkg, Version: reportWas, Latest: reportNow, Git: true})
		if got := PendingUpdates().Waiting; len(got) != 0 {
			t.Fatalf("at its pin, Waiting = %+v", got)
		}
	})
}

// An install or update writes plugin-updates.json again without what it
// updated, rather than leaving the look's answer on file till the next
// look (#4802); an npm one updated past the version the look found too.
func TestSettleWaitingRewritesTheRecord(t *testing.T) {
	updatesSandbox(t)
	const theirs = "opencode-gemini-auth"
	if err := save(List{Plugins: []Entry{{Spec: reportSpec}, {Spec: theirs + "@latest"}}}); err != nil {
		t.Fatal(err)
	}
	gitInstalledAt(t, reportSpec, reportPkg, "faithleysath-magpie-oh-my-aihub-"+reportNow)
	installedAt(t, theirs, "1.2.0")
	writeWaiting(t,
		Waiting{Spec: reportSpec, Package: reportPkg, Version: reportWas, Latest: reportSeen, Git: true},
		Waiting{Spec: theirs + "@latest", Package: theirs, Version: "1.0.0", Latest: "1.1.0"},
	)
	settleWaiting()
	if got := readUpdates(); len(got.Waiting) != 0 || !got.Checked.Equal(time.Date(2026, 11, 1, 14, 38, 55, 0, time.UTC)) {
		t.Fatalf("on file after settling: %+v, want nothing waiting and the look's time kept", got)
	}
}

// The reporter's sequence through Upgrade and Update all, with the fake
// bun and GitHub: the look finds one commit, the repository moves on, the
// reader updates. Nothing waits after it, on file or in what the page is
// told, without another look; the update is done even when the repository
// can't be asked again after it.
func TestGitUpdatePastTheLookWaitsNoMore(t *testing.T) {
	for _, op := range []string{"upgrade", "update", "upgrade, GitHub down after"} {
		t.Run(op, func(t *testing.T) {
			github, _ := gitSandbox(t)
			ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
			defer cancel()
			repo := newGitRepo(t, filepath.Join(github, "owner", "repo.git"))
			const spec = "github:owner/repo"
			if _, err := Add(ctx, spec); err != nil {
				t.Fatal(err)
			}
			was := GitCommit(spec)
			repo.release("2.0.0") // what the look finds
			if _, err := CheckUpdates(ctx); err != nil {
				t.Fatal(err)
			}
			if w := readUpdates().Waiting; len(w) != 1 || !SameCommit(w[0].Version, was) {
				t.Fatalf("the look left %+v, want one waiting from %s", w, was)
			}
			seen := readUpdates().Waiting[0].Latest
			repo.release("3.0.0") // pushed after the look
			now := repoHead(t, repo)
			if op == "upgrade, GitHub down after" {
				down := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					http.Error(w, "down", http.StatusServiceUnavailable)
				}))
				t.Cleanup(down.Close)
				githubAPI = down.URL
			}
			var err error
			if op == "update" {
				err = Update(ctx)
			} else {
				err = Upgrade(ctx, "my-oc-plugin")
			}
			if err != nil {
				t.Fatalf("%s: %v", op, err)
			}
			if c := GitCommit(spec); !SameCommit(c, now) || SameCommit(c, seen) {
				t.Fatalf("installed %s, want %s, newer than the look's %s", c, now, seen)
			}
			if w := PendingUpdates().Waiting; len(w) != 0 {
				t.Fatalf("after the %s, the page is told %+v waits", op, w)
			}
			if w := readUpdates().Waiting; len(w) != 0 {
				t.Fatalf("after the %s, plugin-updates.json still has %+v", op, w)
			}
		})
	}
}
