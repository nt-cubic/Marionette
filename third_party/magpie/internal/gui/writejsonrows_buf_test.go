package gui

import (
	"bytes"
	"encoding/json"
	"net/http/httptest"
	"testing"
)

type rowsBufRow struct {
	Name string  `json:"name"`
	Note string  `json:"note,omitempty"`
	N    float64 `json:"n"`
}

// writeJSONRows writes the rows straight into the answer, and the answer is
// byte for byte what Marshal of each row, joined, gave: the page's delta
// and its tag both rest on those bytes.
func TestWriteJSONRowsWritesWhatMarshalDid(t *testing.T) {
	rows := []rowsBufRow{
		{Name: "a<b>&c", Note: "\u2028 línea", N: 1.5},
		{Name: "", N: 0},
		{Name: "引号\"", Note: "x\ny", N: 1e21},
	}
	old := func(rows []rowsBufRow, rest any) []byte {
		var items [][]byte
		for _, r := range rows {
			b, err := json.Marshal(r)
			if err != nil {
				t.Fatal(err)
			}
			items = append(items, b)
		}
		tail, _ := json.Marshal(rest)
		var b bytes.Buffer
		b.WriteString(`{"rows":[`)
		b.Write(bytes.Join(items, []byte(",")))
		b.WriteByte(']')
		if len(tail) > 2 {
			b.WriteByte(',')
			b.Write(tail[1:])
		} else {
			b.WriteByte('}')
		}
		b.WriteByte('\n')
		return b.Bytes()
	}
	for _, rest := range []any{struct{}{}, map[string]any{"total": 3, "at": "<now>"}} {
		for n := 0; n <= len(rows); n++ {
			// twice, so the second is written into a buffer sized from the first
			for range 2 {
				w := httptest.NewRecorder()
				writeJSONRows(w, httptest.NewRequest("GET", "/x", nil), "rows", rows[:n], rest)
				if want := old(rows[:n], rest); !bytes.Equal(w.Body.Bytes(), want) {
					t.Fatalf("%d rows, rest %v:\n got %q\nwant %q", n, rest, w.Body.Bytes(), want)
				}
			}
		}
	}

	// a row that changed goes in a delta as its own bytes, and the rest as runs
	w := httptest.NewRecorder()
	writeJSONRows(w, httptest.NewRequest("GET", "/x", nil), "rows", rows, struct{}{})
	tag := w.Header().Get("X-Magpie-Tag")
	changed := append([]rowsBufRow(nil), rows...)
	changed[1].Name = "new"
	r := httptest.NewRequest("GET", "/x", nil)
	r.Header.Set("X-Magpie-Had", tag)
	w = httptest.NewRecorder()
	writeJSONRows(w, r, "rows", changed, struct{}{})
	if w.Header().Get("X-Magpie-Delta") != "rows" {
		t.Fatalf("no delta: %q", w.Body)
	}
	mid, _ := json.Marshal(changed[1])
	if want := `{"rows":[[0,1],` + string(mid) + `,[2,1]]}` + "\n"; w.Body.String() != want {
		t.Fatalf("delta\n got %q\nwant %q", w.Body, want)
	}
}
