package gateway

import (
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// The Kiro plugin's real replies (opencode-kiro-auth 0.1.16 against
// Kiro, 10-24): Kiro caches but counts no tokens, cache included, so the
// plugin says magpie_cache_unknown and the Routing page shows no hit rate
// where it showed 0% (zeewell on X).
const (
	kiroStream = "event: message_start\n" +
		`data: {"type":"message_start","message":{"id":"msg_6eb0d1cc14c9126db4218f61","type":"message","role":"assistant","model":"claude-sonnet-4.5","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}}` + "\n\n" +
		"event: content_block_start\n" +
		`data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}` + "\n\n" +
		"event: content_block_delta\n" +
		`data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"OK"}}` + "\n\n" +
		"event: content_block_stop\n" +
		`data: {"type":"content_block_stop","index":0}` + "\n\n" +
		"event: message_delta\n" +
		`data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"input_tokens":0,"output_tokens":19,"magpie_cache_unknown":true}}` + "\n\n" +
		"event: message_stop\n" +
		`data: {"type":"message_stop"}` + "\n\n"
	kiroJSON = `{"id":"msg_e6ad400ac6ed0f3febe5125d","type":"message","role":"assistant","model":"claude-sonnet-4.5","content":[{"type":"text","text":"OK."}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":1,"magpie_cache_unknown":true}}`
	// a vendor that counts its cache and read none of it: a real 0%
	missJSON = `{"id":"msg_1","type":"message","role":"assistant","model":"m1","content":[{"type":"text","text":"OK."}],"stop_reason":"end_turn","usage":{"input_tokens":1200,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}`
)

func TestCacheUnknownReachesTheRoute(t *testing.T) {
	messages := `{"model":"fake/m1","max_tokens":20,"messages":[{"role":"user","content":"Say just OK."}]}`
	streamed := `{"model":"fake/m1","max_tokens":20,"stream":true,"messages":[{"role":"user","content":"Say just OK."}]}`
	chat := `{"model":"fake/m1","messages":[{"role":"user","content":"Say just OK."}]}`
	for _, c := range []struct {
		name, ctype, reply, path, request string
		unknown                           bool
	}{
		{"Claude Code, whole", "application/json", kiroJSON, "/v1/messages", messages, true},
		{"Claude Code, streamed", "", kiroStream, "/v1/messages", streamed, true},
		// a translated request is always streamed from the vendor
		{"translated for a Chat client", "", kiroStream, "/v1/chat/completions", chat, true},
		{"translated, streamed", "", kiroStream, "/v1/chat/completions", strings.Replace(chat, `"messages"`, `"stream":true,"messages"`, 1), true},
		{"a counted miss stays 0%", "application/json", missJSON, "/v1/messages", messages, false},
	} {
		t.Run(c.name, func(t *testing.T) {
			setup(t, provider.Anthropic, &fake{ctype: c.ctype, reply: c.reply})
			s := New()
			rec := httptest.NewRecorder()
			s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", c.path, strings.NewReader(c.request)))
			if rec.Code != 200 {
				t.Fatalf("gateway: %d %s", rec.Code, rec.Body)
			}
			r := lastRoute(s)
			if len(r.Usage) != 1 {
				t.Fatalf("route usage %+v", r.Usage)
			}
			if got := r.Usage[0].CacheUnknown; got != c.unknown {
				t.Fatalf("cache unknown %v, want %v: %+v", got, c.unknown, r.Usage[0])
			}
		})
	}
}

// a try whose vendor did count cache tokens is known, whatever another
// part of the reply said
func TestCacheCountedIsNotUnknown(t *testing.T) {
	var u Usage
	u.add(aUsage{OutputTokens: 1, CacheUnknown: true}.usage())
	u.add(aUsage{InputTokens: 10, OutputTokens: 2, CacheReadInputTokens: 90}.usage())
	if rs := routeUsage("p", "m", u); len(rs) != 1 || rs[0].CacheUnknown {
		t.Fatalf("%+v", rs)
	}
}

// what Claude Code is told is Anthropic's usage: magpie never adds the
// field to a reply it writes itself
func TestCacheUnknownNotTold(t *testing.T) {
	u := aUsage{InputTokens: 5, OutputTokens: 1, CacheUnknown: true}.usage()
	if u.anthropic().CacheUnknown {
		t.Fatal("told")
	}
}
