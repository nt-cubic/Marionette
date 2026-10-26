package gateway

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/gwmw"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// waitingMasker is masking that waits for the local model, which never
// answers: a pf-serve loading or busy with the requests ahead (#3682).
// let is closed when the test ends; left is closed when a wait let go
// because its request's context ended.
type waitingMasker struct{ let, left chan struct{} }

func (m waitingMasker) wait(ctx context.Context) {
	select {
	case <-ctx.Done():
		close(m.left)
		panic("stopped waiting for the local masking model")
	case <-m.let:
		panic("the test ended")
	}
}

func (m waitingMasker) JSONHitsContext(ctx context.Context, b []byte) ([]byte, []gwmw.Hit) {
	m.wait(ctx)
	return b, nil
}

func (m waitingMasker) TextHitsContext(ctx context.Context, s string) (string, []gwmw.Hit) {
	m.wait(ctx)
	return s, nil
}

func (m waitingMasker) JSONHits(b []byte) ([]byte, []gwmw.Hit) {
	return m.JSONHitsContext(context.Background(), b)
}

func (m waitingMasker) TextHits(s string) (string, []gwmw.Hit) {
	return m.TextHitsContext(context.Background(), s)
}
func (m waitingMasker) JSON(b []byte) ([]byte, int)   { b, _ = m.JSONHits(b); return b, 0 }
func (m waitingMasker) Text(s string) (string, int)   { s, _ = m.TextHits(s); return s, 0 }
func (waitingMasker) Restore(s string, _ bool) string { return s }
func (waitingMasker) Value(string) (string, bool)     { return "", false }
func (waitingMasker) Writer(w http.ResponseWriter) (http.ResponseWriter, func()) {
	return w, func() {}
}

// #3682: with Settings → Privacy's local model on, every request after a
// restart waited for it, and was answered after 2m that magpie "held this
// request without routing it to any provider". Its answer says it waited
// for the local masking model, what the model was doing and how to send
// without it, and the wait lets go when magpie answers, rather than keep
// its place in the model's queue.
func TestHeldForTheMaskingModelSaysSo(t *testing.T) {
	unroutedLimits(t, 50*time.Millisecond, 500*time.Millisecond)
	fresh(t)
	if err := settings.Save(settings.Settings{Redact: true}); err != nil {
		t.Fatal(err)
	}
	if err := provider.Save(provider.Provider{ID: "probe", Name: "Probe", Chat: "http://127.0.0.1:9/v1", Key: "key", Models: []string{"text"}}); err != nil {
		t.Fatal(err)
	}
	on, busy := modelOn, modelBusy
	modelOn, modelBusy = func() bool { return true }, func() string { return "it is still loading" }
	t.Cleanup(func() { modelOn, modelBusy = on, busy })
	withoutMasker(t)
	m := waitingMasker{let: make(chan struct{}), left: make(chan struct{})}
	t.Cleanup(func() { close(m.let) })
	remove := gwmw.Mask(m)
	t.Cleanup(remove)

	s := New()
	srv := httptest.NewServer(s.Handler())
	t.Cleanup(srv.Close)
	c := &http.Client{Timeout: 20 * time.Second}
	resp, err := c.Post(srv.URL+"/v1/chat/completions", "application/json", strings.NewReader(`{"model":"probe/text","messages":[{"role":"user","content":"hi"}]}`))
	if err != nil {
		t.Fatal(err)
	}
	b, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	body := string(b)
	if resp.StatusCode != http.StatusGatewayTimeout || !strings.Contains(body, "waiting for the local masking model") ||
		!strings.Contains(body, "it is still loading") || !strings.Contains(body, "turn it off in Settings") || strings.Contains(body, "without routing") {
		t.Fatalf("held for the model: %d %s", resp.StatusCode, body)
	}
	select {
	case <-m.left:
	case <-time.After(10 * time.Second):
		t.Fatal("the request's wait for the model went on after magpie answered it")
	}
	rs := routesOf(s, "probe/text")
	if len(rs) != 1 || !strings.Contains(rs[0].Error, "waiting for the local masking model") {
		t.Fatalf("its route: %+v", rs)
	}
}

// With the model off, a request held while masked says it was held while
// masking, not that it waited for a model that isn't on.
func TestUnroutedMsgNamesTheModelOnlyWhenItIsOn(t *testing.T) {
	on := modelOn
	modelOn = func() bool { return false }
	t.Cleanup(func() { modelOn = on })
	if m := unroutedMsg(2*time.Minute, true); strings.Contains(m, "model") || !strings.Contains(m, "while masking it") {
		t.Fatalf("model off: %s", m)
	}
	if m := unroutedMsg(2*time.Minute, false); !strings.Contains(m, "without routing it to any provider") {
		t.Fatalf("not masking: %s", m)
	}
}
