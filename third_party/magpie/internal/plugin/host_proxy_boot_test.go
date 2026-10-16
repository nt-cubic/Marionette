package plugin

import (
	"testing"
	"time"
)

// #1518: a host started at login before the proxy app had set the
// system's proxy asks again within seconds, not after the 15 a host with
// a proxy waits: until then every request a plugin made went direct.
func TestHostWithNoProxyLooksAgainSoon(t *testing.T) {
	for _, k := range []string{"HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY", "https_proxy", "http_proxy", "all_proxy"} {
		t.Setenv(k, "")
	}
	h := &host{proxy: hostProxy(env()), proxyAt: time.Now().Add(-3 * time.Second)}
	if proxied(h.proxy) {
		t.Skip("this machine's system proxy is set; the case needs none")
	}
	t.Setenv("HTTPS_PROXY", "http://127.0.0.1:7891")
	if !h.proxyMoved() {
		t.Fatal("the proxy was set 3s after a host with none started, and the host kept going direct")
	}
	// A host with a proxy keeps its 15 seconds.
	h = &host{proxy: hostProxy(env()), proxyAt: time.Now().Add(-3 * time.Second)}
	t.Setenv("HTTPS_PROXY", "http://127.0.0.1:7892")
	if h.proxyMoved() {
		t.Fatal("a host with a proxy looked again before its 15 seconds")
	}
}
