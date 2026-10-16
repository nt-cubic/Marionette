package gateway

import (
	"compress/gzip"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"strings"
	"time"

	"github.com/klauspost/compress/zstd"
	"github.com/yetone/magpie/internal/provider"
)

// Bound a single body and its read time without limiting how long a client
// may wait for a provider slot or how many streams a team gateway may serve.
//
// The limit is on the decoded body. Codex sends the whole conversation on
// every turn, its screenshots inline as base64, and zstd-compressed to a
// ChatGPT account, which magpie compresses again on the way out: a long
// session that read many images went past 128 MB decoded and got magpie's
// 413 on every retry, while the same turn is a fraction of that on the
// wire (NeoPengZhile, #1422). 512 MB leaves such a session room and still
// bounds what one request may hold in memory.
const (
	defaultBodyLimit   = 512 << 20
	defaultBodyTimeout = 60 * time.Second
)

type requestLimits struct {
	body        int64
	readTimeout time.Duration
}

func (s *Server) requestBody(w http.ResponseWriter, r *http.Request, from provider.Protocol) ([]byte, bool) {
	return s.readRequestBody(w, r, from, 0)
}

func (s *Server) readRequestBody(w http.ResponseWriter, r *http.Request, from provider.Protocol, limit int64) ([]byte, bool) {
	limits := s.requestLimits
	if limit > 0 {
		limits.body = limit
	}
	body, status, err := readBoundedRequestBody(w, r, limits)
	if err != nil {
		writeError(w, from, status, err.Error())
		return nil, false
	}
	return body, true
}

// idleBody renews the socket deadline on wire bytes, including reads made by
// decoder initialization. Decompressed output must not keep a stalled socket alive.
type idleBody struct {
	io.ReadCloser
	controller *http.ResponseController
	idle       time.Duration
}

func (b *idleBody) Read(p []byte) (int, error) {
	n, err := b.ReadCloser.Read(p)
	if n > 0 {
		if e := b.controller.SetReadDeadline(time.Now().Add(b.idle)); e != nil && !errors.Is(e, http.ErrNotSupported) {
			return n, e
		}
	}
	return n, err
}

// errBodyEncoding is a Content-Encoding magpie can't decode.
var errBodyEncoding = errors.New("unsupported Content-Encoding")

// decodedBody reads r.Body through its Content-Encoding: Codex sends its
// bodies zstd, to the ChatGPT backend path and to a custom provider's
// /v1/responses alike (#1223), and other clients gzip. The header is
// dropped once decoded, so nothing after this sees or forwards it.
func decodedBody(r *http.Request) (io.ReadCloser, error) {
	var rd io.ReadCloser = io.NopCloser(r.Body)
	switch enc := strings.ToLower(strings.TrimSpace(r.Header.Get("Content-Encoding"))); enc {
	case "", "identity":
	case "zstd":
		d, err := zstd.NewReader(r.Body, zstd.WithDecoderMaxMemory(defaultBodyLimit))
		if err != nil {
			return nil, err
		}
		rd = d.IOReadCloser()
	case "gzip", "x-gzip":
		g, err := gzip.NewReader(r.Body)
		if err != nil {
			return nil, fmt.Errorf("request body is not gzip: %w", err)
		}
		rd = g
	default:
		return nil, fmt.Errorf("%w: magpie can't read a %q body; send it as identity, gzip or zstd", errBodyEncoding, enc)
	}
	r.Header.Del("Content-Encoding")
	return rd, nil
}

// Read one bounded body, decoded; callers choose their protocol's error
// envelope. The limit holds for the decoded bytes too.
func readBoundedRequestBody(w http.ResponseWriter, r *http.Request, limits requestLimits) ([]byte, int, error) {
	controller := http.NewResponseController(w)
	reject := func(status int, err error) ([]byte, int, error) {
		// Do not let net/http drain a rejected body from a stalled client.
		w.Header().Set("Connection", "close")
		controller.SetReadDeadline(time.Now())
		if r.Body != nil {
			r.Body.Close()
		}
		controller.SetReadDeadline(time.Time{})
		return nil, status, err
	}
	if limits.body <= 0 {
		limits.body = defaultBodyLimit
	}
	if limits.readTimeout <= 0 {
		limits.readTimeout = defaultBodyTimeout
	}
	if r.ContentLength > limits.body {
		return reject(http.StatusRequestEntityTooLarge, overLimit(limits.body))
	}
	if err := controller.SetReadDeadline(time.Now().Add(limits.readTimeout)); err != nil && !errors.Is(err, http.ErrNotSupported) {
		return reject(http.StatusInternalServerError, errors.New("could not set request body deadline"))
	}
	defer controller.SetReadDeadline(time.Time{})
	readFailure := func(err error) ([]byte, int, error) {
		var timeout net.Error
		if errors.As(err, &timeout) && timeout.Timeout() {
			return reject(http.StatusRequestTimeout, errors.New("request body read timed out"))
		}
		return reject(http.StatusBadRequest, err)
	}
	original := r.Body
	r.Body = &idleBody{original, controller, limits.readTimeout}
	defer func() { r.Body = original }()
	rd, err := decodedBody(r)
	if errors.Is(err, errBodyEncoding) {
		return reject(http.StatusUnsupportedMediaType, err)
	}
	if err != nil {
		return readFailure(err)
	}
	defer rd.Close()
	// Limit decoded bytes as well, without allocating from Content-Length.
	body, err := io.ReadAll(io.LimitReader(rd, limits.body+1))
	if int64(len(body)) > limits.body {
		return reject(http.StatusRequestEntityTooLarge, overLimit(limits.body))
	}
	if err != nil {
		return readFailure(err)
	}
	return body, 0, nil
}

// overLimit says a body is over magpie's own limit, and that it is
// magpie's, not the provider's: an agent shows the message as it comes,
// and a bare "Payload Too Large" read as the model's or the vendor's
// (#1422). writeError adds what to do about it.
func overLimit(limit int64) error {
	return fmt.Errorf("request body is over magpie's gateway limit of %s (decoded); the provider was not asked", sizeWord(limit))
}

// sizeWord is a byte count in MB, or in KB or bytes when it is smaller,
// for limits set low (a test's, a route's own).
func sizeWord(n int64) string {
	switch {
	case n >= 1<<20 && n%(1<<20) == 0:
		return fmt.Sprintf("%d MB", n>>20)
	case n >= 1<<10 && n%(1<<10) == 0:
		return fmt.Sprintf("%d KB", n>>10)
	}
	return fmt.Sprintf("%d bytes", n)
}
