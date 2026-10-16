package cli

import "testing"

// magpie web on loopback behind a reverse proxy (#1479) prints the proxy's
// link with the key when told the proxy's address, so nobody joins
// "/?k=…" onto it by hand: MAGPIE_WEB_URL, else MAGPIE_PUBLIC_URL with no
// port (one with a port isn't a proxy). None is printed without either.
func TestWebProxyLink(t *testing.T) {
	key := "/?k=0123456789abcdef"
	for _, c := range []struct{ env, want string }{
		{"https://magpie.example.com", "https://magpie.example.com/?k=0123456789abcdef"},
		{"https://magpie.example.com/", "https://magpie.example.com/?k=0123456789abcdef"},
		{"magpie.example.com", "http://magpie.example.com/?k=0123456789abcdef"},
		{"https://example.com/magpie/", "https://example.com/magpie/?k=0123456789abcdef"},
		{"http://192.168.1.20:3425", ""},
		{"", ""},
	} {
		t.Setenv("MAGPIE_PUBLIC_URL", c.env)
		if got := proxyLink(key); got != c.want {
			t.Errorf("MAGPIE_PUBLIC_URL=%q: %q, want %q", c.env, got, c.want)
		}
	}
	// the page's own proxy address, when the gateway's is elsewhere
	t.Setenv("MAGPIE_PUBLIC_URL", "https://api.example.com")
	for _, c := range []struct{ env, want string }{
		{"https://magpie.example.com/", "https://magpie.example.com/?k=0123456789abcdef"},
		{"magpie.example.com:8443", "https://magpie.example.com:8443/?k=0123456789abcdef"},
		{"https://magpie.example.com/?x=1", ""},
		{"ftp://magpie.example.com", ""},
	} {
		t.Setenv("MAGPIE_WEB_URL", c.env)
		if got := proxyLink(key); got != c.want {
			t.Errorf("MAGPIE_WEB_URL=%q: %q, want %q", c.env, got, c.want)
		}
	}
}
