package gateway

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"regexp"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/yetone/magpie/internal/gwmw"
	"github.com/yetone/magpie/internal/masking"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// lc on Discord: a model on this machine or the local network the user set
// to go unmasked (provider.Unredacted) gets the request as the agent wrote
// it, with Settings' redaction on; set so, a chat, a token count and an
// embedding go to it whole. Unset, or able to fall back to a vendor, or in
// a group with one, it stays masked.
func TestLocalUnredactedProviderGetsTheRequestWhole(t *testing.T) {
	const key = "sk-proj-abcdEFGH1234ijklMNOP5678qrst"
	chat := `{"model":"fake/m1","max_tokens":10,"messages":[{"role":"user","content":"my key is ` + key + `"}]}`
	count := `{"model":"fake/m1","messages":[{"role":"user","content":"my key is ` + key + `"}]}`
	vendor := provider.Provider{ID: "vendor", Name: "Vendor", Key: "k", Models: []string{"m9"}, Anthropic: "https://api.vendor.example"}
	cases := []struct {
		name  string
		local func(*provider.Provider)
		group bool
		more  []provider.Provider
		whole bool
	}{
		{name: "set", local: func(p *provider.Provider) { p.Unredacted = true }, whole: true},
		{name: "unset", local: func(p *provider.Provider) {}},
		{name: "falls back to a vendor", local: func(p *provider.Provider) { p.Unredacted = true; p.Fallback = []string{"vendor/m9"} }, more: []provider.Provider{vendor}},
		{name: "falls back to another set", local: func(p *provider.Provider) { p.Unredacted = true; p.Fallback = []string{"lan/m2"} },
			more: []provider.Provider{{ID: "lan", Name: "LAN", Key: "k", Models: []string{"m2"}, Chat: "http://192.168.1.20:8000/v1", Unredacted: true}}, whole: true},
		{name: "a group of it alone", local: func(p *provider.Provider) { p.Unredacted = true }, group: true, whole: true},
		{name: "a group with a vendor", local: func(p *provider.Provider) { p.Unredacted = true }, group: true, more: []provider.Provider{vendor}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			f := &fake{ctype: "application/json", reply: `{"id":"msg_1","type":"message","role":"assistant","model":"m1","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":1}}`}
			setup(t, provider.Anthropic, f)
			p, err := provider.Find("fake")
			if err != nil {
				t.Fatal(err)
			}
			tc.local(p)
			for _, q := range append([]provider.Provider{*p}, tc.more...) {
				if err := provider.Save(q); err != nil {
					t.Fatal(err)
				}
			}
			body := chat
			if tc.group {
				members := []string{"fake/m1"}
				if len(tc.more) > 0 {
					members = append(members, "vendor/m9")
				}
				if err := provider.SaveGroup(provider.Group{ID: "mine", Name: "mine", Members: members, Routing: "order"}); err != nil {
					t.Fatal(err)
				}
				body = strings.Replace(chat, "fake/m1", "group/mine", 1)
			}
			if err := settings.Save(settings.Settings{Redact: true}); err != nil {
				t.Fatal(err)
			}
			if code, reply := post(t, "/v1/messages", body); code != 200 {
				t.Fatalf("%d %s", code, reply)
			}
			if whole := strings.Contains(string(f.got), key); whole != tc.whole {
				t.Fatalf("sent whole %v, want %v: %s", whole, tc.whole, f.got)
			}
			if tc.group {
				return
			}
			f.reply = `{"input_tokens":17}`
			if code, reply := post(t, "/v1/messages/count_tokens", count); code != 200 || f.path != "/v1/messages/count_tokens" {
				t.Fatalf("count: %d %s to %s", code, reply, f.path)
			}
			if whole := strings.Contains(string(f.got), key); whole != tc.whole {
				t.Fatalf("counted whole %v, want %v: %s", whole, tc.whole, f.got)
			}
		})
	}
}

// Embeddings likewise: to a local model set to go unmasked, whole.
func TestLocalUnredactedEmbeddings(t *testing.T) {
	s, l := shelved(t)
	p, err := provider.Find("lib")
	if err != nil {
		t.Fatal(err)
	}
	p.Unredacted = true
	if err := provider.Save(*p); err != nil {
		t.Fatal(err)
	}
	if err := settings.Save(settings.Settings{Redact: true}); err != nil {
		t.Fatal(err)
	}
	secret := "sk-ant-api03-" + strings.Repeat("Ab3x", 20)
	if code, raw := postRetrieval(t, s, "/v1/embeddings", `{"model":"lib/embed-1","input":"key `+secret+`"}`); code != 200 {
		t.Fatalf("%d %s", code, raw)
	}
	if sent := l.got("/v1/embeddings"); len(sent) != 1 || !strings.Contains(sent[0], secret) {
		t.Fatalf("the local model was sent %v", sent)
	}
}

// countingMasker is the masker plugins/redact serves, counting the bodies
// it is asked to scan.
type countingMasker struct {
	masking.Masker
	scans *atomic.Int32
}

func (m countingMasker) JSON(b []byte) ([]byte, int) {
	m.scans.Add(1)
	return m.Masker.JSON(b)
}

func (m countingMasker) JSONHits(b []byte) ([]byte, []gwmw.Hit) {
	m.scans.Add(1)
	return m.Masker.JSONHits(b)
}

func (m countingMasker) JSONHitsContext(ctx context.Context, b []byte) ([]byte, []gwmw.Hit) {
	m.scans.Add(1)
	return m.Masker.JSONHitsContext(ctx, b)
}

// scanCount serves a countingMasker for t and answers its count.
func scanCount(t *testing.T) *atomic.Int32 {
	n := new(atomic.Int32)
	t.Cleanup(gwmw.Mask(countingMasker{scans: n}))
	return n
}

// echoVendor is a vendor not set to go unmasked: it answers a Messages
// request with the first placeholder it was sent, so a reply that comes
// back with the value proves the masker's writer put it back.
func echoVendor(t *testing.T, id string, got *[]byte) provider.Provider {
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		*got, _ = io.ReadAll(r.Body)
		ph := regexp.MustCompile(`\{\{[A-Z_]+_[a-z0-9]+\}\}`).Find(*got)
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"msg_1","type":"message","role":"assistant","model":"m9","content":[{"type":"text","text":"echo `+string(ph)+`"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":1}}`)
	}))
	t.Cleanup(up.Close)
	return provider.Provider{ID: id, Name: id, Key: "k", Models: []string{"m9"}, Anthropic: up.URL}
}

// wangbinyq (#3973), beside TestUnmaskedRouteIsNotMasked: a route that
// ends only where the user set requests to go unmasked, a provider alone
// or a group of such, is not masked at all; one that may reach anyone
// else, a vendor fallback or a group member, is masked once, before the
// first try.
func TestUnredactedRouteNotScanned(t *testing.T) {
	const key = "sk-proj-abcdEFGH1234ijklMNOP5678qrst"
	chat := `{"model":"fake/m1","max_tokens":10,"messages":[{"role":"user","content":"my key is ` + key + `"}]}`
	count := `{"model":"fake/m1","messages":[{"role":"user","content":"my key is ` + key + `"}]}`
	vendor := provider.Provider{ID: "vendor", Name: "Vendor", Key: "k", Models: []string{"m9"}, Anthropic: "https://api.vendor.example"}
	cases := []struct {
		name  string
		local func(*provider.Provider)
		group bool
		more  []provider.Provider
		scans int32 // per request
	}{
		{name: "alone", local: func(p *provider.Provider) { p.Unredacted = true }, scans: 0},
		{name: "a group of it alone", local: func(p *provider.Provider) { p.Unredacted = true }, group: true, scans: 0},
		{name: "unset", local: func(p *provider.Provider) {}, scans: 1},
		{name: "falls back to a vendor", local: func(p *provider.Provider) { p.Unredacted = true; p.Fallback = []string{"vendor/m9"} }, more: []provider.Provider{vendor}, scans: 1},
		{name: "a group with a vendor", local: func(p *provider.Provider) { p.Unredacted = true }, group: true, more: []provider.Provider{vendor}, scans: 1},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			f := &fake{ctype: "application/json", reply: `{"id":"msg_1","type":"message","role":"assistant","model":"m1","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":1}}`}
			setup(t, provider.Anthropic, f)
			p, err := provider.Find("fake")
			if err != nil {
				t.Fatal(err)
			}
			tc.local(p)
			for _, q := range append([]provider.Provider{*p}, tc.more...) {
				if err := provider.Save(q); err != nil {
					t.Fatal(err)
				}
			}
			body := chat
			if tc.group {
				members := []string{"fake/m1"}
				if len(tc.more) > 0 {
					members = append(members, "vendor/m9")
				}
				if err := provider.SaveGroup(provider.Group{ID: "mine", Name: "mine", Members: members, Routing: "order"}); err != nil {
					t.Fatal(err)
				}
				body = strings.Replace(chat, "fake/m1", "group/mine", 1)
			}
			if err := settings.Save(settings.Settings{Redact: true}); err != nil {
				t.Fatal(err)
			}
			scans := scanCount(t)
			if code, reply := post(t, "/v1/messages", body); code != 200 {
				t.Fatalf("%d %s", code, reply)
			}
			if n := scans.Load(); n != tc.scans {
				t.Fatalf("scanned %d times, want %d", n, tc.scans)
			}
			if whole := strings.Contains(string(f.got), key); whole != (tc.scans == 0) {
				t.Fatalf("sent whole %v after %d scans: %s", whole, tc.scans, f.got)
			}
			if tc.group {
				return
			}
			scans.Store(0)
			f.reply = `{"input_tokens":17}`
			if code, reply := post(t, "/v1/messages/count_tokens", count); code != 200 || f.path != "/v1/messages/count_tokens" {
				t.Fatalf("count: %d %s to %s", code, reply, f.path)
			}
			if n := scans.Load(); n != tc.scans {
				t.Fatalf("count scanned %d times, want %d", n, tc.scans)
			}
			if whole := strings.Contains(string(f.got), key); whole != (tc.scans == 0) {
				t.Fatalf("counted whole %v: %s", whole, f.got)
			}
		})
	}
}

// A local model set to go unmasked that fails over to a vendor, on its
// own fallback or as a group's first member, sends the vendor the masked
// body: the route was never all unmasked, so it was masked before the
// first try, the local one included.
func TestUnredactedFailoverToVendorMasked(t *testing.T) {
	const key = "sk-proj-abcdEFGH1234ijklMNOP5678qrst"
	chat := `{"model":"fake/m1","max_tokens":10,"messages":[{"role":"user","content":"my key is ` + key + `"}]}`
	for _, group := range []bool{false, true} {
		t.Run(map[bool]string{false: "fallback", true: "group"}[group], func(t *testing.T) {
			f := &fake{ctype: "application/json", code: 500, reply: `{"type":"error","error":{"type":"api_error","message":"down"}}`}
			setup(t, provider.Anthropic, f)
			var got []byte
			vendor := echoVendor(t, "vendor", &got)
			p, err := provider.Find("fake")
			if err != nil {
				t.Fatal(err)
			}
			p.Unredacted = true
			body := chat
			if group {
				if err := provider.SaveGroup(provider.Group{ID: "mine", Name: "mine", Members: []string{"fake/m1", "vendor/m9"}, Routing: "order"}); err != nil {
					t.Fatal(err)
				}
				body = strings.Replace(chat, "fake/m1", "group/mine", 1)
			} else {
				p.Fallback = []string{"vendor/m9"}
			}
			for _, q := range []provider.Provider{*p, vendor} {
				if err := provider.Save(q); err != nil {
					t.Fatal(err)
				}
			}
			if err := settings.Save(settings.Settings{Redact: true}); err != nil {
				t.Fatal(err)
			}
			scans := scanCount(t)
			code, reply := post(t, "/v1/messages", body)
			if code != 200 || f.calls == 0 || len(got) == 0 {
				t.Fatalf("%d %s; local asked %d times, vendor sent %q", code, reply, f.calls, got)
			}
			if strings.Contains(string(f.got), key) || strings.Contains(string(got), key) {
				t.Fatalf("sent unmasked: local %s, vendor %s", f.got, got)
			}
			if !strings.Contains(reply, key) {
				t.Fatalf("the value is not back in the reply: %s", reply)
			}
			if n := scans.Load(); n != 1 {
				t.Fatalf("scanned %d times, want 1", n)
			}
		})
	}
}

// A request that names a model a local provider set to go unmasked serves,
// which routing sends elsewhere (Claude Code's tier, here, stands in a
// vendor's model for the id), is masked by where it goes, not by the name
// it was asked by (#3973), and the reply has the value back.
func TestUnredactedAskedButSentElsewhereMasked(t *testing.T) {
	const key = "sk-proj-abcdEFGH1234ijklMNOP5678qrst"
	f := &fake{ctype: "application/json", reply: `{"id":"msg_1","type":"message","role":"assistant","model":"m1","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":1}}`}
	setup(t, provider.Anthropic, f)
	var got []byte
	vendor := echoVendor(t, "vendor", &got)
	p, err := provider.Find("fake")
	if err != nil {
		t.Fatal(err)
	}
	p.Unredacted = true
	p.Models = []string{"claude-sonnet-4-6"}
	for _, q := range []provider.Provider{*p, vendor} {
		if err := provider.Save(q); err != nil {
			t.Fatal(err)
		}
	}
	if err := settings.Save(settings.Settings{Redact: true}); err != nil {
		t.Fatal(err)
	}
	body := `{"model":"claude-sonnet-4-6","max_tokens":10,"messages":[{"role":"user","content":"my key is ` + key + `"}]}`
	StandIn = func(agent, model string) string { return "vendor/m9" }
	t.Cleanup(func() { StandIn = nil })
	scans := scanCount(t)
	rec := httptest.NewRecorder()
	req := httptest.NewRequest("POST", "/v1/messages", strings.NewReader(body))
	// Claude Code, known by its token (TestClaudeTierStandInByFamily)
	req.Header.Set("User-Agent", "claude-cli/2.1.295 (external, cli)")
	req.Header.Set("x-api-key", TokenFor("claude"))
	New().Handler().ServeHTTP(rec, req)
	if rec.Code != 200 || len(got) == 0 || f.calls != 0 {
		t.Fatalf("%d %s; vendor sent %q, local asked %d times", rec.Code, rec.Body, got, f.calls)
	}
	if strings.Contains(string(got), key) {
		t.Fatalf("the vendor was sent the value: %s", got)
	}
	if !strings.Contains(rec.Body.String(), key) {
		t.Fatalf("the value is not back in the reply: %s", rec.Body)
	}
	if n := scans.Load(); n != 1 {
		t.Fatalf("scanned %d times, want 1", n)
	}
}
