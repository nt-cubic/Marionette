package gateway

import (
	"net"
	"slices"
	"testing"
)

// A computer whose ISP assigns a static public address to its network
// card is reached there as well as at its private one: both are offered,
// private ones first, and the public ones are named. Loopback, link-local
// and IPv6 are not offered.
func TestLANHostsKeepAPublicAddress(t *testing.T) {
	t.Setenv("MAGPIE_PUBLIC_URL", "")
	t.Setenv("MAGPIE_ADDR", "")
	was := hostIPs
	t.Cleanup(func() { hostIPs = was })
	hostIPs = func() []net.IP {
		var out []net.IP
		for _, s := range []string{"203.236.50.70", "10.77.70.2", "100.64.0.1", "169.254.3.4", "127.0.0.2", "fe80::1", "2001:db8::5", "198.18.0.1", "8.8.4.4"} {
			out = append(out, net.ParseIP(s))
		}
		return out
	}
	// private ones first, so the address Settings offers by default is on
	// this network; the public ones after, in the interfaces' order
	want := []string{"10.77.70.2", "100.64.0.1", "203.236.50.70", "8.8.4.4"}
	if got := LANHosts(); !slices.Equal(got, want) {
		t.Fatalf("LANHosts: %v, want %v", got, want)
	}
	var urls []string
	for _, h := range want {
		urls = append(urls, Scheme()+"://"+net.JoinHostPort(h, Port()))
	}
	if got := LANURLs(); !slices.Equal(got, urls) {
		t.Errorf("LANURLs: %v, want %v", got, urls)
	}
	if got := PublicURLs(urls); !slices.Equal(got, urls[2:]) {
		t.Errorf("PublicURLs: %v, want %v", got, urls[2:])
	}
}

// A proxy's TUN interface (Clash, Surge, sing-box fake-ip) holds an
// address in 198.18.0.0/15, which may be listed before the Wi-Fi's. It is
// this computer's alone, so Settings never offers it: with it first,
// "Share on local network" showed http://198.18.0.1 instead of the
// 192.168.110.x other devices reach (coeo91 on Discord). A home network's
// address comes before Tailscale's carrier-grade NAT one.
func TestLANHostsLeaveOutAProxysTUN(t *testing.T) {
	t.Setenv("MAGPIE_PUBLIC_URL", "")
	t.Setenv("MAGPIE_ADDR", "")
	was := hostIPs
	t.Cleanup(func() { hostIPs = was })
	hostIPs = func() []net.IP {
		var out []net.IP
		for _, s := range []string{"198.18.0.1", "100.101.2.3", "198.19.255.1", "192.168.110.23"} {
			out = append(out, net.ParseIP(s))
		}
		return out
	}
	want := []string{"192.168.110.23", "100.101.2.3"}
	if got := LANHosts(); !slices.Equal(got, want) {
		t.Fatalf("LANHosts: %v, want %v", got, want)
	}
	if got := LANURLs(); len(got) == 0 || got[0] != Scheme()+"://192.168.110.23:"+Port() {
		t.Errorf("LANURLs: %v, want 192.168.110.23 first", got)
	}
	// it isn't on the internet either
	if PublicIP(net.ParseIP("198.18.0.1")) {
		t.Error("198.18.0.1 is called an internet address")
	}
}
