package sessions

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"math"
	"os"
	"strings"
	"time"
)

// Pi's and omp's calls (#1951, Elenoxe: Pi in WSL on its own provider
// never reached Usage). Read from the session files Sessions reads
// (pi.go, omp.go), as Pi 1.1.0 and omp 18.9.1 wrote them in a sandbox
// against a relay of their own: each model response is an assistant
// message, written as it ended, naming the provider and model that
// answered and its usage (input without the cache, output with the
// reasoning); Pi adds durationMs, omp duration and ttft:
//
//	{"type":"message","id":"36b3bbef","parentId":"807675bc",
//	 "timestamp":"2026-10-13T07:47:34.522Z","message":{"role":"assistant",
//	 "content":[…],"api":"openai-completions","provider":"relay",
//	 "model":"deepseek-v4","usage":{"input":201,"output":30,"cacheRead":1000,
//	 "cacheWrite":0,"reasoning":0,"totalTokens":1231,…},"stopReason":"stop",
//	 "timestamp":1791877653894,"responseId":"chatcmpl-fake1","durationMs":628}}
//
// The calls outside the conversation are counted too, as Sessions counts
// them: "usage" and omp's "model_usage" entries, each naming its provider
// and model, and a compaction's, a branch summary's or a tool's result's
// usage, on the model and provider in use. A call that failed is written
// with no usage, and is none.
//
// A call made through magpie's own entry (provider "magpie", which
// internal/agent's pi.go and omp.go write, "dial" before the rename) is the
// gateway's to count: it logged the call itself, with the provider and
// account that answered.
//
// A fork (pi --fork, /fork) starts with a copy of its parent's entries, ids
// and times kept: they are the parent's calls, left out of the fork as
// Sessions leaves them (written before its header), and a call is keyed by
// its entry's id and time, so Calls counts one twice in no case.

// piCallFiles are Pi's and omp's session files, here and in WSL.
func piCallFiles() []file {
	out := append(piFiles(), ompFiles()...)
	return append(out, wslFiles("pi", "omp")...)
}

// piGateway is whether a provider is magpie's own entry.
func piGateway(provider string) bool { return provider == "magpie" || provider == "dial" }

type piCallLine struct {
	Type      string   `json:"type"`
	ID        string   `json:"id"`
	Timestamp string   `json:"timestamp"`
	Cwd       string   `json:"cwd"` // session
	Parent    string   `json:"parentSession"`
	Provider  string   `json:"provider"` // model_change, usage, model_usage
	ModelID   string   `json:"modelId"`  // model_change
	Model     string   `json:"model"`    // usage, model_usage; omp's model_change, as provider/model
	Usage     *piUsage `json:"usage"`
	Message   *struct {
		Role       string   `json:"role"`
		Provider   string   `json:"provider"`
		Model      string   `json:"model"`
		Usage      *piUsage `json:"usage"`
		DurationMs float64  `json:"durationMs"` // Pi's
		Duration   float64  `json:"duration"`   // omp's, in milliseconds
		TTFT       float64  `json:"ttft"`       // omp's
	} `json:"message"`
}

// readPiCalls reads one Pi or omp session file whole.
func readPiCalls(f file) (*callFile, bool) {
	r, err := os.Open(f.path)
	if err != nil {
		return &callFile{Path: f.path, Agent: f.agent}, false
	}
	defer r.Close()
	st := &callFile{Agent: f.agent, Path: f.path, Session: strings.TrimPrefix(f.key, f.agent+":")}
	var cwd, provider, model string
	var header bool
	var since time.Time // a fork's: what was written before it is its parent's
	br := bufio.NewReaderSize(r, 1<<20)
	for {
		b, err := br.ReadBytes('\n')
		if err != nil && (!errors.Is(err, io.EOF) || len(bytes.TrimSpace(b)) == 0) {
			break
		}
		b = bytes.TrimSpace(b)
		if len(b) > 0 && len(b) <= maxLine && (bytes.Contains(b, piUsed) || bytes.Contains(b, piHeader) || bytes.Contains(b, piModel)) {
			var l piCallLine
			if json.Unmarshal(b, &l) == nil {
				switch l.Type {
				case "session":
					if !header {
						header, cwd = true, l.Cwd
						if l.Parent != "" {
							since, _ = time.Parse(time.RFC3339Nano, l.Timestamp)
						}
					}
				case "model_change":
					p, id := l.Provider, l.ModelID
					if id == "" {
						p, id, _ = strings.Cut(l.Model, "/")
					}
					if id != "" {
						provider, model = p, id
					}
				case "message":
					if m := l.Message; m != nil && m.Role == "assistant" && m.Model != "" {
						provider, model = m.Provider, m.Model
					}
				}
				if c, ok := piCall(st, f.agent, l, provider, model); ok && !c.Time.Before(since) {
					c.Cwd = st.str(cwd)
					st.Calls = append(st.Calls, c)
				}
			}
		}
		if err != nil {
			break
		}
	}
	return st, true
}

// piCall is an entry as a call, on the provider and model in use for the
// usage that names none.
func piCall(st *callFile, agent string, l piCallLine, provider, model string) (Call, bool) {
	var u *piUsage
	var millis, ttft int64
	switch l.Type {
	case "message":
		m := l.Message
		if m == nil {
			return Call{}, false
		}
		u = m.Usage
		if m.Role == "assistant" {
			provider, model = m.Provider, m.Model
			millis = int64(math.Round(max(m.DurationMs, m.Duration)))
			ttft = int64(math.Round(m.TTFT))
		}
	case "usage", "model_usage":
		u, provider, model = l.Usage, l.Provider, l.Model
	case "compaction", "branch_summary":
		u = l.Usage
	default:
		return Call{}, false
	}
	if u == nil || model == "" || piGateway(provider) {
		return Call{}, false
	}
	if u.Input < 0 || u.Output < 0 || u.CacheRead < 0 || u.CacheWrite < 0 {
		return Call{}, false
	}
	// the entry's time: when the call ended, as a call's Time is
	at, _ := time.Parse(time.RFC3339Nano, l.Timestamp)
	if at.IsZero() {
		return Call{}, false
	}
	c := Call{
		Agent:    agent,
		Time:     at,
		Session:  st.Session,
		Model:    st.str(model),
		Upstream: st.str(provider),
		Tokens:   u.tokens(),
		Millis:   max(millis, 0),
		TTFT:     max(ttft, 0),
	}
	if c.Tokens.zero() {
		return Call{}, false
	}
	if l.ID != "" {
		c.Msg = st.str(agent + ":" + l.ID + ":" + l.Timestamp)
	}
	return c, true
}
