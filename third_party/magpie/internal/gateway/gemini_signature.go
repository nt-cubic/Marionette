package gateway

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"slices"
	"strconv"
	"strings"
)

// Gemini's thought signatures on tool calls (#687). Gemini 3 signs the
// first function call of each step it thinks through, and turns the next
// request away without the signature back on that call: 400 "Function call
// is missing a thought_signature in functionCall parts". Its
// OpenAI-compatible API gives the signature beside the call, in
// tool_calls[].extra_content.google.thought_signature, and takes it back
// there. A Chat client knows no such field (Pi's openai-completions drops
// it, as the AI SDK does), but every client sends a call's id back as it
// was given. So the signature rides in the id the client is given,
// <id>__ts<n>__<signature, base64url>, and comes off it on the way back:
// into extra_content for Gemini, and left out for any other upstream,
// which gets the id the call had.
//
// n is how many characters the signature has in the id. Some clients
// shorten an id they were given (one caps them at 40 characters, #1536),
// and what is left of a signature often still decodes as base64, so only
// n tells a shortened signature from a whole one. A signature that isn't
// whole is none: Gemini answers a partial one with 400 "Corrupted thought
// signature".
//
// A call with no signature to give back — made by another model, or an
// id a client shortened — goes to Gemini with the value Google documents
// for history it didn't sign, skip_thought_signature_validator, as the
// Code Assist path sends every call.
//
// A signature grows with the step's thinking, to several KB, and an id
// that long broke a client that indexes ids (#2411, liangxyz: LobeHub's
// PostgreSQL index takes 2704 bytes). So the signature itself is kept in
// magpie's folder (sigStore) and the id carries a short reference to it,
// <id>__tsk16__<key>. A reference magpie no longer has reads as no
// signature, as a shortened id does.

const (
	// sigMark begins the signature in an id: __ts<n>__ for the signature's
	// own bytes, __tsr<n>__ for one that isn't canonical base64, its text.
	// Ids magpie gave before #1536 have no n (__ts__, __tsr__); they are
	// still read, with nothing to check them by.
	sigMark = "__ts"
	sigRaw  = "r"
	sigKept = "k"
	sigEnd  = "__"
)

// signedID is a tool call's id carrying its thought signature.
func signedID(id, sig string) string {
	if sig == "" {
		return id
	}
	if id == "" {
		id = "call_" + newID()
	}
	if key := keepSignature(sig); key != "" {
		return id + sigMark + sigKept + strconv.Itoa(len(key)) + sigEnd + key
	}
	kind, enc := sigRaw, base64.RawURLEncoding.EncodeToString([]byte(sig))
	if b, err := base64.StdEncoding.DecodeString(sig); err == nil && base64.StdEncoding.EncodeToString(b) == sig {
		kind, enc = "", base64.RawURLEncoding.EncodeToString(b)
	}
	return id + sigMark + kind + strconv.Itoa(len(enc)) + sigEnd + enc
}

// unsignedID splits an id signedID made into the call's own id and its
// signature; any other id, or one whose signature was cut short, comes
// back as it is, with no signature.
func unsignedID(id string) (string, string) {
	for at := 1; at < len(id); {
		i := strings.Index(id[at:], sigMark)
		if i < 0 {
			break
		}
		i += at
		at = i + 1
		rest := id[i+len(sigMark):]
		raw := strings.HasPrefix(rest, sigRaw)
		if raw {
			rest = rest[len(sigRaw):]
		}
		kept := !raw && strings.HasPrefix(rest, sigKept)
		if kept {
			rest = rest[len(sigKept):]
		}
		d := 0
		for d < len(rest) && rest[d] >= '0' && rest[d] <= '9' {
			d++
		}
		enc, ok := strings.CutPrefix(rest[d:], sigEnd)
		if !ok {
			continue // not a mark: part of the call's own id
		}
		if kept && d == 0 {
			continue // a kept signature's mark always has its length
		}
		// the first mark decides: a signature cut short is none
		if d > 0 && rest[:d] != strconv.Itoa(len(enc)) {
			return id, ""
		}
		if kept {
			if sig := keptSignature(enc); sig != "" {
				return id[:i], sig
			}
			return id, ""
		}
		b, err := base64.RawURLEncoding.DecodeString(enc)
		if err != nil || len(b) == 0 {
			return id, ""
		}
		if raw {
			return id[:i], string(b)
		}
		return id[:i], base64.StdEncoding.EncodeToString(b)
	}
	return id, ""
}

// unsignCalls takes the signatures off a parsed request's call ids: a tool
// call keeps its own id and has the signature as its Signature, a result
// answers that id. Messages are copied before they change.
func unsignCalls(r *Request) {
	for i, m := range r.Messages {
		var parts []Part
		for j, p := range m.Parts {
			switch p.Kind {
			case ToolCall:
				id, sig := unsignedID(p.ID)
				if sig == "" {
					continue
				}
				p.ID = id
				if p.Signature == "" {
					p.Signature = sig
				}
			case ToolResult:
				id, sig := unsignedID(p.CallID)
				if sig == "" {
					continue
				}
				p.CallID = id
			default:
				continue
			}
			if parts == nil {
				parts = slices.Clone(m.Parts)
			}
			parts[j] = p
		}
		if parts != nil {
			r.Messages[i].Parts = parts
		}
	}
}

// googleSignature is the thought signature in a tool call's extra_content.
func googleSignature(extra json.RawMessage) string {
	if len(extra) == 0 {
		return ""
	}
	var v struct {
		Google struct {
			ThoughtSignature string `json:"thought_signature"`
		} `json:"google"`
	}
	json.Unmarshal(extra, &v)
	return v.Google.ThoughtSignature
}

// googleExtra is extra_content carrying a thought signature.
func googleExtra(sig string) map[string]any {
	return map[string]any{"google": map[string]any{"thought_signature": sig}}
}

// chatCallSignatures is a Chat request relayed as it is with the
// signatures off its call ids: for Gemini (gemini) each call has its own
// in extra_content, or skip_thought_signature_validator in a turn with
// none; for another upstream the ids are only the calls' own again.
func chatCallSignatures(body []byte, gemini bool) []byte {
	signed := bytes.Contains(body, []byte(sigMark))
	if !signed && !(gemini && bytes.Contains(body, []byte(`"tool_calls"`))) {
		return body
	}
	return editMessages(body, []string{"tool_calls", "tool_call_id"}, func(m map[string]json.RawMessage) bool {
		changed := false
		if raw, ok := m["tool_call_id"]; ok {
			var id string
			if json.Unmarshal(raw, &id) == nil {
				if own, sig := unsignedID(id); sig != "" {
					m["tool_call_id"], _ = marshalPlain(own)
					changed = true
				}
			}
		}
		var calls []map[string]json.RawMessage
		if json.Unmarshal(m["tool_calls"], &calls) != nil || len(calls) == 0 {
			return changed
		}
		sigs := make([]string, len(calls))
		signedStep := false
		for i, c := range calls {
			var id string
			json.Unmarshal(c["id"], &id)
			own, sig := unsignedID(id)
			if sig != "" {
				c["id"], _ = marshalPlain(own)
				changed = true
			}
			if s := googleSignature(c["extra_content"]); s != "" {
				sig = s
			}
			sigs[i] = sig
			signedStep = signedStep || sig != ""
		}
		if gemini {
			for i, c := range calls {
				sig := sigs[i]
				if sig == "" && signedStep {
					continue // a later call of a signed step: Gemini signs the first
				}
				if sig == "" {
					sig = skipSignature
				}
				if googleSignature(c["extra_content"]) == sig {
					continue
				}
				c["extra_content"], _ = marshalPlain(googleExtra(sig))
				changed = true
			}
		}
		if changed {
			m["tool_calls"], _ = marshalPlain(calls)
		}
		return changed
	})
}

// sigTidy gives, in a Chat reply from Gemini relayed as it is, each tool
// call that came with a thought signature an id carrying it (signedID).
// Lines, or a whole reply, pass as they came unless one has a signature.
type sigTidy struct {
	sse bool
	buf []byte
}

var thoughtSignatureField = []byte(`"thought_signature"`)

func (t *sigTidy) write(b []byte) []byte {
	t.buf = append(t.buf, b...)
	if !t.sse {
		return nil
	}
	i := bytes.LastIndexByte(t.buf, '\n')
	if i < 0 {
		return nil
	}
	out := t.lines(t.buf[:i+1])
	t.buf = append(t.buf[:0], t.buf[i+1:]...)
	return out
}

func (t *sigTidy) flush() []byte {
	b := t.buf
	t.buf = nil
	if !t.sse {
		if out, ok := signReply(b); ok {
			return out
		}
		return b
	}
	return t.lines(b)
}

func (t *sigTidy) lines(b []byte) []byte {
	if !bytes.Contains(b, thoughtSignatureField) {
		return append([]byte(nil), b...)
	}
	var out []byte
	for len(b) > 0 {
		line := b
		if i := bytes.IndexByte(b, '\n'); i >= 0 {
			line, b = b[:i+1], b[i+1:]
		} else {
			b = nil
		}
		body := bytes.TrimRight(line, "\r\n")
		data, ok := bytes.CutPrefix(body, []byte("data:"))
		if !ok || !bytes.Contains(data, thoughtSignatureField) {
			out = append(out, line...)
			continue
		}
		nb, ok := signReply(data)
		if !ok {
			out = append(out, line...)
			continue
		}
		out = append(append(append(out, "data: "...), nb...), line[len(body):]...)
	}
	return out
}

// signReply is a chunk, or a whole reply, with the signed calls' ids
// carrying their signatures, and whether there was one.
func signReply(data []byte) ([]byte, bool) {
	if !bytes.Contains(data, thoughtSignatureField) {
		return nil, false
	}
	var reply map[string]json.RawMessage
	if json.Unmarshal(data, &reply) != nil {
		return nil, false
	}
	var choices []map[string]json.RawMessage
	if json.Unmarshal(reply["choices"], &choices) != nil {
		return nil, false
	}
	changed := false
	for _, c := range choices {
		for _, k := range []string{"delta", "message"} {
			var msg map[string]json.RawMessage
			if json.Unmarshal(c[k], &msg) != nil {
				continue
			}
			var calls []map[string]json.RawMessage
			if json.Unmarshal(msg["tool_calls"], &calls) != nil {
				continue
			}
			signed := false
			for _, call := range calls {
				sig := googleSignature(call["extra_content"])
				if sig == "" {
					continue
				}
				var id string
				json.Unmarshal(call["id"], &id)
				if _, had := unsignedID(id); had != "" {
					continue
				}
				call["id"], _ = marshalPlain(signedID(id, sig))
				signed = true
			}
			if signed {
				msg["tool_calls"], _ = marshalPlain(calls)
				c[k], _ = marshalPlain(msg)
				changed = true
			}
		}
	}
	if !changed {
		return nil, false
	}
	reply["choices"], _ = marshalPlain(choices)
	out, err := marshalPlain(reply)
	if err != nil {
		return nil, false
	}
	return out, true
}
