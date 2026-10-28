package agent

import (
	"context"
	"errors"
	"runtime"
	"slices"
	"strings"
	"time"

	"github.com/yetone/magpie/internal/proc"
	"github.com/yetone/magpie/internal/provider"
)

// The Codex app on macOS keeps running, its app-server with it, when its
// window is closed, so a change to Codex's model list reaches it only once
// its app-server starts again. The Agents row's Restart does that for the
// user (the owner, 10-23): it ends the app's own app-server, and the app
// starts another and keeps its window. Quitting the app and opening it
// again, as it did before, opened a new window (yoshi037070, 10-27).

// appQuit and appOpen ask macOS to quit a bundle and to open it; vars so
// tests don't touch a real app.
var (
	appQuit = func(ctx context.Context, bundle string) error {
		out, err := proc.CommandContext(ctx, "osascript", "-e", `tell application "`+strings.NewReplacer(`\`, `\\`, `"`, `\"`).Replace(bundle)+`" to quit`).CombinedOutput()
		return cmdErr(out, err)
	}
	appOpen = func(ctx context.Context, bundle string) error {
		out, err := proc.CommandContext(ctx, "open", bundle).CombinedOutput()
		return cmdErr(out, err)
	}
)

// appQuitWait is how long a quit app's processes have to end.
var appQuitWait = 30 * time.Second

func cmdErr(out []byte, err error) error {
	if err != nil && len(strings.TrimSpace(string(out))) > 0 {
		return errors.New(strings.TrimSpace(string(out)))
	}
	return err
}

// restartAppServer ends the app-server the app at a bundle runs for
// itself; a var so tests don't end a real one.
var restartAppServer = provider.RestartCodexApp

// ReopenCodexApp restarts the app-server of the Codex desktop app at
// bundle; the app starts a new one and its window stays. Only an app the
// Agents row lists as left on the old list is touched, never a path a
// request names otherwise.
func ReopenCodexApp(ctx context.Context, bundle string) error {
	if runtime.GOOS != "darwin" {
		return errors.New("only the macOS Codex app is restarted by magpie")
	}
	a, err := Find("codex")
	if err != nil {
		return err
	}
	return reopenApp(ctx, a, bundle)
}

func reopenApp(ctx context.Context, a *Agent, bundle string) error {
	if bundle == "" || !slices.ContainsFunc(a.StaleCopies(), func(c StaleCopy) bool { return c.Kind == "app" && c.Bundle == bundle }) {
		return errors.New("that Codex app has the current list already")
	}
	return restartAppServer(ctx, bundle)
}
