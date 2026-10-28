package gateway

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/yetone/magpie/internal/gwmw"
	"github.com/yetone/magpie/internal/masking"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/usage"
)

// countedMasker is the built-in masker, counting the bodies it is asked
// to mask.
type countedMasker struct {
	m masking.Masker
	n *atomic.Int64
}

func (c countedMasker) JSON(b []byte) ([]byte, int) { c.n.Add(1); return c.m.JSON(b) }
func (c countedMasker) Text(s string) (string, int) { c.n.Add(1); return c.m.Text(s) }
func (c countedMasker) Restore(s string, escaped bool) string {
	return c.m.Restore(s, escaped)
}
func (c countedMasker) Writer(w http.ResponseWriter) (http.ResponseWriter, func()) {
	return c.m.Writer(w)
}

// wangbinyq (#3973): a request that goes only to a local model set to go
// unmasked was masked all the same before it was routed, and the masked
// body thrown away, up to minutes of magpie's own time for each. Now it
// isn't masked at all, a chat or a token count, and the log has it as it
// went; one that goes to a vendor, or an unknown model's, is masked as
// before. With the request archive on it was masked all the same (#3973,
// wangbinyq: 请求归档开启的话, 还是会先走脱敏路径): the archive keeps it as
// it went now. An observer that keeps the bodies is handed them masked,
// masked after the try instead of before it.
func TestUnmaskedRouteIsNotMasked(t *testing.T) {
	const key = "sk-proj-abcdEFGH1234ijklMNOP5678qrst"
	const placeholder = "{{API_KEY_" // the key masked; the archive and an export take it out otherwise
	chat := `{"model":"fake/m1","max_tokens":10,"messages":[{"role":"user","content":"my key is ` + key + `"}]}`
	count := `{"model":"fake/m1","messages":[{"role":"user","content":"my key is ` + key + `"}]}`
	cases := []struct {
		name     string
		set      bool
		observed bool
		archived bool
		masks    bool // the masker is asked
		whole    bool // the provider is sent the key
	}{
		{name: "set to go unmasked", set: true, whole: true},
		{name: "unset", masks: true},
		{name: "an observer keeps the bodies", set: true, observed: true, masks: true, whole: true},
		{name: "the request archive is on", set: true, archived: true, whole: true},
		{name: "the archive is on, unset", archived: true, masks: true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			f := &fake{ctype: "application/json", reply: `{"id":"msg_1","type":"message","role":"assistant","model":"m1","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":1}}`}
			setup(t, provider.Anthropic, f)
			p, err := provider.Find("fake")
			if err != nil {
				t.Fatal(err)
			}
			p.Unredacted = tc.set
			if err := provider.Save(*p); err != nil {
				t.Fatal(err)
			}
			if err := settings.Save(settings.Settings{Redact: true, RequestArchive: tc.archived}); err != nil {
				t.Fatal(err)
			}
			obs := &fakeObserver{watch: true, bodies: usage.CapturedBodies}
			if tc.observed {
				t.Cleanup(usage.Observe(obs))
			}
			bucket := &memBucket{objs: map[string][]byte{}}
			if tc.archived {
				t.Setenv("TMPDIR", t.TempDir())
				archiveTo(t, bucket)
			}
			withoutMasker(t)
			n := new(atomic.Int64)
			t.Cleanup(gwmw.Mask(countedMasker{n: n}))
			s := New()
			rec := httptest.NewRecorder()
			s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/messages", strings.NewReader(chat)))
			if rec.Code != 200 {
				t.Fatalf("%d %s", rec.Code, rec.Body)
			}
			if whole := strings.Contains(string(f.got), key); whole != tc.whole {
				t.Fatalf("sent whole %v, want %v: %s", whole, tc.whole, f.got)
			}
			if masks := n.Load() > 0; masks != tc.masks {
				t.Fatalf("masked %d times, want masking %v", n.Load(), tc.masks)
			}
			c := s.Recent()[0]
			if logged := strings.Contains(c.RequestBody, key); logged != tc.whole {
				t.Fatalf("the log has the key %v, want %v: %s", logged, tc.whole, c.RequestBody)
			}
			if c.RequestBody == "" {
				t.Fatal("the log has no body")
			}
			if tc.observed {
				obs.mu.Lock()
				got := obs.attempts
				obs.mu.Unlock()
				if len(got) == 0 || !strings.Contains(got[0].BodyIn, placeholder) || strings.Contains(got[0].BodyIn, key) {
					t.Fatalf("the observer was handed %+v", got)
				}
			}
			if tc.archived {
				archivePending.Wait()
				var a Archived
				if err := json.Unmarshal(bucket.objs["archive/"+c.Archive+".json"], &a); err != nil {
					t.Fatalf("%v: %v", keys(bucket.objs), err)
				}
				// as it went: masked, or as written save the secrets every
				// archived body has taken out
				if masked := strings.Contains(a.Request.Body, placeholder); masked == tc.whole || a.Request.Body == "" || strings.Contains(a.Request.Body, key) {
					t.Fatalf("the archive kept %s", a.Request.Body)
				}
			}
			n.Store(0)
			f.reply = `{"input_tokens":17}`
			if code, reply := post(t, "/v1/messages/count_tokens", count); code != 200 {
				t.Fatalf("count: %d %s", code, reply)
			}
			if whole := strings.Contains(string(f.got), key); whole != tc.whole {
				t.Fatalf("counted whole %v, want %v: %s", whole, tc.whole, f.got)
			}
			if masks := n.Load() > 0; masks != (tc.masks && !tc.set) {
				t.Fatalf("count masked %d times, want masking %v", n.Load(), tc.masks && !tc.set)
			}
		})
	}
	t.Run("an unknown model's log", func(t *testing.T) {
		f := &fake{ctype: "application/json"}
		setup(t, provider.Anthropic, f)
		if err := settings.Save(settings.Settings{Redact: true}); err != nil {
			t.Fatal(err)
		}
		s := New()
		rec := httptest.NewRecorder()
		s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/messages", strings.NewReader(strings.Replace(chat, "fake/m1", "nobody/m1", 1))))
		if rec.Code != 404 {
			t.Fatalf("%d %s", rec.Code, rec.Body)
		}
		if c := s.Recent()[0]; c.RequestBody == "" || strings.Contains(c.RequestBody, key) {
			t.Fatalf("the log has %q", c.RequestBody)
		}
	})
}
