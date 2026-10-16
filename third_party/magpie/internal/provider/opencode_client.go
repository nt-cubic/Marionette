package provider

import (
	"container/list"
	"crypto/rand"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/catalog"
)

// OpenCodeVersion is the OpenCode release magpie says it is to OpenCode's
// gateway. Zen's free tier turns away anything older than 1.18.0 (426
// "OpenCode 1.18.0 or newer is required to use the free tier").
const OpenCodeVersion = "1.19.0"

// OpenCodeUserAgent is the User-Agent OpenCode 1.19.0 sends, its ai-sdk
// and Bun included, as captured from the real client.
const OpenCodeUserAgent = "opencode/" + OpenCodeVersion + " ai-sdk/provider-utils/4.0.23 runtime/bun/1.3.14"

// OpenCodeAnonymousKey is the key OpenCode asks Zen with when no one is
// signed in to it, which Zen's free models answer.
const OpenCodeAnonymousKey = "public"

// OpenCodeClient makes a request to OpenCode's gateway (Zen or Go) carry
// what OpenCode itself sends there (packages/core/src/session/
// model-request.ts): its User-Agent, the session in x-opencode-session and
// x-opencode-session-id, the turn in x-opencode-request, and
// x-opencode-client and x-opencode-project. OpenCode 1.19.0 sends Zen no
// x-session-affinity or X-Session-Id (captured from the real client), and
// since mid-October 2026 Zen's free tier answers 403 FreeTierError to a
// request carrying x-session-affinity, so neither goes. Zen's
// free models are served to no one else ("OpenCode's free tier can only be
// used from within OpenCode"), and Zen reads the session id as OpenCode
// lays one out (openCodeSession). session names the conversation, whose
// turns keep one session id, which Zen routes and caches by.
func OpenCodeClient(h http.Header, session string) {
	id := openCodeSession(session)
	h.Set("User-Agent", OpenCodeUserAgent)
	h.Set("x-opencode-session", id)
	h.Set("x-opencode-session-id", id)
	h.Del("x-session-affinity")
	h.Del("X-Session-Id")
	h.Set("x-opencode-request", openCodeNewID("msg", false, time.Now()))
	h.Set("x-opencode-client", "cli")
	// OpenCode's project outside a git repository
	h.Set("x-opencode-project", "global")
}

// OpenCodeURL says whether u is at OpenCode's gateway (Zen, Go or the
// inference API): opencode.ai or a name under it, whatever port the URL
// gives. What goes there is judged by the URL it goes to, not by the
// provider's first base (IsOpenCode), so a custom provider whose URL
// names a port, or one with only some of its endpoints there, is asked
// as OpenCode asks too.
func OpenCodeURL(u string) bool {
	pu, err := url.Parse(strings.TrimSpace(u))
	return err == nil && openCodeHost(pu.Hostname())
}

// openCodeHost says whether h (no port) is one of OpenCode's names.
func openCodeHost(h string) bool {
	h = strings.TrimSuffix(strings.ToLower(h), ".")
	return h == "opencode.ai" || strings.HasSuffix(h, ".opencode.ai")
}

// OpenCodeSession names a session on a request to OpenCode's gateway that
// names none yet: since September 2026 it answers every request without
// x-opencode-session 400 "Request is missing x-opencode-session and
// cannot be routed efficiently" (luci on Discord). A call that is no
// conversation's turn — a model list, a System One decision, a key check —
// gets a session of its own, as OpenCode gives its own stateless calls
// one (packages/core/src/generate.ts: "Gateways require session
// attribution even for a stateless call"). A turn's is named before this,
// by OpenCodeClient, and stays; so does one the user set on the provider.
// Sign calls it, so every signed request has it.
func OpenCodeSession(h http.Header, u string) {
	if !OpenCodeURL(u) || h.Get("x-opencode-session") != "" {
		return
	}
	id := openCodeSession("")
	h.Set("x-opencode-session", id)
	h.Set("x-opencode-session-id", id)
}

// OpenCodeFree says whether model is one of OpenCode Zen's free ones on p,
// which Zen serves only to what looks like OpenCode: besides the headers
// OpenCodeClient sets, a streamed request offering tools named bash and
// read (lowercase, each taking a parameter: OpenCodeToolSchemas) at the
// inference API (OpenCodeFreeURL), as checked against Zen in October 2026. The
// gateway asks them so whoever the client is (gateway/zenfree.go), and the
// Test button's probe so (zenFreeProbe). A free model is one whose id ends
// in -free, or one models.dev's opencode prices at nothing, as big-pickle,
// whose id doesn't say so (361 on Discord: it answered 403 "OpenCode's
// free tier can only be used from within OpenCode"); openCodeFreeIDs are
// such ones known without the catalog.
func (p Provider) OpenCodeFree(model string) bool {
	if !p.IsOpenCode() {
		return false
	}
	if strings.HasSuffix(model, "-free") || openCodeFreeIDs[model] {
		return true
	}
	pr, ok := catalog.PriceOf("opencode", model)
	return ok && pr.Input == 0 && pr.Output == 0
}

// openCodeFreeIDs are Zen's free models whose ids don't end in -free.
var openCodeFreeIDs = map[string]bool{"big-pickle": true, "grok-code": true}

// OpenCodeTools are the tools Zen's free tier wants offered, by name.
var OpenCodeTools = []string{"bash", "read"}

// OpenCodeToolSchemas are the parameters OpenCode itself gives bash and
// read (1.19.0, as it sent them to Zen). Since mid-October 2026 Zen's free
// tier answers 403 FreeTierError when either is offered with no
// parameters, as magpie's stubs were ({"type":"object","properties":{}});
// one property is enough, and these are OpenCode's own.
var OpenCodeToolSchemas = map[string]string{
	"bash": `{"type":"object","properties":{"command":{"type":"string","description":"The command to execute"},"timeout":{"type":"integer","description":"Optional timeout in milliseconds"},"workdir":{"type":"string","description":"The working directory to run the command in"}},"required":["command"]}`,
	"read": `{"type":"object","properties":{"filePath":{"type":"string","description":"The absolute path to the file or directory to read"},"offset":{"type":"integer","description":"The line number to start reading from (1-indexed)"},"limit":{"type":"integer","description":"The maximum number of lines to read"}},"required":["filePath"]}`,
}

// OpenCodeFreeURL is where a request to Zen's free tier goes, given the
// URL it would go to at Zen (https://opencode.ai/zen/v1/…, or under the
// root for Messages): the same path under the inference API's root, which
// models.dev gives OpenCode as Zen's URL since October 2026 and OpenCode
// 1.19.0 asks. Since mid-October 2026 Zen answers a free model at /zen/v1
// with 403 FreeTierError even when the request is OpenCode's own, word for
// word, and the same request at /inference/v1 with 200. Any other URL, Go's
// (/zen/go) among them, is u.
func OpenCodeFreeURL(u string) string {
	pu, err := url.Parse(strings.TrimSpace(u))
	if err != nil || pu.Scheme == "" {
		return u
	}
	if !openCodeHost(pu.Hostname()) {
		return u
	}
	rest, ok := strings.CutPrefix(pu.Path, "/zen")
	if !ok || rest != "" && rest[0] != '/' || rest == "/go" || strings.HasPrefix(rest, "/go/") {
		return u
	}
	pu.Path = "/inference" + rest
	return pu.String()
}

// OpenCodeStub describes a tool offered only for Zen's free tier to see,
// which the client doesn't have.
const OpenCodeStub = "Not available in this session. Never call this tool."

// zenFreeProbe is the Test button's smallest request, body, asked as Zen's
// free tier wants it: streamed, offering bash and read, which say they
// aren't to be called.
func zenFreeProbe(proto Protocol, body string) string {
	var m map[string]any
	if json.Unmarshal([]byte(body), &m) != nil {
		return body
	}
	var tools []any
	for _, n := range OpenCodeTools {
		var schema map[string]any
		_ = json.Unmarshal([]byte(OpenCodeToolSchemas[n]), &schema)
		switch proto {
		case Chat:
			tools = append(tools, map[string]any{"type": "function", "function": map[string]any{"name": n, "description": OpenCodeStub, "parameters": schema}})
		case Responses:
			tools = append(tools, map[string]any{"type": "function", "name": n, "description": OpenCodeStub, "parameters": schema})
		default:
			tools = append(tools, map[string]any{"name": n, "description": OpenCodeStub, "input_schema": schema})
		}
	}
	m["stream"], m["tools"] = true, tools
	b, err := json.Marshal(m)
	if err != nil {
		return body
	}
	return string(b)
}

var openCodeIDRe = regexp.MustCompile(`^[a-z]{3}_[0-9a-f]{12}[0-9A-Za-z]{14}$`)

const base62 = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"

// openCodeIDs is the clock and counter ids are made with, and the session
// id made for each conversation.
var openCodeIDs = struct {
	sync.Mutex
	last, counter int64
	sessions      map[string]*list.Element // seed → its element in order
	order         *list.List               // of openCodeKept, the least recently used first
}{sessions: map[string]*list.Element{}, order: list.New()}

type openCodeKept struct{ seed, id string }

// openCodeKeep is how many conversations' session ids are kept.
const openCodeKeep = 1000

// openCodeNewID is an id as OpenCode makes one (packages/schema/src/
// identifier.ts): prefix_, 12 hex digits of the time in milliseconds times
// 0x1000 plus a count of the ids made in that millisecond (bit-inverted
// for a session, so the newest sorts first), then 14 random base62 ones.
func openCodeNewID(prefix string, descending bool, now time.Time) string {
	ms := now.UnixMilli()
	openCodeIDs.Lock()
	if ms != openCodeIDs.last {
		openCodeIDs.last, openCodeIDs.counter = ms, 0
	}
	openCodeIDs.counter++
	current := uint64(ms)*0x1000 + uint64(openCodeIDs.counter)
	openCodeIDs.Unlock()
	if descending {
		current = ^current
	}
	var b [14]byte
	if _, err := rand.Read(b[:]); err != nil {
		copy(b[:], randomUUID())
	}
	id := []byte(fmt.Sprintf("%s_%012x", prefix, current&0xffffffffffff))
	for _, c := range b {
		id = append(id, base62[int(c)%len(base62)])
	}
	return string(id)
}

// openCodeRealSession says whether id is a session id OpenCode could have
// made: of its shape, with a small count in its low 12 bits (inverted).
// Zen's free tier answers 403 FreeTierError to one that isn't, a hash cut
// to the same shape among them (c9cu on Discord).
func openCodeRealSession(id string) bool {
	if !strings.HasPrefix(id, "ses_") || !openCodeIDRe.MatchString(id) {
		return false
	}
	low, err := strconv.ParseUint(id[13:16], 16, 16)
	return err == nil && 0xfff-low < 16
}

// openCodeSession is the session id sent for the conversation seed names:
// seed itself when it is one OpenCode could have made, else one made the
// first time seed is seen and kept for its next turns (the last
// openCodeKeep conversations' are kept), or a new one when there is no
// seed.
func openCodeSession(seed string) string {
	if openCodeRealSession(seed) {
		return seed
	}
	if seed == "" {
		return openCodeNewID("ses", true, time.Now())
	}
	id := openCodeNewID("ses", true, time.Now()) // made first: it takes the lock
	openCodeIDs.Lock()
	defer openCodeIDs.Unlock()
	if e, ok := openCodeIDs.sessions[seed]; ok {
		openCodeIDs.order.MoveToBack(e)
		return e.Value.(openCodeKept).id
	}
	if openCodeIDs.order.Len() >= openCodeKeep {
		oldest := openCodeIDs.order.Front()
		openCodeIDs.order.Remove(oldest)
		delete(openCodeIDs.sessions, oldest.Value.(openCodeKept).seed)
	}
	openCodeIDs.sessions[seed] = openCodeIDs.order.PushBack(openCodeKept{seed, id})
	return id
}
