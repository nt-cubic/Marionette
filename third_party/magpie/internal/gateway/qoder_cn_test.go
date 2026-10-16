package gateway

import (
	"context"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/qoder"
)

// TestQoderCNGateway: a Qoder CN account is served through the gateway like
// the global Qoder's, its credential asked for as Qoder CN's and its chat sent
// to Qoder CN's inference host, gateway.qoder.com.cn; a global account still
// goes to api3.qoder.sh.
func TestQoderCNGateway(t *testing.T) {
	real := qoderURL
	done := qoderUpstream(t, []string{qoderChunk(qoderContent(t, "ok"))})
	defer done()
	up, _ := url.Parse(qoderURL(nil))
	qoderURL = real
	for _, tt := range []struct{ agent, site, host string }{
		{provider.QoderCNID, qoder.CNProviderKey, "gateway.qoder.com.cn"},
		{"qoder", "", "api3.qoder.sh"},
	} {
		var asked []string
		qoderAuth = func(_ context.Context, agent, user string) (*qoder.Credential, error) {
			asked = append(asked, agent+" "+user)
			return &qoder.Credential{UID: "uid-test", Token: "jt-test", MachineID: "machine-test", Site: tt.site}, nil
		}
		qoderModel = func(_ context.Context, agent, _, _ string) (qoder.ModelInfo, error) {
			asked = append(asked, "model "+agent)
			return testQoderModel(t), nil
		}
		var host, path string
		s := New()
		s.client = &http.Client{Transport: qoderRoundTrip(func(r *http.Request) (*http.Response, error) {
			host, path = r.URL.Host, r.URL.Path
			r.URL.Scheme, r.URL.Host = up.Scheme, up.Host
			return http.DefaultTransport.RoundTrip(r)
		})}
		p := provider.Provider{ID: tt.agent, Account: &provider.Account{Agent: tt.agent, User: "one@x"}}
		body := `{"model":"qfmodel","messages":[{"role":"user","content":"hi"}]}`
		w := httptest.NewRecorder()
		var call Call
		status, msg := s.attempt(w, httptest.NewRequest("POST", "/", strings.NewReader(body)), provider.Chat, p, "qfmodel", []byte(body), &call)
		if status != 200 || !strings.Contains(w.Body.String(), "ok") {
			t.Fatalf("%s: %d %s %s", tt.agent, status, msg, w.Body.String())
		}
		if host != tt.host || path != strings.Split(qoder.ChatPath, "?")[0] {
			t.Errorf("%s: chat went to %s%s, want %s", tt.agent, host, path, tt.host)
		}
		if len(asked) != 2 || asked[0] != tt.agent+" one@x" || asked[1] != "model "+tt.agent {
			t.Errorf("%s: asked %v", tt.agent, asked)
		}
	}
}

type qoderRoundTrip func(*http.Request) (*http.Response, error)

func (f qoderRoundTrip) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }
