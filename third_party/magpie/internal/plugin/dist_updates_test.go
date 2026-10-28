package plugin

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

// A community plugin npm has never had (CodeArts and River were merged and
// packed onto the dist branch, never published to npm) is offered the
// branch's newer version with the 「国内镜像」 switch on, while jsDelivr's
// copy of the branch is still the one from before it: the hourly update
// installs it, Check for updates says "update", and Update installs it
// (star21321321124 on Discord: CodeArts 0.2.3 "auto-updated", every plugin
// "up to date", Update doing nothing, with 0.2.4 on the branch and a Gcore
// edge of jsDelivr serving the index with 0.2.3 as its latest). A plugin
// with no newer version anywhere stays current, not unknown.
func TestDistOnlyPluginUpdatesPastAStaleMirror(t *testing.T) {
	for _, how := range []string{"hourly", "update"} {
		t.Run(how, func(t *testing.T) {
			sandbox(t)
			const pkg = "@magpie-community/opencode-codearts-auth"
			const river = "@magpie-community/opencode-river-auth"
			npmHasNothing(t)
			fakeDist(t, map[string][]string{pkg: {"0.2.3"}, river: {"0.1.0"}}, false)
			ctx, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
			defer cancel()
			for _, p := range []string{pkg, river} {
				if _, err := Add(ctx, p); err != nil {
					t.Fatalf("Add %s: %v", p, err)
				}
			}
			if v := Installed(pkg); v != "0.2.3" {
				t.Fatalf("installed %q, want 0.2.3", v)
			}

			// the branch gains 0.2.4; the mirror's copy is still the old one
			stale := distServer(t, map[string][]string{pkg: {"0.2.3"}, river: {"0.1.0"}}, false)
			fakeDist(t, map[string][]string{pkg: {"0.2.3", "0.2.4"}, river: {"0.1.0"}}, false)
			was := distMirror
			distMirror = func(u string) string { return stale + strings.TrimPrefix(u, distBase) }
			t.Cleanup(func() { distMirror = was })

			checks := CheckNow(ctx)
			got := map[string]VersionCheck{}
			for _, c := range checks.Plugins {
				got[c.Package] = c
			}
			if c := got[pkg]; c.Status != "update" || c.Latest != "0.2.4" || c.Version != "0.2.3" {
				t.Fatalf("Check for updates found %+v for CodeArts, want 0.2.3 with an update to 0.2.4", c)
			}
			if c := got[river]; c.Status != "current" || c.Latest != "0.1.0" {
				t.Fatalf("Check for updates found %+v for River, want 0.1.0 current", c)
			}

			if how == "hourly" {
				u, err := CheckUpdates(ctx)
				if err != nil {
					t.Fatalf("CheckUpdates: %v", err)
				}
				if len(u.Updated) != 1 || u.Updated[0].Package != pkg || u.Updated[0].From != "0.2.3" || u.Updated[0].To != "0.2.4" {
					t.Fatalf("the hourly look updated %+v, want CodeArts 0.2.3 to 0.2.4", u.Updated)
				}
			} else if err := Upgrade(ctx, pkg); err != nil {
				t.Fatalf("Update: %v", err)
			}
			if v := Installed(pkg); v != "0.2.4" {
				t.Fatalf("installed %q, want 0.2.4", v)
			}
			if v := Installed(river); v != "0.1.0" {
				t.Fatalf("River is at %q, want 0.1.0", v)
			}
		})
	}
}

// Where GitHub can't be reached, the mirror's copy of the index is read
// alone, as before.
func TestDistIndexFromTheMirrorAloneWhenGitHubFails(t *testing.T) {
	sandbox(t)
	const pkg = "@magpie-community/opencode-codearts-auth"
	mirror := distServer(t, map[string][]string{pkg: {"0.2.3", "0.2.4"}}, false)
	fakeDist(t, nil, false)
	distBase = "http://127.0.0.1:1" // nothing listens there
	was := distMirror
	distMirror = func(u string) string { return mirror + strings.TrimPrefix(u, distBase) }
	t.Cleanup(func() { distMirror = was })
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if info, ok := distInfo(ctx, pkg); !ok || info.Version != "0.2.4" {
		t.Fatalf("distInfo = %+v, %v; want the mirror's 0.2.4", info, ok)
	}
}

// GitHub answering nothing, as where it is blocked, holds the mirror's
// index back by distGrace only, not the request's whole time.
func TestDistIndexDoesntWaitOutAHangingGitHub(t *testing.T) {
	sandbox(t)
	const pkg = "@magpie-community/opencode-codearts-auth"
	mirror := distServer(t, map[string][]string{pkg: {"0.2.3", "0.2.4"}}, false)
	hang := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { <-r.Context().Done() }))
	t.Cleanup(hang.Close)
	fakeDist(t, nil, false)
	distBase = hang.URL
	was, grace := distMirror, distGrace
	distMirror = func(u string) string { return mirror + strings.TrimPrefix(u, distBase) }
	distGrace = 50 * time.Millisecond
	t.Cleanup(func() { distMirror, distGrace = was, grace })
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	if info, ok := distInfo(ctx, pkg); !ok || info.Version != "0.2.4" {
		t.Fatalf("distInfo = %+v, %v; want the mirror's 0.2.4", info, ok)
	}
	if ctx.Err() != nil {
		t.Fatal("waited out the request's whole time for GitHub")
	}
}
