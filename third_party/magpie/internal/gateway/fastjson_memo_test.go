package gateway

import (
	"bytes"
	"encoding/json"
	"strings"
	"sync/atomic"
	"testing"
)

// memoBody is a request body past validMemoMin, unique to the test.
func memoBody(tag string) []byte {
	return []byte(`{"model":"` + tag + `","messages":[{"role":"user","content":"` +
		strings.Repeat("a long prompt ", 6000) + `"}]}`)
}

// countScans counts jsonValid's byte-by-byte checks of bodies equal to b.
func countScans(t *testing.T, b []byte) *atomic.Int32 {
	var n atomic.Int32
	f := func(data []byte) {
		if bytes.Equal(data, b) {
			n.Add(1)
		}
	}
	validScanned.Store(&f)
	t.Cleanup(func() { validScanned.Store(nil) })
	return &n
}

// A long body decoded by each reader of a request is checked for valid
// JSON once, not by every decodeJSON: at 2 MB each check cost ~2.5ms, and
// a request's body went through a dozen of them.
func TestDecodeJSONChecksALongBodyOnce(t *testing.T) {
	body := memoBody(t.Name())
	scans := countScans(t, body)
	for range 6 {
		var v struct {
			Model string `json:"model"`
		}
		if err := decodeJSON(body, &v); err != nil || v.Model != t.Name() {
			t.Fatalf("decode: %v %q", err, v.Model)
		}
	}
	if n := scans.Load(); n != 1 {
		t.Fatalf("a %d-byte body was checked %d times over six decodes, want 1", len(body), n)
	}
}

// A long body's top level is split at its members once, not by each decode
// of it: each reader of a request (the model, the stream flag, the
// messages, the tools…) walked every byte of the conversation again to
// find its member. What each is given is what encoding/json gives it.
func TestDecodeJSONSplitsALongBodyOnce(t *testing.T) {
	body := []byte(`{"model":"` + t.Name() + `","stream":true,"max_tokens":9,"messages":[{"role":"user","content":"` +
		strings.Repeat("a long prompt ", 6000) + `"}],"metadata":{"user_id":"u"}}`)
	var n atomic.Int32
	f := func(data []byte) {
		if bytes.Equal(data, body) {
			n.Add(1)
		}
	}
	topSplit.Store(&f)
	t.Cleanup(func() { topSplit.Store(nil) })
	type msgs struct {
		Messages []struct {
			Role    string `json:"role"`
			Content string `json:"content"`
		} `json:"messages"`
	}
	for range 4 {
		var a struct {
			Model  string `json:"model"`
			Stream bool   `json:"stream"`
		}
		var b msgs
		var c *msgs
		var d map[string]json.RawMessage
		var e struct {
			Metadata struct {
				UserID string `json:"user_id"`
			} `json:"metadata"`
			Max int `json:"max_tokens"`
		}
		for _, dst := range []any{&a, &b, &c, &d, &e} {
			if err := decodeJSON(body, dst); err != nil {
				t.Fatal(err)
			}
		}
		wa, wb, we := a, msgs{}, e
		wa.Model, wa.Stream, we.Metadata.UserID, we.Max = "", false, "", 0
		var wd map[string]json.RawMessage
		json.Unmarshal(body, &wa)
		json.Unmarshal(body, &wb)
		json.Unmarshal(body, &wd)
		json.Unmarshal(body, &we)
		if a != wa || len(b.Messages) != 1 || b.Messages[0] != wb.Messages[0] || c == nil || len(c.Messages) != 1 ||
			len(d) != len(wd) || string(d["messages"]) != string(wd["messages"]) || e != we {
			t.Fatalf("decoded otherwise than encoding/json: %+v %+v %d %v", a, e, len(d), c == nil)
		}
	}
	if got := n.Load(); got != 1 {
		t.Fatalf("a %d-byte body's top level was split %d times over 20 decodes, want 1", len(body), got)
	}
}

// What is remembered is a valid body: an invalid one, or a valid one
// changed by a byte at the same length, is checked again and refused.
func TestDecodeJSONMemoKeepsRefusing(t *testing.T) {
	body := memoBody(t.Name())
	var v map[string]any
	if err := decodeJSON(body, &v); err != nil {
		t.Fatal(err)
	}
	bad := bytes.Clone(body)
	bad[len(bad)-1] = '!'
	for range 3 {
		var w map[string]any
		if err := decodeJSON(bad, &w); err == nil {
			t.Fatal("a body with a broken end decoded")
		}
	}
	trunc := body[:len(body)-1]
	var w map[string]any
	if err := decodeJSON(trunc, &w); err == nil {
		t.Fatal("a truncated body decoded")
	}
	if !jsonValid(body) || jsonValid(bad) {
		t.Fatal("jsonValid's answer changed")
	}
}
