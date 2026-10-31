package masking

import (
	"context"
	"encoding/json"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/pii"
	"github.com/yetone/magpie/internal/redact"
	"github.com/yetone/magpie/internal/settings"
)

// slowReader is the local model on a slow CPU: a text it is asked about
// the first time isn't read in the request's wait (a pii.Partial, nothing
// found in it), and it has been read by the next request, which finds
// the name in it.
type slowReader struct {
	fake
	name string
	mu   sync.Mutex
	read map[string]bool
}

func (m *slowReader) Find(_ context.Context, texts []string) ([][]redact.Found, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	out := make([][]redact.Found, len(texts))
	missing := 0
	for i, t := range texts {
		if !m.read[t] {
			m.read[t] = true // read after the request stopped waiting
			missing++
			continue
		}
		if at := strings.Index(t, m.name); at >= 0 {
			out[i] = []redact.Found{{Start: at, End: at + len(m.name), Label: "private_person", Score: 0.95}}
		}
	}
	if missing > 0 {
		return out, &pii.Partial{Missing: missing, Of: len(texts)}
	}
	return out, nil
}

// vvrfxyz on X: with masking on, dsh's prompt-cache hits fell. Replayed
// on a Linux CPU, a tool's output (customers.csv) the default model
// hadn't read in its 2s went with the rules' masking alone, and the next
// turn masked a name in it the model had read since: from that message on
// the prompt was new to the vendor. A text sent before on a route goes on
// it as it went; new text has the name masked, and another route, which
// was never sent the text, gets it masked.
func TestModelReadingLateKeepsTheSentPrefix(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	if err := settings.Save(settings.Settings{Redact: true, RedactPersonal: true, RedactModelAuto: true, RedactModelName: pii.AstrLinkGuard}); err != nil {
		t.Fatal(err)
	}
	const name = "Fatima Rossi"
	m := &slowReader{fake: fake{installed: true}, name: name, read: map[string]bool{}}
	wasPicked, wasFetch, wasDisk := picked, fetch, onDisk
	picked = func(settings.Settings) pii.Model { return m }
	fetch = func(pii.Model, bool) {}
	onDisk = func() bool { return true }
	t.Cleanup(func() { picked, fetch, onDisk = wasPicked, wasFetch, wasDisk })

	msg := func(role, text string) string {
		b, _ := json.Marshal(map[string]string{"role": role, "content": text})
		return string(b)
	}
	csv := "1: id,name,company,city\n2: 1000," + name + ",Client 0 GmbH,Berlin\n3: 1001,Kenji Ito,Client 1 SA,Lyon"
	turn1 := []string{msg("system", "You are dsh."), msg("user", "read customers.csv"), msg("tool", csv)}
	turn2 := append(append([]string{}, turn1...), msg("assistant", "Read."), msg("user", "who is "+name+"?"))
	body := func(ms []string) []byte { return []byte(`{"model":"m","messages":[` + strings.Join(ms, ",") + `]}`) }
	messages := func(b []byte) []string {
		var q struct{ Messages []json.RawMessage }
		if err := json.Unmarshal(b, &q); err != nil {
			t.Fatalf("%v: %s", err, b)
		}
		out := make([]string, len(q.Messages))
		for i, r := range q.Messages {
			out[i] = string(r)
		}
		return out
	}
	route := WithRoute(context.Background(), "deepseek")
	first, _ := Masker{}.JSONHitsContext(route, body(turn1))
	if !strings.Contains(string(first), name) {
		t.Fatalf("turn 1: the model hadn't read the tool's output, the rules alone mask it: %s", first)
	}
	second, _ := Masker{}.JSONHitsContext(route, body(turn2))
	a, b := messages(first), messages(second)
	for i := range a {
		if a[i] != b[i] {
			t.Errorf("turn 2 rewrote message %d the vendor has:\n%s\nwas\n%s", i, b[i], a[i])
		}
	}
	if strings.Contains(b[len(b)-1], name) {
		t.Errorf("the new turn has the name the model found as written: %s", b[len(b)-1])
	}

	// another route was never sent the text: masked with what is found now
	other, _ := Masker{}.JSONHitsContext(WithRoute(context.Background(), "another"), body(turn1))
	if strings.Contains(string(other), name) {
		t.Errorf("another route got the name as written: %s", other)
	}
	// nor is one without a route (a search, an embedding)
	if none, _ := (Masker{}).JSONHitsContext(context.Background(), body(turn1)); strings.Contains(string(none), name) {
		t.Errorf("no route got the name as written: %s", none)
	}
}
