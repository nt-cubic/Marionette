package sessions

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/klauspost/compress/zstd"

	"github.com/yetone/magpie/internal/codexsummary"
	"github.com/yetone/magpie/internal/filelock"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/steady"
)

// A Codex conversation magpie compacted keeps magpie's compaction item in
// its rollout, and Codex sends it back with every later turn: to OpenAI
// itself, once magpie is quit or disconnected, it is refused
// (invalid_encrypted_content) and the conversation can't go on (LukeMa on
// X). PortCodexRollouts writes, in each rollout that has one, the message
// it stands for in its place (codexsummary.Message): what Codex's own
// local compaction keeps, and what the gateway sends for it anyway, so a
// rollout so written reads the same through magpie and goes on without it.
//
// Only magpie's compaction items change, in whichever line holds them (a
// response_item, a compacted line's replacement_history); every other
// line keeps its bytes, and the file its line count, mode and time. A
// rollout is changed as Codex publishes a compressed one
// (codex-rs/rollout/src/writer_lock.rs, try_acquire_for_publication):
// under its home's coordination lock, and only when no Codex holds the
// thread's writer lock, so never under a Codex that has the thread open.
// One that does is left for a later pass. A Codex home without
// thread-writer-locks (a Codex from before them) is left alone, as there
// is no telling whether a Codex is writing there.

// codexPortState is what PortCodexRollouts keeps between passes, per
// Codex home: when its last whole pass began, and the rollouts it had to
// leave for a Codex that held them.
type codexPortState struct {
	Homes map[string]codexPortHome `json:"homes"`
}

type codexPortHome struct {
	Since   time.Time `json:"since"`
	Pending []string  `json:"pending,omitempty"`
}

// codexPortFirst is the first day magpie wrote a compaction item of its
// own (c3931c91d, 2026-09-25): no rollout last written before then has one.
var codexPortFirst = time.Date(2026, 9, 24, 0, 0, 0, 0, time.UTC)

// codexPortSlack is how far before a pass began the next one looks again:
// a file's time can be coarser than the clock.
const codexPortSlack = 2 * time.Minute

func codexPortPath() string { return filepath.Join(settings.Dir(), "codex-portable.json") }

// CodexPort is what a pass did.
type CodexPort struct {
	Rewritten []string // rollouts written portable
	Held      []string // rollouts with magpie's item a Codex holds, left for a later pass
	Done      bool     // every rollout due was looked at
}

// PortCodexRollouts makes the rollouts of the Codex home home portable,
// those written since the last whole pass and those it left before, until
// ctx ends. The first pass reads every rollout written since magpie began
// to compact; later ones only what changed since.
func PortCodexRollouts(ctx context.Context, home string) (CodexPort, error) {
	var res CodexPort
	locks := filepath.Join(home, "thread-writer-locks")
	if fi, err := os.Stat(locks); err != nil || !fi.IsDir() {
		return res, nil
	}
	// one pass at a time across magpies
	unlock, held := filelock.Lock(codexPortPath()+".lock", 0)
	if !held {
		return res, nil
	}
	defer unlock()
	var st codexPortState
	if b, err := os.ReadFile(codexPortPath()); err == nil {
		if json.Unmarshal(b, &st) != nil {
			st = codexPortState{} // unreadable: a whole pass again, which only costs time
		}
	} else if !errors.Is(err, fs.ErrNotExist) {
		return res, err
	}
	if st.Homes == nil {
		st.Homes = map[string]codexPortHome{}
	}
	key := filepath.Clean(home)
	was := st.Homes[key]
	since := codexPortFirst
	if !was.Since.IsZero() {
		since = was.Since.Add(-codexPortSlack)
	}
	start := time.Now()
	var due []string
	seen := map[string]bool{}
	add := func(p string) {
		if !seen[p] {
			seen[p] = true
			due = append(due, p)
		}
	}
	for _, p := range was.Pending {
		for _, q := range []string{p, rolloutTwin(p)} { // Codex may have compressed it since
			if fi, err := os.Stat(q); err == nil && fi.Mode().IsRegular() {
				add(q)
			}
		}
	}
	for _, folder := range []string{"sessions", "archived_sessions"} {
		filepath.WalkDir(filepath.Join(home, folder), func(p string, d fs.DirEntry, err error) error {
			if err != nil || d.IsDir() || codexThreadOf(d.Name()) == "" {
				return nil
			}
			if fi, err := d.Info(); err == nil && fi.Mode().IsRegular() && !fi.ModTime().Before(since) {
				add(p)
			}
			return nil
		})
	}
	res.Done = true
	var pending []string
	for i, p := range due {
		if ctx.Err() != nil {
			res.Done = false
			pending = append(pending, due[i:]...)
			break
		}
		changed, err := portRollout(ctx, home, p)
		switch {
		case errors.Is(err, errRolloutHeld):
			res.Held = append(res.Held, p)
			pending = append(pending, p)
		case err != nil:
			if ctx.Err() != nil {
				res.Done = false
				pending = append(pending, due[i:]...)
				break
			}
			log.Printf("codex rollout %s: %v", p, err)
		case changed:
			res.Rewritten = append(res.Rewritten, p)
		}
		if !res.Done {
			break
		}
	}
	now := codexPortHome{Since: was.Since, Pending: pending}
	if res.Done {
		now.Since = start
	} else {
		now.Pending = append(now.Pending, was.Pending...)
		now.Pending = uniq(now.Pending)
	}
	st.Homes[key] = now
	b, _ := json.MarshalIndent(st, "", "  ")
	if err := os.MkdirAll(settings.Dir(), 0o700); err != nil {
		return res, err
	}
	return res, steady.WriteFile(codexPortPath(), b, 0o600)
}

func uniq(list []string) []string {
	var out []string
	for _, s := range list {
		if !contains(out, s) {
			out = append(out, s)
		}
	}
	return out
}

// errRolloutHeld is a rollout a Codex has open: its thread's writer lock
// is held, or the file changed while it was read.
var errRolloutHeld = errors.New("held by a Codex")

// codexRolloutName is Codex's name for a rollout (rollout_file_name.rs):
// its time, its thread and, for a later rollout of the thread, its own id.
var codexRolloutName = regexp.MustCompile(`^rollout-\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})(?:_[0-9a-fA-F-]{36})?\.jsonl(?:\.zst)?$`)

// codexThreadOf is the thread a rollout's file name says it is of; "" for
// a file that isn't one.
func codexThreadOf(name string) string {
	m := codexRolloutName.FindStringSubmatch(name)
	if m == nil {
		return ""
	}
	return strings.ToLower(m[1])
}

var magpieMark = []byte(codexsummary.Mark)

// portRollout writes the rollout at path portable, if it has magpie's
// compaction item, and says whether it did.
func portRollout(ctx context.Context, home, path string) (bool, error) {
	thread := codexThreadOf(filepath.Base(path))
	if thread == "" {
		return false, nil
	}
	fi, err := os.Stat(path)
	if err != nil {
		return false, err
	}
	in, err := openLines(path)
	if err != nil {
		return false, err
	}
	defer in.Close()
	tmp, err := os.CreateTemp(filepath.Dir(path), ".magpie-"+filepath.Base(path)+"-*")
	if err != nil {
		return false, err
	}
	done := false
	defer func() {
		if !done {
			tmp.Close()
			os.Remove(tmp.Name())
		}
	}()
	var w io.Writer = tmp
	var z *zstd.Encoder
	if packed(path) {
		if z, err = zstd.NewWriter(tmp); err != nil {
			return false, err
		}
		w = z
	}
	bw := bufio.NewWriterSize(w, 64<<10)
	br := bufio.NewReaderSize(in, 64<<10)
	changed := false
	for n := 0; ; n++ {
		if n%4096 == 0 && ctx.Err() != nil {
			return false, ctx.Err()
		}
		line, err := br.ReadBytes('\n')
		if len(line) > 0 {
			out, ok := portLine(line)
			changed = changed || ok
			if _, werr := bw.Write(out); werr != nil {
				return false, werr
			}
		}
		if err == io.EOF {
			break
		}
		if err != nil {
			return false, err
		}
	}
	if !changed {
		return false, nil
	}
	if err := bw.Flush(); err != nil {
		return false, err
	}
	if z != nil {
		if err := z.Close(); err != nil {
			return false, err
		}
	}
	if err := tmp.Chmod(fi.Mode().Perm()); err != nil {
		return false, err
	}
	if err := steady.Sync(tmp); err != nil {
		return false, err
	}
	if err := tmp.Close(); err != nil {
		return false, err
	}
	in.Close()
	os.Chtimes(tmp.Name(), fi.ModTime(), fi.ModTime())

	// published as Codex publishes a compressed rollout: under the
	// coordination lock every Codex writer takes before its thread's
	// lock, with that lock free (or never made)
	locks := filepath.Join(home, "thread-writer-locks")
	unlock, held := filelock.Lock(filepath.Join(locks, ".coordination.lock"), filelock.Wait)
	if !held {
		return false, errRolloutHeld
	}
	defer unlock()
	if lf, err := os.OpenFile(filepath.Join(locks, thread+".lock"), os.O_RDWR, 0); err == nil {
		got, err := filelock.TryLock(lf)
		if err != nil || !got {
			lf.Close()
			return false, errRolloutHeld
		}
		defer func() { filelock.Unlock(lf); lf.Close() }()
	} else if !errors.Is(err, fs.ErrNotExist) {
		return false, errRolloutHeld
	}
	// written to since it was read: a Codex had it after all
	if now, err := os.Stat(path); err != nil || now.Size() != fi.Size() || !now.ModTime().Equal(fi.ModTime()) {
		return false, errRolloutHeld
	}
	if err := steady.Rename(tmp.Name(), path); err != nil {
		return false, err
	}
	done = true
	return true, nil
}

// portLine is a rollout line with each of magpie's compaction items in it
// written as the message it stands for; any other line as it is, byte for
// byte. One of magpie's items whose summary can't be read is left out of
// a list (and its entry of replacement_history_metadata with it), as the
// gateway leaves it out; a line that is one alone stays as it is, as
// leaving a line out would move every line after it.
func portLine(line []byte) ([]byte, bool) {
	if !bytes.Contains(line, magpieMark) {
		return line, false
	}
	body := bytes.TrimRight(line, "\r\n")
	end := line[len(body):]
	d := json.NewDecoder(bytes.NewReader(body))
	d.UseNumber()
	var v any
	if d.Decode(&v) != nil || d.More() {
		return line, false
	}
	v, changed, _ := portValue(v)
	if !changed {
		return line, false
	}
	var buf bytes.Buffer
	e := json.NewEncoder(&buf)
	e.SetEscapeHTML(false)
	if e.Encode(v) != nil {
		return line, false
	}
	return append(bytes.TrimRight(buf.Bytes(), "\n"), end...), true
}

// portValue is v with magpie's compaction items in it written as their
// messages; drop says v is one of magpie's that can't be read.
func portValue(v any) (_ any, changed, drop bool) {
	switch x := v.(type) {
	case map[string]any:
		if t, _ := x["type"].(string); t == "compaction" || t == "compaction_summary" {
			enc, _ := x["encrypted_content"].(string)
			if msg, ours := codexsummary.Message(enc); ours {
				if msg == nil {
					return x, false, true
				}
				return msg, true, false
			}
			return x, false, false
		}
		for k, c := range x {
			list, isList := c.([]any)
			if !isList {
				if nv, ch, dr := portValue(c); ch && !dr {
					x[k], changed = nv, true
				}
				continue
			}
			// Codex reads replacement_history_metadata beside
			// replacement_history, one entry for each item: an item is
			// left out only with its entry, and where the two don't line
			// up (Codex refuses that line anyway) none is
			meta, hasMeta := x[k+"_metadata"]
			metaList, _ := meta.([]any)
			drop := !hasMeta || meta == nil || len(metaList) == len(list)
			kept, gone, ch := portList(list, drop)
			if !ch {
				continue
			}
			x[k], changed = kept, true
			if len(gone) > 0 && metaList != nil {
				m := []any{}
				for i, e := range metaList {
					if !gone[i] {
						m = append(m, e)
					}
				}
				x[k+"_metadata"] = m
			}
		}
		return x, changed, false
	case []any:
		kept, _, ch := portList(x, true)
		return kept, ch, false
	}
	return v, false, false
}

func portList(list []any, drop bool) (_ []any, gone map[int]bool, changed bool) {
	out := make([]any, 0, len(list))
	for i, e := range list {
		nv, ch, dr := portValue(e)
		if dr && drop {
			if gone == nil {
				gone = map[int]bool{}
			}
			gone[i], changed = true, true
			continue
		}
		changed = changed || ch
		out = append(out, nv)
	}
	return out, gone, changed
}
