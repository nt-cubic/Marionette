package agent

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

// The Codex app's Restart restarts only the app-server of the app the row
// lists as left on the old list. It never quits the app and opens it
// again, which opened a new window (yoshi037070, 10-27).
func TestCodexAppReopen(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip(staleOnWindows)
	}
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	t.Setenv("CODEX_HOME", "")
	dir := filepath.Join(home, ".codex")
	os.MkdirAll(dir, 0o755)
	os.WriteFile(filepath.Join(dir, "config.toml"), []byte("model_provider = \"magpie\"\n"), 0o600)

	const bundle = "/Applications/Codex.app"
	wasRunning, wasQuit, wasOpen, wasRestart := running, appQuit, appOpen, restartAppServer
	t.Cleanup(func() { running, appQuit, appOpen, restartAppServer = wasRunning, wasQuit, wasOpen, wasRestart })
	running = func(pat string) []runningProc {
		ps := []runningProc{
			{3 * time.Hour, bundle + "/Contents/MacOS/Codex"},
			{3 * time.Hour, bundle + "/Contents/Resources/codex app-server"},
		}
		var out []runningProc
		for _, p := range ps {
			if strings.Contains(pat, "codex") && strings.Contains(p.cmd, "codex ") || strings.Contains(pat, `Codex\.app/Contents/`) {
				out = append(out, p)
			}
		}
		return out
	}
	var did []string
	appQuit = func(_ context.Context, b string) error { did = append(did, "quit "+b); return nil }
	appOpen = func(_ context.Context, b string) error { did = append(did, "open "+b); return nil }
	restartAppServer = func(_ context.Context, b string) error { did = append(did, "restart "+b); return nil }
	a := codex(home)

	if err := reopenApp(context.Background(), a, "/Applications/Other.app"); err == nil || len(did) > 0 {
		t.Fatalf("an app not listed: %v, did %v", err, did)
	}
	if err := reopenApp(context.Background(), a, bundle); err != nil {
		t.Fatal(err)
	}
	if got := strings.Join(did, ", "); got != "restart "+bundle {
		t.Fatalf("did %q, want only its app-server restarted", got)
	}

	// a restart that fails is said
	did = nil
	restartAppServer = func(context.Context, string) error { return errors.New("didn't end") }
	if err := reopenApp(context.Background(), a, bundle); err == nil || len(did) > 0 {
		t.Fatalf("failed restart: %v, did %v", err, did)
	}
}
