package gui

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"sync"

	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/edit"
	"github.com/yetone/magpie/internal/settings"
)

// The last-known state (#1519, fatkun). state() reads every agent's config
// anew, and with WSL agents on, each distro's through \\wsl.localhost, so
// the Agents page waited 0.4–0.6 s for it on the Windows test box after a
// start (fatkun: about 2 s, and seconds more with WSL on), and the
// Providers page waited behind it. The page now draws from the state
// built last, kept here and in the cache folder for the next start, and
// the fresh one replaces it the moment it is in: an agent leaves the list
// only once a fresh read says it is gone.
//
// What is kept leaves the settings out (they hold the GitHub token and the
// LAN key): the last-known state is served with the settings as they are
// now, so it never brings back one the user changed since.

var lastState struct {
	sync.Mutex
	kept []byte // the last state built, as stateFile holds it
	read bool   // the file was looked at (or kept was set)
}

// stateFile is the last-known state, with the version that wrote it: an
// older magpie's is not drawn by a newer page.
type stateFile struct {
	Version string    `json:"version"`
	State   stateJSON `json:"state"`
}

func lastStatePath() string { return filepath.Join(appdir.Cache(), "agents-state.json") }

// keepState remembers s as the last-known state. The file is written only
// when what is kept changed.
func keepState(s stateJSON) {
	s.Settings = settings.Settings{}
	// and what is told once: a notice, a file recovered after a crash
	s.Notice, s.Connected, s.Recovered = "", nil, nil
	b, err := json.Marshal(stateFile{Version: Version, State: s})
	if err != nil {
		return
	}
	lastState.Lock()
	defer lastState.Unlock()
	if !lastState.read {
		lastState.kept, _ = os.ReadFile(lastStatePath())
		lastState.read = true
	}
	if bytes.Equal(b, lastState.kept) {
		return
	}
	lastState.kept = b
	_ = edit.WriteAtomic(lastStatePath(), b)
}

// lastKnown is the last state built, by this run or the one before it,
// with the settings as they are now; false when none is kept, or it was
// kept by another version.
func lastKnown() (stateJSON, bool) {
	lastState.Lock()
	if !lastState.read {
		lastState.kept, _ = os.ReadFile(lastStatePath())
		lastState.read = true
	}
	b := lastState.kept
	lastState.Unlock()
	var f stateFile
	if len(b) == 0 || json.Unmarshal(b, &f) != nil || f.Version != Version || f.State.Agents == nil {
		return stateJSON{}, false
	}
	f.State.Settings = settings.Load()
	f.State.Last = true
	return f.State, true
}
