package gui

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/gateway"
)

// A request whose User-Agent names only an HTTP library, a runtime or an
// SDK is no agent (theforeveriris on X: Bun, curl and python-httpx each
// had an agent's scorecard). The Context tab puts them on one card,
// "scripts", which names them, the most used first; an agent keeps its
// own. Read from the history as it was kept before, so days already on
// disk read the same way, and the days kept for the next refresh keep each
// route's own agent.
func TestContextScriptsAreOneCard(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	mux := keptMux(t)
	ctxKept.Lock()
	ctxKept.days, ctxKept.key = map[string]ctxDay{}, ""
	ctxKept.Unlock()

	now := time.Now()
	var b bytes.Buffer
	id := int64(0)
	add := func(agent, session string, n int) {
		for range n {
			id++
			r := ctxRoute(id, agent, session, 3000, int(id)*100, 200000, 0)
			r.Time = time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, time.Local).Add(time.Duration(id) * time.Second)
			j, _ := json.Marshal(r)
			b.Write(append(j, '\n'))
		}
	}
	add("python-httpx", "p1", 4)
	add("Bun", "b1", 2)
	add("curl", "", 1)
	add("other", "", 1)
	add("OpenAI", "o1", 1)
	add("claude", "c1", 3)
	dir := gateway.HistoryDir()
	os.MkdirAll(dir, 0o700)
	os.WriteFile(filepath.Join(dir, now.Format("2006-01-02")+".jsonl"), b.Bytes(), 0o600)

	for round := range 2 {
		var out ctxJSON
		getJSON(t, mux, "/api/context?days=1", &out)
		var ids []string
		for _, a := range out.Agents {
			ids = append(ids, a.Agent)
		}
		if !slices.Equal(ids, []string{"scripts", "claude"}) {
			t.Fatalf("round %d: cards %v", round, ids)
		}
		s := out.Agents[0]
		if s.Requests != 9 || !slices.Equal(s.Clients, []string{"python-httpx", "Bun", "OpenAI", "curl"}) {
			t.Fatalf("round %d: scripts' card %+v", round, s)
		}
		for _, x := range out.Sessions {
			if x.Agent != "scripts" && x.Agent != "claude" {
				t.Fatalf("round %d: a session under %q", round, x.Agent)
			}
		}
	}
	ctxKept.Lock()
	defer ctxKept.Unlock()
	for _, d := range ctxKept.days {
		for _, r := range d.recs {
			if r.Agent == "scripts" {
				t.Fatalf("the kept day's route %d lost its own agent", r.ID)
			}
		}
	}
}
