package gateway

import (
	"bytes"
	"net/http"
)

// Marionette patch — see tools\magpie-seed\marionette_body_cap.go.
//
// ollama.com refuses a request body larger than it takes with a 400 that
// says it never read the body at all:
//
//	ollama: failed to read request body (ref: <uuid>)
//
// so it names neither its cap nor the model's window. Stock byteCapRefusal
// (body_cap.go) matches a vendor's words for "too large", which these are
// not, and pageOverSize (body_cap.go) is about a proxy's page. The refusal
// therefore fell through as an ordinary 400: Codex re-sent the same body
// (33.9 MB — screenshots inline on every turn), every turn answered the
// same, and the conversation could neither go on nor compact.
//
// Measured against ollama.com on 2026-11-12, same model, same minute:
// 30 MB still answered "prompt too long; exceeded max context length"
// (the body was read), 34 MB and 40 MB answered this, in about 15 s.
//
// Counting it as a byte-cap refusal hands the request to the fit that
// already exists — the oldest images left out, asked again — and, when no
// image is left to drop, a 413 that names the size and the cap instead of
// an opaque 400 Codex takes for a transport error and retries verbatim.
//
// Only a body that could plausibly be over a cap counts. The same words can
// come from a connection cut on a small request, and learning a cap from
// that would make every later request drop its images.
const marionetteUnreadBodyFloor = 8 << 20 // 8 MiB

func marionetteUnreadBodyOverCap(status int, errBody []byte, body int) bool {
	if status != http.StatusBadRequest || body < marionetteUnreadBodyFloor {
		return false
	}
	return bytes.Contains(errBody, []byte("failed to read request body"))
}
