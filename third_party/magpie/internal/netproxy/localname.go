package netproxy

import (
	"context"
	"net"
	"strings"
	"sync"
	"time"
)

// LocalName says whether a name of the user's own DNS
// (http://llm.corp.lan:8000, AcMilan on Discord) resolves only to this
// computer or the local network. The provider editor asks it
// (/api/local-hosts) to tell whether "Send requests unmasked" needs its
// caution; no request is routed or dialled by it.

// localKeep is how long what LocalName found for a name is kept.
const localKeep = time.Minute

var localCache = struct {
	sync.Mutex
	m map[string]lanSeen
}{m: map[string]lanSeen{}}

// LocalName reports whether host, a name, resolves (in lanWait) to at
// least one address and every address it resolves to is on this computer
// or the local network (LocalIP). A name that doesn't resolve, or doesn't
// in time, is not: what can't be read is unknown, and unknown is masked.
func LocalName(host string) bool {
	host = strings.ToLower(strings.TrimSuffix(host, "."))
	if host == "" || net.ParseIP(host) != nil {
		return false
	}
	localCache.Lock()
	s, ok := localCache.m[host]
	localCache.Unlock()
	if ok && time.Since(s.at) < localKeep {
		return s.lan
	}
	ctx, cancel := context.WithTimeout(context.Background(), lanWait)
	addrs, err := lanLookup(ctx, host)
	cancel()
	local := err == nil && allLocal(addrs)
	localCache.Lock()
	if len(localCache.m) >= 256 {
		clear(localCache.m)
	}
	localCache.m[host] = lanSeen{lan: local, at: time.Now()}
	localCache.Unlock()
	return local
}

// LocalIP says ip is on this computer or the local network: a loopback,
// private (10/8, 172.16/12, 192.168/16, fc00::/7) or link-local address.
// Carrier-grade NAT (100.64/10) is not: it is a Tailscale address, but
// also an ISP's.
func LocalIP(ip net.IP) bool {
	return ip.IsLoopback() || ip.IsPrivate() || ip.IsLinkLocalUnicast()
}

func allLocal(addrs []net.IPAddr) bool {
	for _, a := range addrs {
		if !LocalIP(a.IP) {
			return false
		}
	}
	return len(addrs) > 0
}

// SetLookup has names resolved by f, for a test that must not reach the
// network, and forgets what earlier lookups found; restore puts the
// system's resolver back.
func SetLookup(f func(ctx context.Context, host string) ([]net.IPAddr, error)) (restore func()) {
	old := lanLookup
	lanLookup = f
	forget := func() {
		lanCache.Lock()
		clear(lanCache.m)
		lanCache.Unlock()
		localCache.Lock()
		clear(localCache.m)
		localCache.Unlock()
	}
	forget()
	return func() { lanLookup = old; forget() }
}
