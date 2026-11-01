package provider

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/yetone/magpie/internal/appdir"
)

// Claude Code's own locks, taken as Claude Code takes them (2.1.316, its
// bundled proper-lockfile): a lock is a directory made with mkdir, held
// while it stands and let go by removing it. Its holder touches it every
// few seconds; one not touched for longer than the lock's stale time is
// left by a holder that died, and is taken over.
//
// Claude Code refreshes its sign-in under two of them, in this order:
// <config dir>/.oauth_refresh.lock, then the older <config dir>.lock
// (~/.claude.lock), both stale after 60s. Under them it reads the sign-in
// again, posts the refresh token, and writes back the new pair. A switch
// that read the sign-in it was replacing while a refresh ran kept that
// account's refresh token as it was before the refresh: spent, and the
// account signed out of magpie on its next use (#4532). It saves
// ~/.claude.json under <that file>.lock, stale after 10s, while it may
// hold the refresh locks: so it is taken after them, as Claude Code does.

var (
	// claudeLockWait is how long a switch waits for Claude Code to let a
	// lock go. A refresh holds them for one token request; Claude Code
	// itself gives up after five tries a second or two apart.
	claudeLockWait = 20 * time.Second
	// claudeLockPoll is how often a held lock is looked at again.
	claudeLockPoll = 100 * time.Millisecond
	// claudeLockTouch is how often a lock magpie holds is touched, as
	// Claude Code's update (5s for the refresh locks, half the stale
	// time for ~/.claude.json's), so it never reads as left behind.
	claudeLockTouch = 2 * time.Second
)

const (
	claudeRefreshStale = 60 * time.Second
	claudeConfigStale  = 10 * time.Second
)

// errClaudeLocked is a lock Claude Code held past claudeLockWait.
var errClaudeLocked = errors.New("Claude Code is refreshing its sign-in: try again in a moment")

// claudeRefreshLocks takes the locks Claude Code refreshes its sign-in
// under, for the config directory magpie reads the sign-in of. The
// release lets them go in the reverse order. A directory not there yet
// (Claude Code never run) has no refresh to wait for.
func claudeRefreshLocks() (release func(), err error) {
	dir := appdir.ClaudeDir()
	if fi, err := os.Stat(dir); err != nil || !fi.IsDir() {
		return func() {}, nil
	}
	first, err := takeClaudeLock(filepath.Join(dir, ".oauth_refresh.lock"), claudeRefreshStale)
	if err != nil {
		return nil, err
	}
	resolved := dir
	if r, err := filepath.EvalSymlinks(dir); err == nil {
		resolved = r
	}
	legacy, err := takeClaudeLock(resolved+".lock", claudeRefreshStale)
	if err != nil {
		first()
		return nil, err
	}
	return func() { legacy(); first() }, nil
}

// claudeConfigLock takes the lock Claude Code saves ~/.claude.json under:
// at the file's real path (proper-lockfile resolves it) plus ".lock".
func claudeConfigLock() (release func(), err error) {
	p := claudeProfilePath()
	if r, err := filepath.EvalSymlinks(p); err == nil {
		p = r
	}
	if fi, err := os.Stat(filepath.Dir(p)); err != nil || !fi.IsDir() {
		return func() {}, nil
	}
	return takeClaudeLock(p+".lock", claudeConfigStale)
}

// takeClaudeLock makes the lock directory path, waiting while another
// holds it and taking over one left stale. Held, it is touched until
// released.
func takeClaudeLock(path string, stale time.Duration) (release func(), err error) {
	deadline := time.Now().Add(claudeLockWait)
	for {
		err := os.Mkdir(path, 0o755)
		if err == nil {
			break
		}
		if !os.IsExist(err) {
			return nil, fmt.Errorf("take Claude Code's lock %s: %w", path, err)
		}
		if fi, serr := os.Stat(path); serr == nil && time.Since(fi.ModTime()) > stale {
			// its holder died without letting it go: Claude Code takes
			// such a lock over the same way
			if os.Remove(path) == nil {
				continue
			}
		}
		if time.Now().After(deadline) {
			return nil, errClaudeLocked
		}
		time.Sleep(claudeLockPoll)
	}
	stop, done := make(chan struct{}), make(chan struct{})
	go func() {
		defer close(done)
		t := time.NewTicker(claudeLockTouch)
		defer t.Stop()
		for {
			select {
			case <-stop:
				return
			case <-t.C:
				now := time.Now()
				_ = os.Chtimes(path, now, now)
			}
		}
	}()
	return func() {
		close(stop)
		<-done
		_ = os.Remove(path)
	}, nil
}
