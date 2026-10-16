package gateway

// Native compaction (#1099): a provider the user set to compact natively
// (provider.Provider.NativeCompaction) is asked Codex's compaction_trigger
// as Codex sent it, and the compaction item it writes back goes to Codex
// as it came, sealed (encrypted_content), not as magpie's "magpie1:"
// summary. Only the key or account that sealed it can read it again, so
// magpie remembers who wrote each one (compactions.json, beside
// affinity.json), and a later request carrying it goes only there. When
// that one can't be asked — switched off, removed, out of the group —
// the request is refused, saying why: the compaction is never sent to
// another to be refused, dropped (withoutCompaction), or summarised in its
// place, which would lose the conversation's earlier context unasked.

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/tidwall/gjson"
	"github.com/yetone/magpie/internal/edit"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/steady"
)

const (
	// checkpointKeep is how long who sealed a compaction is remembered: a
	// conversation goes on from its compaction for days.
	checkpointKeep = 30 * 24 * time.Hour
	// checkpointsKept is how many compactions, the latest, are kept.
	checkpointsKept = 1024
)

// compactionTriggerIn says whether a Responses request's input holds
// Codex's compaction_trigger.
func compactionTriggerIn(body []byte) bool {
	if !bytes.Contains(body, []byte(`"compaction_trigger"`)) {
		return false
	}
	for _, it := range gjson.GetBytes(body, "input").Array() {
		if it.Get("type").Str == "compaction_trigger" {
			return true
		}
	}
	return false
}

// sealedCompactions are the encrypted_content of the compaction items in
// a Responses request's input that a vendor sealed: magpie's own
// ("magpie1:") are text by the time a request is sent, and aren't.
func sealedCompactions(body []byte) []string {
	if !bytes.Contains(body, []byte(`"compaction`)) {
		return nil
	}
	var out []string
	for _, it := range gjson.GetBytes(body, "input").Array() {
		if !slices.Contains(compactionKinds, it.Get("type").Str) {
			continue
		}
		if enc := it.Get("encrypted_content").Str; enc != "" && !strings.HasPrefix(enc, magpieCompaction) {
			out = append(out, enc)
		}
	}
	return out
}

// carriesCompaction is whether c's provider can be asked Codex's own
// compaction for model: its wire carries it, and model is asked on
// Responses there.
func (s *Server) carriesCompaction(p provider.Provider, model string) bool {
	return p.CarriesCompaction() && slices.Contains(s.usable(p, model), provider.Responses)
}

// compactsNatively is whether a Codex compaction for model at p goes to it
// as Codex sent it (provider.Provider.CompactsOn).
func (s *Server) compactsNatively(p provider.Provider, model string) bool {
	return p.CompactsOn(model) && s.carriesCompaction(p, model)
}

// nativeCompactionFor is whether Codex's compaction_trigger for the model
// it asked (a provider's model, or a routing group) goes on as Codex sent
// it: every member of a group compacting natively, as any of them may
// answer it. A group with one that doesn't keeps magpie's summary, which
// any member can read.
func (s *Server) nativeCompactionFor(asked string) bool {
	if _, ms, ok := provider.FindGroup(asked); ok {
		if len(ms) == 0 {
			return false
		}
		for _, m := range ms {
			if !s.compactsNatively(m.Provider, m.Model) {
				return false
			}
		}
		return true
	}
	p, model, ok := provider.Resolve(asked)
	return ok && s.compactsNatively(p, model)
}

// keepsCompaction is whether body goes to p as it is for its compaction:
// Codex's trigger, to one that carries it, or a sealed compaction, to one
// set to compact natively or recorded as its writer — a fallback that
// answered a native compaction for another sealed it whatever its own
// setting (#2271). A translation would leave either out, so such a request
// is relayed.
func (s *Server) keepsCompaction(from provider.Protocol, p provider.Provider, model string, body []byte) bool {
	if from != provider.Responses || !s.carriesCompaction(p, model) {
		return false
	}
	if compactionTriggerIn(body) {
		return true
	}
	if len(sealedCompactions(body)) == 0 {
		return false
	}
	if p.CompactsOn(model) {
		return true
	}
	cp, ok := checkpointOf(body)
	return ok && cp.Provider == p.ID
}

// checkpointWriter is who sealed a compaction: the key or account
// (candidate.who), its provider and the model it answered as.
type checkpointWriter struct {
	Who      string    `json:"who"`
	Provider string    `json:"provider"`
	Model    string    `json:"model,omitempty"`
	At       time.Time `json:"at"`
}

var checkpoints = struct {
	sync.Mutex
	m map[string]checkpointWriter // hex sha256 of encrypted_content → who wrote it
	// the file as last read or written
	from string
	mod  time.Time
}{m: map[string]checkpointWriter{}}

var checkpointsSaving sync.Mutex

func checkpointsPath() string {
	return filepath.Join(filepath.Dir(provider.Path()), "compactions.json")
}

func checkpointKey(enc string) string {
	h := sha256.Sum256([]byte(enc))
	return hex.EncodeToString(h[:])
}

// checkpointFor is who sealed the compaction enc, read from the file
// again when it isn't known here and the file changed since. Called with
// checkpoints held.
func checkpointFor(enc string) (checkpointWriter, bool) {
	path := checkpointsPath()
	if path != checkpoints.from && checkpoints.from != "" {
		// another home (a test's): what was known is of the other's
		checkpoints.m = map[string]checkpointWriter{}
		checkpoints.mod = time.Time{}
	}
	k := checkpointKey(enc)
	if cp, ok := checkpoints.m[k]; ok && time.Since(cp.At) <= checkpointKeep {
		return cp, true
	}
	fi, err := os.Stat(path)
	if err != nil || path == checkpoints.from && fi.ModTime().Equal(checkpoints.mod) {
		checkpoints.from = path
		return checkpointWriter{}, false
	}
	checkpoints.from, checkpoints.mod = path, fi.ModTime()
	b, err := os.ReadFile(path)
	if err != nil {
		return checkpointWriter{}, false
	}
	var saved map[string]checkpointWriter
	if decodeJSON(b, &saved) != nil {
		return checkpointWriter{}, false
	}
	for sk, v := range saved {
		if v.Who == "" || time.Since(v.At) > checkpointKeep {
			continue
		}
		if had, ok := checkpoints.m[sk]; ok && !had.At.Before(v.At) {
			continue
		}
		checkpoints.m[sk] = v
	}
	cp, ok := checkpoints.m[k]
	return cp, ok
}

// checkpointOf is who sealed the latest compaction in a Responses
// request's input that magpie knows the writer of.
func checkpointOf(body []byte) (checkpointWriter, bool) {
	encs := sealedCompactions(body)
	if len(encs) == 0 {
		return checkpointWriter{}, false
	}
	checkpoints.Lock()
	defer checkpoints.Unlock()
	for _, enc := range slices.Backward(encs) {
		if cp, ok := checkpointFor(enc); ok {
			return cp, true
		}
	}
	return checkpointWriter{}, false
}

// wroteCheckpoint remembers that c sealed the compaction enc.
func wroteCheckpoint(enc string, c candidate) {
	now := time.Now()
	checkpoints.Lock()
	checkpointFor(enc) // the file's, read in first so the write keeps them
	checkpoints.m[checkpointKey(enc)] = checkpointWriter{Who: c.who(), Provider: c.p.ID, Model: c.model, At: now}
	checkpoints.Unlock()
	saveCheckpoints()
}

// saveCheckpoints writes the latest compactions' writers to the file,
// only for this user to read.
func saveCheckpoints() {
	checkpointsSaving.Lock()
	defer checkpointsSaving.Unlock()
	type kept struct {
		k string
		v checkpointWriter
	}
	var ks []kept
	checkpoints.Lock()
	for k, v := range checkpoints.m {
		if time.Since(v.At) <= checkpointKeep {
			ks = append(ks, kept{k, v})
		}
	}
	checkpoints.Unlock()
	slices.SortFunc(ks, func(a, b kept) int { return b.v.At.Compare(a.v.At) })
	ks = ks[:min(len(ks), checkpointsKept)]
	out := make(map[string]checkpointWriter, len(ks))
	for _, k := range ks {
		out[k.k] = k.v
	}
	b, err := json.Marshal(out)
	if err != nil {
		return
	}
	path := checkpointsPath()
	if os.MkdirAll(filepath.Dir(path), 0o700) != nil {
		return
	}
	target, err := edit.Target(path) // a symlink stays, its target written
	if err != nil {
		return
	}
	tmp := target + ".magpie-tmp"
	if os.WriteFile(tmp, b, 0o600) != nil {
		return
	}
	if steady.Rename(tmp, target) != nil {
		os.Remove(tmp)
		return
	}
	if fi, err := os.Stat(path); err == nil {
		checkpoints.Lock()
		// what this magpie wrote isn't read back as another's
		checkpoints.from, checkpoints.mod = path, fi.ModTime()
		checkpoints.Unlock()
	}
}

// checkpointReaders keeps of cands those of the key or account who, pl's
// order with them; left says one is left, if only held or capped.
func checkpointReaders(cands []candidate, pl planned, who string) (_ []candidate, _ planned, left bool) {
	return keepCandidates(cands, pl, func(c candidate) bool { return c.who() == who })
}

// compactionCarriers keeps of cands those that can be asked Codex's own
// compaction.
func (s *Server) compactionCarriers(cands []candidate, pl planned) (_ []candidate, _ planned, left bool) {
	return keepCandidates(cands, pl, func(c candidate) bool { return s.carriesCompaction(c.p, c.model) })
}

func keepCandidates(cands []candidate, pl planned, keep func(candidate) bool) ([]candidate, planned, bool) {
	var kept []candidate
	var order []Weighed
	for i, c := range cands {
		if keep(c) {
			kept = append(kept, c)
			if i < len(pl.order) {
				order = append(order, pl.order[i])
			}
		}
	}
	cands, pl.order = kept, order
	pl.held = slices.DeleteFunc(slices.Clone(pl.held), func(c candidate) bool { return !keep(c) })
	pl.capped = slices.DeleteFunc(slices.Clone(pl.capped), func(c candidate) bool { return !keep(c) })
	return cands, pl, len(cands) > 0 || len(pl.held) > 0 || len(pl.capped) > 0
}

// checkpointError is what Codex is told when the compaction a request
// carries was sealed by a key or account the request can't go to now.
func checkpointError(model string, cp checkpointWriter) string {
	return fmt.Sprintf("This conversation was compacted natively by %s, and only that key or account can read the compaction it sealed. The request for %s can't go to it now: it is switched off, removed, or no longer serves %s. magpie doesn't send the compaction to another, which would refuse it, nor drop it or summarise it in its place, which would lose the conversation's earlier context. Switch %s on again for %s to go on with this conversation, or start a new one.",
		cp.Who, model, model, cp.Who, model)
}

// carrierError is what Codex is told when its compaction_trigger, asked
// of a provider set to compact natively, has nowhere left that carries it.
func carrierError(model string) string {
	return fmt.Sprintf("Codex asked %s for its own compaction (native compaction), and none of the keys or accounts that serve %s now can be asked it on the Responses API. magpie doesn't turn it into a summary of its own unasked. Turn native compaction off for the provider to have magpie summarise the conversation instead.", model, model)
}

// compactionWatch reads, on the way past, the compaction a native
// compaction request's reply seals, for wroteCheckpoint.
type compactionWatch struct {
	w    http.ResponseWriter
	buf  bytes.Buffer
	over bool
}

// compactionWatchMax is how much of a reply is read for its compaction.
const compactionWatchMax = 64 << 20

func (c *compactionWatch) Header() http.Header         { return c.w.Header() }
func (c *compactionWatch) WriteHeader(code int)        { c.w.WriteHeader(code) }
func (c *compactionWatch) Unwrap() http.ResponseWriter { return c.w }

func (c *compactionWatch) Write(p []byte) (int, error) {
	if !c.over {
		if c.buf.Len()+len(p) > compactionWatchMax {
			c.over = true
			c.buf = bytes.Buffer{}
		} else {
			c.buf.Write(p)
		}
	}
	return c.w.Write(p)
}

func (c *compactionWatch) Flush() {
	if f, ok := c.w.(http.Flusher); ok {
		f.Flush()
	}
}

// sealed is the encrypted_content of the last compaction item the reply
// gave: in a stream's output_item.done or response.completed, or a JSON
// body's output.
func (c *compactionWatch) sealed() string {
	b := bytes.TrimSpace(c.buf.Bytes())
	if len(b) == 0 {
		return ""
	}
	found := ""
	fromItems := func(items []gjson.Result) {
		for _, it := range items {
			if slices.Contains(compactionKinds, it.Get("type").Str) {
				if enc := it.Get("encrypted_content").Str; enc != "" && !strings.HasPrefix(enc, magpieCompaction) {
					found = enc
				}
			}
		}
	}
	if b[0] == '{' {
		fromItems(gjson.GetBytes(b, "output").Array())
		return found
	}
	for line := range bytes.SplitSeq(b, []byte("\n")) {
		data, ok := bytes.CutPrefix(bytes.TrimSpace(line), []byte("data:"))
		if !ok || !bytes.Contains(data, []byte(`"compaction`)) {
			continue
		}
		ev := gjson.ParseBytes(bytes.TrimSpace(data))
		if it := ev.Get("item"); it.Exists() {
			fromItems([]gjson.Result{it})
		}
		fromItems(ev.Get("response.output").Array())
	}
	return found
}
