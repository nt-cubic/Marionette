package gui

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"sync"

	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/edit"
)

// The last-known providers list (#1519, fatkun): the Providers page asked
// /api/providers each time it opened, and after a start that read took
// seconds (every account's sign-in, every agent's model), so the page
// showed skeletons for that long. As the Agents page draws the last state
// (laststate.go), it now draws the list built last, kept here and in the
// cache folder for the next start, and the fresh one replaces it the
// moment it is in.
//
// What is kept leaves out what the page never needs before the fresh list
// is in and what can hold a secret: the gateway's recent calls and lanes,
// each provider's headers, proxies and extra body. The page opens no
// provider's editor from the kept list; it waits for the fresh one.

var lastProviders struct {
	sync.Mutex
	kept []byte
	read bool
}

type providersFile struct {
	Version   string        `json:"version"`
	Providers providersJSON `json:"providers"`
}

func lastProvidersPath() string { return filepath.Join(appdir.Cache(), "providers-state.json") }

// keepProviders remembers s as the last-known list, written only when what
// is kept changed.
func keepProviders(s providersJSON) {
	s.Gateway.Calls, s.Gateway.Lanes = nil, nil
	s.Moved, s.Added, s.Had, s.Removed, s.Fetching = nil, 0, 0, 0, false
	ps := make([]providerJSON, len(s.Providers))
	for i, p := range s.Providers {
		p.Headers, p.ExtraBody, p.Proxy, p.AccountProxies = nil, nil, "", nil
		ps[i] = p
	}
	s.Providers = ps
	b, err := json.Marshal(providersFile{Version: Version, Providers: s})
	if err != nil {
		return
	}
	lastProviders.Lock()
	defer lastProviders.Unlock()
	if !lastProviders.read {
		lastProviders.kept, _ = os.ReadFile(lastProvidersPath())
		lastProviders.read = true
	}
	if bytes.Equal(b, lastProviders.kept) {
		return
	}
	lastProviders.kept = b
	// only this user's to read: it names their accounts
	if _, err := os.Stat(lastProvidersPath()); os.IsNotExist(err) {
		os.MkdirAll(filepath.Dir(lastProvidersPath()), 0o700)
		os.WriteFile(lastProvidersPath(), nil, 0o600)
	}
	_ = edit.WriteAtomic(lastProvidersPath(), b)
}

// lastKnownProviders is the last list built, by this run or the one
// before it, as kept; false when none is kept, or another version kept it.
func lastKnownProviders() (json.RawMessage, bool) {
	lastProviders.Lock()
	if !lastProviders.read {
		lastProviders.kept, _ = os.ReadFile(lastProvidersPath())
		lastProviders.read = true
	}
	b := lastProviders.kept
	lastProviders.Unlock()
	var f struct {
		Version   string          `json:"version"`
		Providers json.RawMessage `json:"providers"`
	}
	if len(b) == 0 || json.Unmarshal(b, &f) != nil || f.Version != Version || len(f.Providers) == 0 || f.Providers[0] != '{' {
		return nil, false
	}
	// marked, for the page to tell it from a fresh one
	return append(json.RawMessage(`{"last":true,`), f.Providers[1:]...), true
}
