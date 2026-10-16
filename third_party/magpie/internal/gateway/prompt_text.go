package gateway

// The text behind the context window card (masaka on Discord): click a
// part of the prompt, and see what the request sent there — the agent's
// system prompt, each tool's definition, an instruction file, a turn.
//
// The routing history keeps a prompt's sizes and names only. The text is
// read again from the request's body, which the trace holds for the last
// few requests: in memory only, never written to disk or sent anywhere,
// and masked as the vendor got it (redacted), the same body the Recent
// calls keep. When magpie quits, or a request is older than the last
// promptBodies, its text is gone.

import (
	"bytes"
	"encoding/json"
	"slices"
	"strconv"
	"strings"

	"github.com/yetone/magpie/internal/provider"
)

const (
	// promptBodies is how many requests' bodies the trace keeps for
	// their text
	promptBodies = 10
	// promptBodyMax is the largest body kept; a larger one's text isn't
	promptBodyMax = 32 << 20
	// promptBodiesMax is what the kept bodies take at most together, the
	// oldest going first
	promptBodiesMax = 64 << 20
)

// keptBody is a request's body, kept for its prompt's text.
type keptBody struct {
	id   int64
	from provider.Protocol
	body []byte
}

// keepBody keeps a request's masked body for its prompt's text, dropping
// the oldest past promptBodies or promptBodiesMax.
func (t *trace) keepBody(id int64, from provider.Protocol, body []byte) {
	if len(body) == 0 || len(body) > promptBodyMax {
		return
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	t.bodies = append(t.bodies, keptBody{id: id, from: from, body: body})
	total := 0
	for _, b := range t.bodies {
		total += len(b.body)
	}
	for len(t.bodies) > promptBodies || total > promptBodiesMax {
		total -= len(t.bodies[0].body)
		t.bodies[0] = keptBody{}
		t.bodies = t.bodies[1:]
	}
}

// keptBodyOf is a route's kept body; false when it isn't kept.
func (t *trace) keptBodyOf(id int64) (keptBody, bool) {
	t.mu.Lock()
	defer t.mu.Unlock()
	for _, b := range t.bodies {
		if b.id == id {
			return b, true
		}
	}
	return keptBody{}, false
}

// PromptEntry is one item of a prompt's part with its text: the card's
// item, unfolded — every turn and every tool, not the latest or largest
// promptItems.
type PromptEntry struct {
	Name   string        `json:"name"`
	Tag    string        `json:"tag,omitempty"`
	N      int           `json:"n,omitempty"`
	Tokens int           `json:"tokens"`
	Pieces []PromptPiece `json:"pieces,omitempty"`
}

// PromptText is a part of a request's prompt, item by item, with the text
// when an item is asked for.
type PromptText struct {
	ID    int64         `json:"id"`
	Kind  string        `json:"kind"`
	Items []PromptEntry `json:"items"`
	// Kept is how many requests' text is kept, for the GUI to say so
	Kept int `json:"kept"`
}

// PromptKept is how many requests' text the trace keeps.
const PromptKept = promptBodies

// PromptText is the part kind of request id's prompt, item by item, in the
// card's order: the turns oldest first, the rest largest first. Each
// entry's text is there when withText; false when the request's body is no
// longer kept, or its prompt can't be read.
func (s *Server) PromptText(id int64, kind string, withText bool) (PromptText, bool) {
	kb, ok := s.trace.keptBodyOf(id)
	if !ok || !slices.Contains(partOrder, kind) {
		return PromptText{}, false
	}
	b, ok := promptPieces(kb.from, kb.body)
	if !ok {
		return PromptText{}, false
	}
	// the sizes as the card shows them: scaled to what the vendor counted
	est := 0
	for _, m := range b.parts {
		for _, it := range m {
			est += it.Tokens
		}
	}
	scale := 1.0
	s.trace.mu.Lock()
	for _, r := range s.trace.routes {
		if r.ID == id && r.Prompt != nil && r.Prompt.Tokens > 0 && est > 0 {
			scale = float64(r.Prompt.Tokens) / float64(est)
		}
	}
	s.trace.mu.Unlock()
	out := PromptText{ID: id, Kind: kind, Items: []PromptEntry{}, Kept: promptBodies}
	for key, it := range b.parts[kind] {
		e := PromptEntry{Name: it.Name, Tag: it.Tag, Tokens: int(float64(it.Tokens)*scale + 0.5)}
		if it.N > 1 && (it.Tag == "mcp" || it.Tag == "namespace" || it.Tag == "result") {
			e.N = it.N
		}
		if withText {
			for _, pc := range b.pieces[kind+"\x00"+key] {
				e.Pieces = append(e.Pieces, pc.read())
			}
		}
		out.Items = append(out.Items, e)
	}
	if kind == PartChat {
		slices.SortFunc(out.Items, func(a, b PromptEntry) int {
			x, errX := strconv.Atoi(a.Name)
			y, errY := strconv.Atoi(b.Name)
			if errX != nil || errY != nil { // "compacted" goes first
				return boolInt(errY != nil) - boolInt(errX != nil)
			}
			return x - y
		})
	} else {
		slices.SortFunc(out.Items, func(a, b PromptEntry) int {
			if a.Tokens != b.Tokens {
				return b.Tokens - a.Tokens
			}
			return strings.Compare(a.Name, b.Name)
		})
	}
	return out, true
}

func boolInt(b bool) int {
	if b {
		return 1
	}
	return 0
}

// read is the piece with its JSON laid out to be read: indented, or a
// string's text when the JSON is a string.
func (pc PromptPiece) read() PromptPiece {
	if pc.raw == nil {
		return pc
	}
	raw := bytes.TrimSpace(pc.raw)
	pc.raw = nil
	var s string
	if json.Unmarshal(raw, &s) == nil {
		// a call's arguments sent as a JSON string, as chat completions
		// does: what the string holds, laid out when it is JSON too
		var out bytes.Buffer
		if json.Indent(&out, []byte(s), "", "  ") == nil {
			pc.Text = out.String()
		} else {
			pc.Text = s
		}
		return pc
	}
	var out bytes.Buffer
	if json.Indent(&out, raw, "", "  ") == nil {
		pc.Text = out.String()
	} else {
		pc.Text = string(raw)
	}
	return pc
}

// promptPieces reads a request's prompt keeping each item's text.
func promptPieces(from provider.Protocol, body []byte) (b *promptBuilder, ok bool) {
	defer func() {
		if recover() != nil {
			b, ok = nil, false
		}
	}()
	b = newPromptBuilder()
	b.keep, b.pieces = true, map[string][]PromptPiece{}
	switch from {
	case provider.Anthropic:
		ok = b.anthropic(body)
	case provider.Responses:
		ok = b.responses(body)
	case provider.Chat:
		ok = b.chatCompletions(body)
	case provider.Gemini:
		ok = b.gemini(body)
	}
	return b, ok
}
