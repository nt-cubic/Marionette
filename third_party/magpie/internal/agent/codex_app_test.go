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

// The Codex app's Restart quits only the app the row lists as left on the
// old list, waits for its processes to end, and opens it again; an app
// that doesn't end is said, and not opened a second time.
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
	up := true
	wasRunning, wasQuit, wasOpen, wasWait := running, appQuit, appOpen, appQuitWait
	t.Cleanup(func() { running, appQuit, appOpen, appQuitWait = wasRunning, wasQuit, wasOpen, wasWait })
	running = func(pat string) []runningProc {
		if !up {
			return nil
		}
		ps := []runningProc{
			{3 * time.Hour, bundle + "/Contents/MacOS/Codex"},
			{3 * time.Hour, bundle + "/Contents/Resources/codex app-server --listen stdio://"},
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
	appQuit = func(_ context.Context, b string) error { did = append(did, "quit "+b); up = false; return nil }
	appOpen = func(_ context.Context, b string) error { did = append(did, "open "+b); return nil }
	a := codex(home)

	if err := reopenApp(context.Background(), a, "/Applications/Other.app"); err == nil || len(did) > 0 {
		t.Fatalf("an app not listed: %v, did %v", err, did)
	}
	if err := reopenApp(context.Background(), a, bundle); err != nil {
		t.Fatal(err)
	}
	if got := strings.Join(did, ", "); got != "quit "+bundle+", open "+bundle {
		t.Fatalf("did %q", got)
	}

	// the app asks something and stays open: said, not opened
	up, did = true, nil
	appQuit = func(_ context.Context, b string) error { did = append(did, "quit "+b); return nil }
	appQuitWait = 300 * time.Millisecond
	if err := reopenApp(context.Background(), a, bundle); !errors.Is(err, ErrCodexAppStillOpen) {
		t.Fatalf("still open: %v", err)
	}
	if got := strings.Join(did, ", "); got != "quit "+bundle {
		t.Fatalf("did %q", got)
	}
}
