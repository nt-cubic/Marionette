package gui

import (
	"net"
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/gateway"
)

// magpie web behind a reverse proxy (#372): MAGPIE_PUBLIC_URL with no port
// of its own is where the page is, so the link printed for other machines
// is it, scheme, path and all — not http://<its host>:<the page's port>,
// which nothing answers. One with a port names the host the container's
// ports are published on, and the page is there on its own port.
func TestNetworkLinksPublicURL(t *testing.T) {
	key := "/?k=0123456789abcdef"
	for _, c := range []struct{ env, want string }{
		{"https://magpie.example.com", "https://magpie.example.com/?k=0123456789abcdef"},
		{"https://magpie.example.com/", "https://magpie.example.com/?k=0123456789abcdef"},
		{"https://nas.example.com/magpie/", "https://nas.example.com/magpie/?k=0123456789abcdef"},
		{"http://192.168.1.20:3425", "http://192.168.1.20:3430/?k=0123456789abcdef"},
		{"nas.local:13425", "http://nas.local:3430/?k=0123456789abcdef"},
	} {
		t.Setenv("MAGPIE_PUBLIC_URL", c.env)
		if got := NetworkLinks("3430", key); !slices.Equal(got, []string{c.want}) {
			t.Errorf("MAGPIE_PUBLIC_URL=%q: %v, want %s", c.env, got, c.want)
		}
	}
}

// The links printed for other machines are at the gateway's own addresses
// (gateway.LANHosts), one for one: Settings showed a computer's static
// public address under the gateway and not under the page.
func TestNetworkLinksAtTheGatewaysHosts(t *testing.T) {
	t.Setenv("MAGPIE_PUBLIC_URL", "")
	var want []string
	for _, h := range gateway.LANHosts() {
		want = append(want, "http://"+net.JoinHostPort(h, "3430")+"/?k=k")
	}
	if got := NetworkLinks("3430", "/?k=k"); !slices.Equal(got, want) {
		t.Errorf("%v, want %v", got, want)
	}
}
