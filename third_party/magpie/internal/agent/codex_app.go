package agent

import (
	"context"
	"errors"
	"regexp"
	"runtime"
	"slices"
	"strings"
	"time"

	"github.com/yetone/magpie/internal/proc"
)

// The Codex app on macOS keeps running, its app-server with it, when its
// window is closed, so a change to Codex's model list reaches it only once
// it is quit and opened again. The Agents row's Restart does that for the
// user (the owner, 10-23): it asks the app to quit, as ⌘Q does, waits for
// every process of its bundle to end, and opens it again.

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

// ErrCodexAppStillOpen is a quit the app didn't finish: it may be asking
// something in its window (a task still running).
var ErrCodexAppStillOpen = errors.New("the Codex app is still open; it may be asking something in its window")

// ReopenCodexApp quits the Codex desktop app at bundle and opens it again.
// Only an app the Agents row lists as left on the old list is quit, never
// a path a request names otherwise.
func ReopenCodexApp(ctx context.Context, bundle string) error {
	if runtime.GOOS != "darwin" {
		return errors.New("only the macOS Codex app is reopened by magpie")
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
	if err := appQuit(ctx, bundle); err != nil {
		return err
	}
	pat := regexp.QuoteMeta(bundle + "/Contents/")
	end := time.Now().Add(appQuitWait)
	for len(running(pat)) > 0 {
		if time.Now().After(end) {
			return ErrCodexAppStillOpen
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(200 * time.Millisecond):
		}
	}
	return appOpen(ctx, bundle)
}
