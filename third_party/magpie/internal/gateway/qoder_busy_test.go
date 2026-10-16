package gateway

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/tidwall/gjson"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/qoder"
)

// plugins #98: Qoder's queue refusing a chat (code 10605) is Qoder being
// busy for the plan, not a lapsed sign-in. The body is the one Qoder gave a
// real trial account on 2026-10-12, byte for byte from the issue
// (testdata/qoder_queued_10605.json). The other codes are read as Qoder's
// own client reads them (QoderErrorCode in Qoder.app's qoder-agent-sdk
// worker), as the plugin reads them since 0.2.14: only 105 is loginExpired.

func qoderQueuedBody(t *testing.T) string {
	t.Helper()
	b, err := os.ReadFile("testdata/qoder_queued_10605.json")
	if err != nil {
		t.Fatal(err)
	}
	return strings.TrimSpace(string(b))
}

// qoderRefusing stands in for api3.qoder.sh answering every chat with
// status and body, as qoderUpstream does with a stream.
func qoderRefusing(t *testing.T, status int, body string) {
	t.Helper()
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.Copy(io.Discard, r.Body)
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(status)
		io.WriteString(w, body)
	}))
	auth, url, model := qoderAuth, qoderURL, qoderModel
	qoderAuth = func(context.Context, string, string) (*qoder.Credential, error) {
		return &qoder.Credential{UID: "uid-test", Token: "jt-test", MachineID: "machine-test"}, nil
	}
	qoderModel = func(context.Context, string, string, string) (qoder.ModelInfo, error) { return testQoderModel(t), nil }
	qoderURL = func(*qoder.Credential) string { return up.URL }
	t.Cleanup(func() { up.Close(); qoderAuth, qoderURL, qoderModel = auth, url, model })
}

const qoderBusyWant = "Qoder is busy for efficient (this plan's queue: p3); try again in 30s"

// serveQoderOnce sends one chat through serveQoder, as the gateway does.
func serveQoderOnce(t *testing.T, stream bool) *httptest.ResponseRecorder {
	t.Helper()
	body := `{"model":"qfmodel","stream":` + map[bool]string{true: "true", false: "false"}[stream] + `,"messages":[{"role":"user","content":"hi"}]}`
	w := httptest.NewRecorder()
	r := httptest.NewRequest("POST", "/", strings.NewReader(body))
	var u Usage
	s := New()
	s.serveQoder(w, r, provider.Chat, provider.Provider{Account: &provider.Account{Agent: "qoder", User: "one@x"}}, "qfmodel", []byte(body), &u)
	return w
}

// checkQoderBusy: a 503 with Qoder's own wait as Retry-After, worded as
// busy, never "sign in again" — and the gateway's busy handling takes it
// as busy (asked again after the pause) and waits its Retry-After.
func checkQoderBusy(t *testing.T, w *httptest.ResponseRecorder) {
	t.Helper()
	msg := gjson.Get(w.Body.String(), "error.message").String()
	if w.Code != 503 || w.Header().Get("Retry-After") != "30" || msg != qoderBusyWant {
		t.Fatalf("got %d Retry-After %q %q, want 503 Retry-After 30 %q", w.Code, w.Header().Get("Retry-After"), msg, qoderBusyWant)
	}
	if strings.Contains(strings.ToLower(w.Body.String()), "sign in") {
		t.Fatalf("a queued request asks to sign in again: %s", w.Body.String())
	}
	if !busy(w.Code, w.Body.Bytes()) {
		t.Fatalf("the gateway doesn't take it as busy (failure %q): %s", failure(w.Code, w.Body.Bytes()), w.Body.String())
	}
	if d := retryAfter(w.Header(), time.Now()); d != 30*time.Second {
		t.Fatalf("the gateway waits %s, want Qoder's 30s", d)
	}
}

func TestQoderQueuedIsBusy(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	queued := qoderQueuedBody(t)

	t.Run("http 403", func(t *testing.T) {
		qoderRefusing(t, 403, queued)
		for _, stream := range []bool{false, true} {
			checkQoderBusy(t, serveQoderOnce(t, stream))
		}
	})

	t.Run("in the stream before any answer", func(t *testing.T) {
		env, _ := json.Marshal(map[string]any{"headers": map[string]any{}, "body": queued, "statusCodeValue": 403, "statusCode": "FORBIDDEN"})
		done := qoderUpstream(t, []string{string(env)})
		defer done()
		for _, stream := range []bool{false, true} {
			checkQoderBusy(t, serveQoderOnce(t, stream))
		}
	})
}

// The queue's own wait is used, rounded up; a queued request says so; with
// no wait it is 30s; and a queue state under another status is the same.
func TestQoderQueueWait(t *testing.T) {
	q := func(status int, code string, queue map[string]any) (int, string, time.Duration) {
		m, _ := json.Marshal(queue)
		b, _ := json.Marshal(map[string]any{"code": code, "message": string(m)})
		return qoderFailure(status, b)
	}
	for _, tt := range []struct {
		name   string
		status int
		code   string
		queue  map[string]any
		want   string
		wait   time.Duration
	}{
		{"queued", 403, "10605", map[string]any{"isQueued": true, "modelKey": "ultimate", "retryAfterSeconds": 12.2}, "Qoder queued the request for ultimate; try again in 13s", 13 * time.Second},
		{"no wait", 403, "10605", map[string]any{"serviceAvailable": false}, "Qoder is busy; try again in 30s", 30 * time.Second},
		{"queue state on a 429, no code", 429, "", map[string]any{"serviceAvailable": false, "retryAfterSeconds": 5}, "Qoder is busy; try again in 5s", 5 * time.Second},
	} {
		status, msg, wait := q(tt.status, tt.code, tt.queue)
		if status != 503 || msg != tt.want || wait != tt.wait {
			t.Errorf("%s: %d %q %s, want 503 %q %s", tt.name, status, msg, wait, tt.want, tt.wait)
		}
	}
	// the code alone, the message plain text; a numeric code too
	for _, b := range []string{`{"code":"10605","message":"busy"}`, `{"code":10605,"message":"busy"}`} {
		if status, _, wait := qoderFailure(403, []byte(b)); status != 503 || wait != 30*time.Second {
			t.Errorf("%s: %d %s, want 503 30s", b, status, wait)
		}
	}
}

// Qoder's other codes on a 401/403 aren't a lapsed sign-in either; 105
// and no code are.
func TestQoderErrorCodes(t *testing.T) {
	f := func(status int, code, message string) (int, string, time.Duration) {
		b, _ := json.Marshal(map[string]any{"code": code, "message": message})
		return qoderFailure(status, b)
	}
	for _, status := range []int{401, 403} {
		for _, code := range []string{"110", "112", "113", "114", "115", "116", "117", "118", "119", "122"} {
			if got, msg, _ := f(status, code, "credits used up"); got != 429 || msg != "usage limit reached: credits used up" {
				t.Errorf("%d code %s: %d %q, want 429", status, code, got, msg)
			}
		}
		for _, code := range []string{"406", "416", "430", "80411", "80412", "100400", "100401", "100403"} {
			if got, msg, _ := f(status, code, "no"); got != 400 || msg != "Qoder refused the request ("+code+"): no" {
				t.Errorf("%d code %s: %d %q, want 400", status, code, got, msg)
			}
		}
		for _, b := range []string{`{"code":"105","message":"login expired"}`, `{"message":"forbidden"}`, ``} {
			if got, msg, wait := qoderFailure(status, []byte(b)); got != 401 || msg != "Qoder: the sign-in lapsed — sign in again" || wait != 0 {
				t.Errorf("%d %s: %d %q %s, want the 401", status, b, got, msg, wait)
			}
		}
	}
	// a quota without a code reads as before
	if got, _, _ := qoderFailure(429, []byte(`{"message":"额度已耗尽"}`)); got != 429 {
		t.Errorf("a 429: %d", got)
	}
}
