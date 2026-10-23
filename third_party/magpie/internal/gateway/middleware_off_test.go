package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/gwmw"
	"github.com/yetone/magpie/internal/masking"
	"github.com/yetone/magpie/internal/middleware"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// echoing is a vendor that keeps what it was sent and says the last
// message back.
type echoing struct {
	mu   sync.Mutex
	sent []string
}

func (e *echoing) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	b, _ := io.ReadAll(r.Body)
	e.mu.Lock()
	e.sent = append(e.sent, string(b))
	e.mu.Unlock()
	var req struct {
		Messages []struct{ Content string } `json:"messages"`
	}
	json.Unmarshal(b, &req)
	said := ""
	if n := len(req.Messages); n > 0 {
		said = req.Messages[n-1].Content
	}
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]any{"id": "x", "choices": []any{map[string]any{"index": 0, "message": map[string]any{"role": "assistant", "content": "you said " + said}, "finish_reason": "stop"}}})
}

func (e *echoing) count() int {
	e.mu.Lock()
	defer e.mu.Unlock()
	return len(e.sent)
}

func (e *echoing) last() string {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.sent[len(e.sent)-1]
}

// panicky is a masker that fails, on each way the gateway asks it to
// mask (a Teller is asked through JSONHits and TextHits).
type panicky struct{ masking.Masker }

func (panicky) JSON([]byte) ([]byte, int)            { panic("masker broke") }
func (panicky) JSONHits([]byte) ([]byte, []gwmw.Hit) { panic("masker broke") }
func (panicky) Text(string) (string, int)            { panic("masker broke") }
func (panicky) TextHits(string) (string, []gwmw.Hit) { panic("masker broke") }

// The middleware point's masker with plugins/redact on and off. On, a
// secret goes to the vendor as a placeholder and comes back as it was.
// Off, while the settings ask for masking, nothing is sent: the request is
// refused with 503 and the vendor never sees it, unless a refresh turns
// the plugin on, as the settings asking for masking do. A masker that
// panics refuses too. Off with nothing asked for, a request goes as it is.
func TestMaskingOnAndOff(t *testing.T) {
	fresh(t)
	const secret = "sk-proj-abcdEFGH1234ijklMNOP5678qrst"
	v := &echoing{}
	serveOn(t, "pa", "ka", []string{"m"}, v)
	if err := settings.Save(settings.Settings{Redact: true}); err != nil {
		t.Fatal(err)
	}
	s := New()
	ask := `{"model":"pa/m","messages":[{"role":"user","content":"key ` + secret + `"}]}`
	masked := func(when string) {
		t.Helper()
		code, body := postAs(t, s, "", ask)
		if code != 200 || strings.Contains(v.last(), secret) || !strings.Contains(body, "you said key "+secret) {
			t.Fatalf("%s: %d %s, the vendor got %s", when, code, body, v.last())
		}
	}
	refused := func(when string) {
		t.Helper()
		n := v.count()
		code, body := postAs(t, s, "", ask)
		if code != http.StatusServiceUnavailable || !strings.Contains(body, "nothing was sent") || v.count() != n {
			t.Fatalf("%s: %d %s, the vendor was sent %d", when, code, body, v.count()-n)
		}
	}
	masked("on")
	t.Run("off", func(t *testing.T) {
		withoutMasker(t)
		gwmw.ForgetRefreshForTest()
		refused("off")
		// the settings ask for masking: the refresh turns it on
		gwmw.ForgetRefreshForTest()
		var remove func()
		stop := gwmw.Refresh(func() { remove = gwmw.Mask(masking.Masker{}) })
		masked("off, turned on by the refresh")
		remove()
		stop()
		gwmw.ForgetRefreshForTest()
		// nothing asked for: it goes as it is
		if err := settings.Save(settings.Settings{}); err != nil {
			t.Fatal(err)
		}
		if code, body := postAs(t, s, "", ask); code != 200 || !strings.Contains(v.last(), secret) {
			t.Fatalf("off, not asked for: %d %s, the vendor got %s", code, body, v.last())
		}
		if err := settings.Save(settings.Settings{Redact: true}); err != nil {
			t.Fatal(err)
		}
	})
	t.Run("panics", func(t *testing.T) {
		remove := gwmw.Mask(panicky{})
		defer remove()
		refused("a masker that panics")
	})
	masked("on again")
}

// The same for a web search's query, which goes to a search service, not
// a model: off with masking asked for, the search MCP's query isn't sent.
func TestMaskedQueryFailsClosed(t *testing.T) {
	fresh(t)
	if err := settings.Save(settings.Settings{Redact: true}); err != nil {
		t.Fatal(err)
	}
	const secret = "sk-proj-abcdEFGH1234ijklMNOP5678qrst"
	if q, err := maskedQuery("find "+secret, nil); err != nil || strings.Contains(q, secret) {
		t.Fatalf("on: %q %v", q, err)
	}
	withoutMasker(t)
	gwmw.ForgetRefreshForTest()
	if q, err := maskedQuery("find "+secret, nil); err == nil {
		t.Fatalf("off: sent %q", q)
	}
	// one the user set to go unmasked keeps it, plugin or not
	p := &provider.Provider{ID: "local", Unredacted: true, Chat: "http://127.0.0.1:1/v1"}
	if q, err := maskedQuery("find "+secret, p); err != nil || q != "find "+secret {
		t.Fatalf("off, to a provider set unmasked: %q %v", q, err)
	}
}

// The middleware point's agent middleware with plugins/middleware on and
// off: on, a JS middleware in plugins.json rewrites the request; off, the
// gateway runs none, and the request is routed as the agent sent it.
func TestMiddlewareOnAndOff(t *testing.T) {
	fresh(t)
	v := &echoing{}
	serveOn(t, "pa", "ka", []string{"m"}, v)
	t.Cleanup(plugin.Settle)
	mw := filepath.Join(t.TempDir(), "alias.middleware.js")
	os.WriteFile(mw, []byte(`
export function onRequest(body, ctx) {
  if (body.model === "fast") body.model = "pa/m";
  return body;
}`), 0o644)
	b, _ := json.Marshal(plugin.List{Plugins: []plugin.Entry{{Spec: mw}}})
	os.MkdirAll(settings.Dir(), 0o755)
	os.WriteFile(filepath.Join(settings.Dir(), "plugins.json"), b, 0o644)
	middleware.Reload()
	defer middleware.Reload()
	if middleware.On() != 1 {
		t.Fatalf("middleware on: %d", middleware.On())
	}
	s := New()
	ask := `{"model":"fast","messages":[{"role":"user","content":"hi"}]}`
	if code, body := postAs(t, s, "", ask); code != 200 {
		t.Fatalf("on: %d %s", code, body)
	}
	t.Run("off", func(t *testing.T) {
		withoutMiddleware(t)
		calls := middleware.States()[mw].Calls
		n := v.count()
		if code, body := postAs(t, s, "", ask); code == 200 || v.count() != n {
			t.Fatalf("off: %d %s, the vendor was sent %d", code, body, v.count()-n)
		}
		if got := middleware.States()[mw].Calls; got != calls {
			t.Fatalf("off, the middleware ran: %d calls, was %d", got, calls)
		}
	})
	// off while plugins.json has a middleware on: the gateway asks a
	// refresh, which turns the plugin on for this request (the gateway's is
	// pluginrt.Refresh), not after KeepFresh's next look
	t.Run("off, then a refresh turns it on", func(t *testing.T) {
		withoutMiddleware(t)
		gwmw.ForgetRefreshForTest()
		var remove func()
		stop := gwmw.Refresh(func() { remove = gwmw.Use(middleware.Chain{}) })
		defer stop()
		if code, body := postAs(t, s, "", ask); code != 200 || remove == nil {
			t.Fatalf("refreshed: %d %s", code, body)
		}
		remove()
	})
	if code, body := postAs(t, s, "", ask); code != 200 {
		t.Fatalf("on again: %d %s", code, body)
	}
}
