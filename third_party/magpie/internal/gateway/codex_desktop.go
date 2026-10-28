package gateway

import (
	"bytes"
	"context"
	"encoding/json"
	"log"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// The Codex app's picker shows only the models of Codex's list that a
// flag of OpenAI's names, for Codex on its built-in provider, unless Codex
// reads its list from a file (model_catalog_json): beside the ChatGPT
// sign-in it showed GPT alone, magpie's models in the list it was handed
// all the same (laddieyao on Discord). The CLI shows the whole list. So
// where the Codex app is installed, magpie writes the list it hands Codex
// into a file for Codex to read (agent.CodexDesktopList), and Codex asks
// for no list then: the backend's part of it is asked for here, with
// Codex's sign-in, and kept.

// CodexDesktopChanged is called when the backend's list for the Codex app
// changed, for the file to be written again (agent.SyncCatalogSoon).
var CodexDesktopChanged func()

// codexDeskFetch asks the backend for its list (a test's stands in).
var codexDeskFetch = provider.CodexLiveList

const (
	// codexDeskFor is how long a list the backend gave stands before it
	// is asked for again; codexDeskRetry how long after one it didn't
	codexDeskFor   = time.Hour
	codexDeskRetry = 5 * time.Minute
	codexDeskWait  = 30 * time.Second
)

var codexDesk struct {
	sync.Mutex
	body       []byte
	got, tried time.Time
	asking     bool
	done       chan struct{} // closed as the ask under way ends
}

// CodexDesktopCatalog is the list a Codex beside its ChatGPT sign-in is
// handed (/backend-api/codex/models), as a file Codex reads with
// model_catalog_json: the backend's entries as it sent them, narrowed and
// set as the codex provider has them, magpie's after them. ok is false
// while the backend's list is not known: none asked for yet, or none it
// gave. That is unknown, never an empty list, and the file as it was
// stands. The backend is asked again in the background when its list is
// old, CodexDesktopChanged told when it changed.
func CodexDesktopCatalog() (b []byte, ok bool) {
	codexDesk.Lock()
	body := codexDesk.body
	stale := body == nil || time.Since(codexDesk.got) >= codexDeskFor
	if stale && !codexDesk.asking && time.Since(codexDesk.tried) >= codexDeskRetry {
		codexDesk.asking, codexDesk.tried = true, time.Now()
		codexDesk.done = make(chan struct{})
		go codexDeskRefresh(codexDesk.done)
	}
	codexDesk.Unlock()
	if body == nil {
		return nil, false
	}
	// each caller's own copy: codexMerged writes into the entries
	var list struct {
		Models []any `json:"models"`
	}
	if json.Unmarshal(body, &list) != nil || len(list.Models) == 0 {
		return nil, false
	}
	b, err := json.MarshalIndent(map[string]any{"models": codexMerged(list.Models, false)}, "", " ")
	if err != nil {
		return nil, false
	}
	return b, true
}

func codexDeskRefresh(done chan struct{}) {
	ctx, cancel := context.WithTimeout(context.Background(), codexDeskWait)
	defer cancel()
	b, err := codexDeskFetch(ctx)
	codexDesk.Lock()
	changed := false
	if err == nil {
		changed = !bytes.Equal(b, codexDesk.body)
		codexDesk.body, codexDesk.got = b, time.Now()
	}
	codexDesk.asking = false
	codexDesk.Unlock()
	if err != nil {
		log.Println("codex: the ChatGPT backend's model list for the Codex app:", err)
	}
	if changed && CodexDesktopChanged != nil {
		CodexDesktopChanged()
	}
	close(done)
}
