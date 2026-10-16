package gateway

import (
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// ownIPv4 is an IPv4 address of one of this computer's interfaces that
// isn't loopback, "" when it has none.
func ownIPv4() string {
	ifs, _ := net.Interfaces()
	for _, i := range ifs {
		if i.Flags&net.FlagUp == 0 || i.Flags&net.FlagLoopback != 0 {
			continue
		}
		addrs, _ := i.Addrs()
		for _, a := range addrs {
			if n, ok := a.(*net.IPNet); ok && n.IP.To4() != nil && !n.IP.IsLoopback() && !n.IP.IsLinkLocalUnicast() {
				return n.IP.String()
			}
		}
	}
	return ""
}

// #1546 (jiayeZhu): with MAGPIE_ADDR on one of this computer's addresses
// that isn't loopback (a LAN, Tailscale or loopback-alias IP) and gateway
// keys on, the MCP helper of a Claude Code the subscription bridge runs
// called back to that address without a key, lanGuard answered 401, its
// tool call never reached the run, and the turn with the tool's results
// found no run waiting: a new Claude Code was told the whole conversation,
// its prompt cache written anew. The gateway here listens on such an
// address behind lanGuard, as the real one does, and the run's helper calls
// it from there.
func TestClaudeToolCallbackOnANonLoopbackAddress(t *testing.T) {
	ip := ownIPv4()
	if ip == "" {
		t.Skip("this computer has no address beyond loopback")
	}
	fresh(t)
	log := takebackClaude(t)
	ln, err := net.Listen("tcp", net.JoinHostPort(ip, "0"))
	if err != nil {
		t.Skip("can't listen on", ip, err)
	}
	s := New()
	srv := httptest.NewUnstartedServer(lanGuard(s.Handler()))
	srv.Listener.Close()
	srv.Listener = ln
	srv.Start()
	t.Cleanup(srv.Close)
	t.Cleanup(s.subscription.abortAll) // before srv.Close: a run's MCP call holds a request open
	t.Setenv("MAGPIE_ADDR", ln.Addr().String())
	t.Setenv("MAGPIE_TRUST_PROXY", "")
	newCaller(t, "Host") // gateway keys on, and shared
	if !OnNetwork() {
		t.Fatal("MAGPIE_ADDR", ln.Addr(), "isn't on the network")
	}
	h := &rewindHarness{t: t, s: s, log: log,
		p: provider.Provider{ID: "claude", Account: &provider.Account{Agent: "claude", User: "u"}}}

	conv := rmsg("user", "CALL the read tool")
	body := h.body(`[` + conv + `]`)
	rec := httptest.NewRecorder()
	var u Usage
	if code, msg := s.serveClaudeSubscription(rec, httptest.NewRequest("POST", "/v1/messages", strings.NewReader(body)), provider.Anthropic, h.p, "claude-sonnet-5", []byte(body), &u); code != 200 {
		t.Fatalf("%d %s", code, msg)
	}
	var res struct {
		Content []struct{ Type, ID string } `json:"content"`
	}
	json.Unmarshal(rec.Body.Bytes(), &res)
	if len(res.Content) == 0 || res.Content[len(res.Content)-1].Type != "tool_use" {
		t.Fatalf("no call: %s\n%s", rec.Body, h.read())
	}
	id := res.Content[len(res.Content)-1].ID
	for deadline := time.Now().Add(10 * time.Second); ; {
		s.subscription.mu.Lock()
		waits := s.subscription.calls[id] != nil
		s.subscription.mu.Unlock()
		if waits {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("the helper's call %s never reached the run:\n%s", id, h.read())
		}
		time.Sleep(5 * time.Millisecond)
	}
	pid := strings.TrimPrefix(strings.Split(id, "_")[1], "T")
	conv += `,{"role":"assistant","content":[{"type":"tool_use","id":"` + id + `","name":"read","input":{}}]}`
	conv += `,{"role":"user","content":[{"type":"tool_result","tool_use_id":"` + id + `","content":"file body"}]}`
	if got := h.ask(`[` + conv + `]`); got != "pid "+pid+" turn 1 read" {
		t.Fatalf("the tool's results went to another Claude Code: %q (the call's was %s)\n%s", got, pid, h.read())
	}
}

// What lets the helper in is the callback token of a run now live, on its
// own path, by POST, and nothing else: from another machine, a wrong,
// spent or missing token, another method or another path is still asked
// for a gateway key (401), and so is every other route while a token is
// live.
func TestOnlyALiveRunsCallbackSkipsTheKey(t *testing.T) {
	fresh(t)
	t.Setenv("MAGPIE_ADDR", "0.0.0.0:3425")
	t.Setenv("MAGPIE_TRUST_PROXY", "")
	if err := settings.Save(settings.Settings{LAN: false}); err != nil {
		t.Fatal(err)
	}
	newCaller(t, "Host")
	h := lanGuard(New().Handler())
	token := randomToken()
	addCallback(token)
	t.Cleanup(func() { dropCallback(token) })
	call := func(method, path string, hdr ...string) (int, string) {
		r := httptest.NewRequest(method, path, strings.NewReader(`{"tool_call_id":"x","name":"read","arguments":{}}`))
		r.RemoteAddr = "192.168.1.9:5000"
		for i := 0; i+1 < len(hdr); i += 2 {
			r.Header.Set(hdr[i], hdr[i+1])
		}
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		return w.Code, w.Body.String()
	}
	// past the guard: this server has no such run, so mcpCall says so
	if c, b := call("POST", claudeMCPPath+token); c != http.StatusNotFound || !strings.Contains(b, "unknown or expired Claude run") {
		t.Fatalf("the live token: %d %s", c, b)
	}
	for _, p := range [][2]string{
		{"POST", claudeMCPPath + randomToken()},
		{"POST", claudeMCPPath + token[:len(token)-1]},
		{"POST", claudeMCPPath + token + "0"},
		{"POST", claudeMCPPath},
		{"POST", claudeMCPPath + token + "/x"},
		{"POST", claudeMCPPath + fmt.Sprintf("%%%02X", token[0]) + token[1:]},
		{"POST", "/x" + claudeMCPPath + token},
		{"GET", claudeMCPPath + token},
		{"POST", "/v1/messages"}, {"POST", "/v1/chat/completions"}, {"GET", "/v1/models"},
		{"GET", "/v1/magpie/quotas"}, {"POST", "/mcp/neon"},
	} {
		if c, b := call(p[0], p[1]); c != http.StatusUnauthorized {
			t.Errorf("%s %s: %d %.120s", p[0], p[1], c, b)
		}
		if c, b := call(p[0], p[1], "Authorization", "Bearer "+token); c != http.StatusUnauthorized {
			t.Errorf("%s %s with the token as a key: %d %.120s", p[0], p[1], c, b)
		}
	}
	// a run let go takes its token with it
	dropCallback(token)
	if c, b := call("POST", claudeMCPPath+token); c != http.StatusUnauthorized {
		t.Fatalf("a spent token: %d %s", c, b)
	}
}
