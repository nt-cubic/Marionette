package plugin

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

// With MAGPIE_FAKE_BUN_ONE set, the test binary stands in for a bun that
// can tell when another runs in the same folder at once: it holds a busy
// file while it works, and fails when one is held already.
func init() {
	if os.Getenv("MAGPIE_FAKE_BUN_ONE") != "1" {
		return
	}
	f, err := os.OpenFile("busy", os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o644)
	if err != nil {
		fmt.Fprintln(os.Stderr, "another bun is running here:", err)
		os.Exit(1)
	}
	f.Close()
	time.Sleep(150 * time.Millisecond)
	os.Remove("busy")
	os.Exit(0)
}

// #2002 (anganing, Windows): after an update every plugin switched on
// said "no such plugin: …\node_modules\@magpie-community\opencode-grok-auth"
// at each start, and stayed so until it was taken off and added again.
// A plugin on the list whose package isn't in plugins/ is installed again,
// as its spec says, by the loop that keeps plugins updated; one that is
// there is left alone.
func TestMissingPluginsInstalledAgain(t *testing.T) {
	dir := offSandbox(t)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	const other = "@magpie-community/opencode-grok-auth"
	for _, spec := range []string{pkg + "@1.2.3", other + "@0.1.14"} {
		if _, err := Add(ctx, spec); err != nil {
			t.Fatal(err)
		}
	}
	if err := SetOff(other, true); err != nil {
		t.Fatal(err)
	}
	if err := os.RemoveAll(Target(pkg + "@1.2.3")); err != nil {
		t.Fatal(err)
	}
	if err := os.RemoveAll(Target(other + "@0.1.14")); err != nil {
		t.Fatal(err)
	}
	log := filepath.Join(dir, "bun.log")
	if err := os.Remove(log); err != nil {
		t.Fatal(err)
	}
	n, err := InstallMissing(ctx)
	if err != nil || n != 2 {
		t.Fatalf("InstallMissing = %d, %v; want 2, nil", n, err)
	}
	if got := Installed(pkg + "@1.2.3"); got != "1.2.3" {
		t.Errorf("%s installed at %q after InstallMissing, want 1.2.3", pkg, got)
	}
	// switched off, it is installed too, so it loads once switched on
	if got := Installed(other + "@0.1.14"); got != "0.1.14" {
		t.Errorf("%s (off) installed at %q after InstallMissing, want 0.1.14", other, got)
	}
	// all there now: nothing is installed again
	if err := os.Remove(log); err != nil {
		t.Fatal(err)
	}
	if n, err := InstallMissing(ctx); err != nil || n != 0 {
		t.Fatalf("InstallMissing with every plugin there = %d, %v; want 0, nil", n, err)
	}
	if b, err := os.ReadFile(log); err == nil && strings.TrimSpace(string(b)) != "" {
		t.Errorf("bun ran with every plugin there: %s", b)
	}
}

// The bun commands that change plugins/ run one at a time. Two at once
// read package.json and bun.lock before either writes them, so one's
// change is lost: a git plugin's dependency taken out of package.json
// leaves it unloadable (#2002). At start the hourly plugin updates
// (KeepUpdated) and the moved subscriptions' (keepMovedCurrent) each run
// bun add.
func TestBunCommandsOneAtATime(t *testing.T) {
	offSandbox(t)
	self, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(Dir(), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(Dir(), "package.json"), []byte(`{"name":"magpie-plugins","private":true}`), 0o644); err != nil {
		t.Fatal(err)
	}
	orig := bunCommand
	bunCommand = func(ctx context.Context, bun, dir string, args ...string) *exec.Cmd {
		cmd := exec.CommandContext(ctx, self, args...)
		cmd.Dir = dir
		cmd.Env = append(os.Environ(), "MAGPIE_FAKE_BUN_ONE=1")
		return cmd
	}
	t.Cleanup(func() { bunCommand = orig })
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	var wg sync.WaitGroup
	errs := make([]error, 4)
	run := []func() error{
		func() error { return install(ctx, pkg+"@1.2.3") },
		func() error { return installAt(ctx, "@magpie-community/opencode-grok-auth", "0.1.14") },
		func() error { return reinstall(ctx, "github:owner/repo") },
		func() error {
			_, err := runBun(ctx, "bun", "remove", "--ignore-scripts", "x")
			return err
		},
	}
	for i, f := range run {
		wg.Add(1)
		go func() {
			defer wg.Done()
			errs[i] = f()
		}()
	}
	wg.Wait()
	for _, err := range errs {
		// installAt says nothing was installed, as the fake installs
		// nothing: only bun's own failure is of interest here
		if err != nil && strings.Contains(err.Error(), "another bun is running here") {
			t.Fatalf("two bun commands ran in plugins/ at once: %v", err)
		}
	}
}
