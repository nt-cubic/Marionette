package gateway

import "testing"

// The words ollama.com answers a body over its cap with, and how the fit
// decides whether to treat them as one. See marionette_body_cap.go.
func TestMarionetteUnreadBodyOverCap(t *testing.T) {
	said := []byte(`{"error":{"message":"ollama: failed to read request body (ref: 4e771dcc-99bc-4a12-9d29-8490d3495ca6)","type":"invalid_request_error"}}`)
	const big = 34 << 20
	const small = 300 << 10

	cases := []struct {
		name   string
		status int
		body   []byte
		size   int
		want   bool
	}{
		{"the refusal that stalled Codex", 400, said, big, true},
		{"the same words on a small request are a cut, not a cap", 400, said, small, false},
		{"a body just over the floor", 400, said, marionetteUnreadBodyFloor, true},
		{"some other 400", 400, []byte(`{"error":{"message":"ollama: model not found"}}`), big, false},
		{"a 413 is already a byte-cap refusal", 413, said, big, false},
		{"a 429 names a rate, not a cap", 429, said, big, false},
		{"a 200 is not a refusal", 200, said, big, false},
		{"an empty error body", 400, nil, big, false},
	}
	for _, c := range cases {
		if got := marionetteUnreadBodyOverCap(c.status, c.body, c.size); got != c.want {
			t.Errorf("%s: %d with %d bytes -> %v, want %v", c.name, c.status, c.size, got, c.want)
		}
	}
}
