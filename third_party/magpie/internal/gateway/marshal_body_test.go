package gateway

import (
	"bytes"
	"encoding/json"
	"math"
	"runtime"
	"strings"
	"testing"
)

type ptrMarshaler struct{ n int }

func (p *ptrMarshaler) MarshalJSON() ([]byte, error) { return []byte(`"ptr"`), nil }

type valMarshaler []int

func (valMarshaler) MarshalJSON() ([]byte, error) { return []byte(`"whole"`), nil }

// marshalBody writes what json.Marshal writes, every kind of value a
// builder puts in a body included.
func TestMarshalBodyIsJSONMarshal(t *testing.T) {
	var nilList []any
	bodies := []map[string]any{
		{},
		{"model": "m", "stream": true, "max_tokens": 10, "temperature": 0.5},
		{"messages": []any{map[string]any{"role": "user", "content": "<a&b> \u2028 \u2029 \x01 é"}, map[string]any{"role": "assistant", "content": nil}}},
		{"messages": []map[string]any{{"role": "user"}, {"role": "system", "content": []any{"x", 1}}}},
		{"one": []any{"only"}, "none": []any{}, "nil": nilList, "nilv": nil},
		{"bytes": []byte("raw bytes"), "raw": json.RawMessage(`{"a":1}`), "raws": []json.RawMessage{json.RawMessage(`1`), json.RawMessage(`{"b":2}`)}},
		{"ptrs": []ptrMarshaler{{1}, {2}}, "whole": valMarshaler{1, 2, 3}, "strs": []string{"a", "<b>"}},
		{"<k>": 1, "a\u2028": 2, "z": []int{1, 2, 3}},
	}
	for i, b := range bodies {
		want, werr := json.Marshal(b)
		got, gerr := marshalBody(b)
		if (werr == nil) != (gerr == nil) || !bytes.Equal(got, want) {
			t.Errorf("body %d:\n got %s (%v)\nwant %s (%v)", i, got, gerr, want, werr)
		}
	}
	for _, bad := range []map[string]any{
		{"messages": []any{1, math.NaN()}},
		{"x": math.Inf(1)},
		{"ch": []any{"a", make(chan int)}},
	} {
		if _, err := marshalBody(bad); err == nil {
			t.Errorf("marshalBody(%v) took what json.Marshal refuses", bad)
		}
	}
}

// A 45 MB conversation is built without the heap growing past a body
// and its items: json.Marshal's doubling buffer took it from 45 to 316
// MiB (#4004).
func TestMarshalBodyAllocatesTwoBodies(t *testing.T) {
	big := strings.Repeat("tool output line\n", 1<<14)
	msgs := make([]any, 0, 170)
	for range 170 {
		msgs = append(msgs, map[string]any{"role": "tool", "tool_call_id": "c", "content": big})
	}
	body := map[string]any{"model": "m", "messages": msgs}
	peak := func(f func() []byte) (uint64, int) {
		runtime.GC()
		var m runtime.MemStats
		runtime.ReadMemStats(&m)
		before := m.TotalAlloc
		n := len(f())
		runtime.ReadMemStats(&m)
		return m.TotalAlloc - before, n
	}
	stdAlloc, n := peak(func() []byte { b, _ := json.Marshal(body); return b })
	ownAlloc, _ := peak(func() []byte { b, _ := marshalBody(body); return b })
	t.Logf("%d MiB body: json.Marshal allocated %d MiB, marshalBody %d MiB", n>>20, stdAlloc>>20, ownAlloc>>20)
	if ownAlloc > uint64(n)*5/2 {
		t.Errorf("marshalBody allocated %d MiB for a %d MiB body, want at most 2.5x", ownAlloc>>20, n>>20)
	}
}
