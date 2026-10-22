package plugin

import (
	"encoding/base64"
	"encoding/json"
	"strings"
	"testing"
)

// TestChunkFrameReadsWithoutJSON: a chunk frame as host.js writes it
// (JSON.stringify({id, event: "chunk", data}), which chunkLine joins by
// hand) is read without encoding/json, and reads the same as through it.
// Any line of another shape goes through encoding/json as before.
func TestChunkFrameReadsWithoutJSON(t *testing.T) {
	raw := make([]byte, 70<<10)
	for i := range raw {
		raw[i] = byte(i * 31)
	}
	for _, tc := range []struct {
		id   int64
		data string
	}{
		{1, base64.StdEncoding.EncodeToString(raw)},
		{987654321, base64.StdEncoding.EncodeToString([]byte("data: {}\n\n"))},
		{42, "AA=="},
	} {
		b, err := json.Marshal(struct {
			ID    int64  `json:"id"`
			Event string `json:"event"`
			Data  string `json:"data"`
		}{tc.id, "chunk", tc.data})
		if err != nil {
			t.Fatal(err)
		}
		for _, line := range []string{string(b) + "\n", string(b), string(b) + "\r\n"} {
			got, ok := chunkFrame([]byte(line))
			if !ok {
				t.Fatalf("a chunk frame as host.js writes it (%.60q…) went through encoding/json", line)
			}
			var want message
			if err := json.Unmarshal([]byte(line), &want); err != nil {
				t.Fatal(err)
			}
			if got.ID != want.ID || got.Event != want.Event || got.Data != want.Data {
				t.Fatalf("read as id %d %q %d bytes, encoding/json reads id %d %q %d bytes", got.ID, got.Event, len(got.Data), want.ID, want.Event, len(want.Data))
			}
		}
	}
	for _, line := range []string{
		`{"id":1,"event":"head","status":200,"headers":{}}`,
		`{"id":1,"result":null}`,
		`{"id":1,"event":"chunk","data":"QU` + "\\" + `/A"}`, // an escape: JSON reads it otherwise
		`{"id":1,"event":"chunk","data":"QUFB\"}`,
		`{"id":-1,"event":"chunk","data":"QUFB"}`,
		`{"id":1e3,"event":"chunk","data":"QUFB"}`,
		`{"id":1,"event":"chunk","data":"QUFB","x":"y"}`,
		`{"id":1,"event":"chunk","data":"QUFB"} `,
		`{"id":1,"event":"chunk","data":"` + strings.Repeat("A", 8) + `"}x`,
		`{"id":12345678901234567890,"event":"chunk","data":"QUFB"}`,
		`{"event":"chunk","id":1,"data":"QUFB"}`,
	} {
		if m, ok := chunkFrame([]byte(line)); ok {
			t.Fatalf("%s read as a chunk frame %+v, want it left to encoding/json", line, m)
		}
	}
}
