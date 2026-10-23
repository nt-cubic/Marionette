package settings

import "testing"

// The remembered custom proxy holds the proxy's login, so it is this
// computer's own as the proxy is: a sync or a restored backup keeps the
// one here and never brings another computer's.
func TestProxyCustomOwn(t *testing.T) {
	from := Settings{Theme: "dark", Proxy: "direct", ProxyCustom: "socks5://a:b@there:1080"}
	from.KeepOwn(Settings{ProxyCustom: "socks5://c:d@here:1080"})
	if from.ProxyCustom != "socks5://c:d@here:1080" || from.Theme != "dark" {
		t.Fatalf("KeepOwn: %+v", from)
	}
	from.KeepOwn(Settings{})
	if from.ProxyCustom != "" {
		t.Fatal("another computer's proxy login taken here")
	}
}
