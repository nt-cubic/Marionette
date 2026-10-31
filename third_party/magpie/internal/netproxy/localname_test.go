package netproxy

import (
	"context"
	"net"
	"sync/atomic"
	"testing"
)

// lookups answers names from m, several addresses each, and counts the
// lookups made.
func lookups(t *testing.T, m map[string][]string) *atomic.Int32 {
	t.Helper()
	n := new(atomic.Int32)
	t.Cleanup(SetLookup(func(_ context.Context, host string) ([]net.IPAddr, error) {
		n.Add(1)
		ips, ok := m[host]
		if !ok {
			return nil, &net.DNSError{Err: "no such host", Name: host, IsNotFound: true}
		}
		var out []net.IPAddr
		for _, s := range ips {
			out = append(out, net.IPAddr{IP: net.ParseIP(s)})
		}
		return out, nil
	}))
	return n
}

// AcMilan on Discord: a name of the user's own DNS counts as local when
// every address it resolves to is loopback, private or link-local; one
// resolving to a public address, to a public and a private one, or not at
// all, is not.
func TestLocalName(t *testing.T) {
	lookups(t, map[string][]string{
		"llm.corp.lan":    {"192.168.1.20"},
		"gpu.home.arpa":   {"10.0.0.7", "fd00::7"},
		"loop.test":       {"127.0.0.1"},
		"linklocal.lan":   {"169.254.10.2"},
		"public.example":  {"203.0.113.7"},
		"mixed.example":   {"10.0.0.2", "203.0.113.7"},
		"cgnat.example":   {"100.101.102.103"},
		"nothing.example": {},
	})
	for host, want := range map[string]bool{
		"llm.corp.lan": true, "LLM.corp.lan.": true, "gpu.home.arpa": true, "loop.test": true, "linklocal.lan": true,
		"public.example": false, "mixed.example": false, "cgnat.example": false, "nothing.example": false,
		"gone.lan": false, "192.168.1.20": false, "": false,
	} {
		if got := LocalName(host); got != want {
			t.Errorf("%q: %v, want %v", host, got, want)
		}
	}
}

// A lookup that doesn't answer in lanWait is unknown, so not local.
func TestLocalNameSlowLookupIsNot(t *testing.T) {
	t.Cleanup(SetLookup(func(ctx context.Context, host string) ([]net.IPAddr, error) {
		<-ctx.Done()
		return nil, ctx.Err()
	}))
	if LocalName("slow.lan") {
		t.Fatal("a lookup that timed out counted as local")
	}
}
