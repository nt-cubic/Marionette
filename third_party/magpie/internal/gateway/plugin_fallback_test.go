package gateway

import (
	"io"
	"net/http"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A plugin is told when another candidate follows its try
// (FallbackHeader), so one whose vendor queues the request answers at once
// and the next is asked, instead of the agent waiting in a free model's
// queue for minutes while another provider could answer (axuhere on X:
// "模型请求正在排队 · 预计等待超过 10 分钟"). The last candidate isn't told,
// and waits as the vendor's own client does; the header never reaches the
// vendor.
func TestPluginToldAnotherFollows(t *testing.T) {
	var mu sync.Mutex
	type seen struct{ path, fallback string }
	var got []seen
	var leaked []string
	pid := besideFake(t, "fakeco", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.Copy(io.Discard, r.Body)
		mu.Lock()
		got = append(got, seen{r.URL.Path, r.Header.Get("X-Saw-Fallback")})
		if v := r.Header.Values(provider.FallbackHeader); len(v) > 0 {
			leaked = append(leaked, v...)
		}
		mu.Unlock()
		if r.Header.Get("X-Saw-Fallback") == "1" {
			// as Qoder's plugin answers its queue when told another follows
			w.Header().Set("Content-Type", "application/json")
			w.Header().Set("Retry-After", "30")
			w.WriteHeader(http.StatusServiceUnavailable)
			io.WriteString(w, `{"error":{"message":"queued the request (position 12, about 10 minutes)"}}`)
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		if r.URL.Path == "/v1/messages" {
			for _, e := range []string{
				`{"type":"message_start","message":{"id":"m","type":"message","role":"assistant","model":"fake-claude","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}`,
				`{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}`,
				`{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}`,
				`{"type":"content_block_stop","index":0}`,
				`{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}`,
				`{"type":"message_stop"}`,
			} {
				io.WriteString(w, "data: "+e+"\n\n")
			}
			return
		}
		io.WriteString(w, `data: {"id":"c","object":"chat.completion.chunk","model":"fake-1","choices":[{"index":0,"delta":{"role":"assistant","content":"hi"},"finish_reason":null}]}`+"\n\n")
		io.WriteString(w, `data: {"id":"c","object":"chat.completion.chunk","model":"fake-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}`+"\n\ndata: [DONE]\n\n")
	}))
	if err := provider.SaveGroup(provider.Group{Name: "Queue", Members: []string{pid + "/fake-1", pid + "/fake-claude"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	take := func() []seen {
		mu.Lock()
		defer mu.Unlock()
		s := got
		got = nil
		return s
	}

	// a group: its first member is told another follows, gives way, and
	// the last answers untold; the trace says why the first didn't
	rt := postProto(t, New(), "/v1/chat/completions", `{"model":"group/queue","messages":[{"role":"user","content":"hi"}]}`)
	sent := take()
	if len(sent) != 2 || sent[0] != (seen{"/v1/chat/completions", "1"}) || sent[1] != (seen{"/v1/messages", ""}) {
		t.Fatalf("the vendor saw %+v; want fake-1 told another follows, then fake-claude untold", sent)
	}
	if len(rt.Tries) < 2 || rt.Tries[0].Status != http.StatusServiceUnavailable || rt.Tries[len(rt.Tries)-1].Status != http.StatusOK {
		t.Fatalf("tries %+v", rt.Tries)
	}
	if !strings.Contains(rt.Tries[0].Error, "queued the request (position 12") {
		t.Fatalf("the first try says %q, not why it gave way", rt.Tries[0].Error)
	}

	// a model of its own, one account: nobody follows, nothing is said
	postProto(t, New(), "/v1/chat/completions", `{"model":"`+pid+`/fake-1","messages":[{"role":"user","content":"hi"}]}`)
	if sent := take(); len(sent) != 1 || sent[0].fallback != "" {
		t.Fatalf("a lone model's plugin was told %+v", sent)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(leaked) > 0 {
		t.Fatalf("the vendor got %s: %q", provider.FallbackHeader, leaked)
	}
}
