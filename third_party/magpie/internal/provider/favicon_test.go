package provider

import (
	"context"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func TestFaviconSites(t *testing.T) {
	for base, want := range map[string][]string{
		"https://api.deepseek.com/v1":          {"https://api.deepseek.com", "https://deepseek.com"},
		"https://open.bigmodel.cn/api/paas/v4": {"https://open.bigmodel.cn", "https://bigmodel.cn"},
		"https://example.com:8443/v1":          {"https://example.com:8443", "https://example.com"},
		"https://openrouter.ai/api/v1":         {"https://openrouter.ai"},
	} {
		if got, err := faviconSites(base); err != nil || !reflect.DeepEqual(got, want) {
			t.Errorf("%s: %v %v, want %v", base, got, err, want)
		}
	}
	for _, base := range []string{"", "http://127.0.0.1:11434/v1", "http://localhost:1234", "http://192.168.1.2/v1"} {
		if _, err := faviconSites(base); err == nil {
			t.Errorf("%q: looked for an icon", base)
		}
	}
	// the guard itself refuses a loopback site
	if _, err := FaviconFor(context.Background(), "http://localhost:3425/v1", ""); err == nil {
		t.Error("a local base URL's icon was fetched")
	}
}

func TestFavicon(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))

	png := []byte("\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR")
	svg := []byte(`<svg xmlns="http://www.w3.org/2000/svg"/>`)
	pages := map[string]string{}
	status := map[string]string{}
	mux := http.NewServeMux()
	mux.HandleFunc("/", func(rw http.ResponseWriter, r *http.Request) {
		if st, ok := status[r.Host]; ok && r.URL.Path == "/api/status" {
			rw.Write([]byte(st))
			return
		}
		if p, ok := pages[r.Host]; ok && r.URL.Path == "/" {
			rw.Write([]byte(p))
			return
		}
		switch r.URL.Path {
		case "/logo.png": // a panel's own
			rw.Write(png)
			return
		case "/static/touch.png", "/favicon.ico":
			if strings.HasPrefix(r.Host, "bare.") || r.URL.Path == "/static/touch.png" {
				rw.Write(png)
				return
			}
		case "/logo.svg":
			rw.Write(svg)
			return
		}
		// a site that answers every path with its page, the icon's too
		rw.Write([]byte("<html><body>not found</body></html>"))
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()
	port := srv.URL[strings.LastIndex(srv.URL, ":"):]
	// every site is the test server, told apart by the Host it is asked as
	c := &http.Client{Transport: &http.Transport{DialContext: func(ctx context.Context, network, _ string) (net.Conn, error) {
		var d net.Dialer
		return d.DialContext(ctx, network, srv.Listener.Addr().String())
	}}}
	site := func(h string) string { return "http://" + h + port }
	pass := func(s string) (string, error) { return s, nil }
	stored := func(icon string, want []byte) {
		t.Helper()
		b, err := os.ReadFile(IconFile(strings.TrimPrefix(icon, iconPrefix)))
		if err != nil || string(b) != string(want) {
			t.Errorf("stored %q: %q %v, want %q", icon, b, err, want)
		}
	}

	// an SVG link is taken before the touch icon, whatever the order
	pages["svg.test"+port] = `<head><link rel="apple-touch-icon" href="/static/touch.png"><link rel="icon" type="image/svg+xml" href="/logo.svg"></head>`
	icon, err := favicon(context.Background(), c, []string{site("svg.test")}, "", pass)
	if err != nil {
		t.Fatal(err)
	}
	stored(icon, svg)

	// a relative apple-touch-icon, in single quotes and odd case
	pages["touch.test"+port] = `<HEAD><LINK REL='apple-touch-icon' HREF='static/touch.png'></HEAD>`
	if icon, err = favicon(context.Background(), c, []string{site("touch.test")}, "", pass); err != nil {
		t.Fatal(err)
	}
	stored(icon, png)

	// a data: URI icon (#12)
	pages["data.test"+port] = `<head><link rel="shortcut icon" href="data:image/svg+xml,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%2F%3E"></head>`
	if icon, err = favicon(context.Background(), c, []string{site("data.test")}, "", pass); err != nil {
		t.Fatal(err)
	}
	stored(icon, svg)

	// the API host has nothing; the domain above it has /favicon.ico
	if icon, err = favicon(context.Background(), c, []string{site("api.none"), site("bare.test")}, "", pass); err != nil {
		t.Fatal(err)
	}
	stored(icon, png)

	// a page answered for the icon is not a picture: nothing is found
	if _, err = favicon(context.Background(), c, []string{site("api.none")}, "", pass); err == nil {
		t.Error("an HTML page was kept as the icon")
	}
	// a link in the body is not the page's icon
	pages["body.test"+port] = `<head></head><body><link rel="icon" href="/logo.svg"></body>`
	if _, err = favicon(context.Background(), c, []string{site("body.test")}, "", pass); err == nil {
		t.Error("a link in the body was taken")
	}

	// a new-api panel: its logo is in /api/status, not on its page, whose
	// icon is the panel's own (wiixdede on X)
	newapi := `<head><link rel="icon" href="/logo.png"></head>`
	pages["brand.test"+port] = newapi
	status["brand.test"+port] = `{"success":true,"data":{"system_name":"Foo","logo":"/logo.svg"}}`
	if icon, err = favicon(context.Background(), c, []string{site("brand.test")}, "", pass); err != nil {
		t.Fatal(err)
	}
	stored(icon, svg)

	// one with no logo of its own gets the provider's letters, not the panel's icon
	letters := func(host, name, want string) {
		t.Helper()
		pages[host+port] = newapi
		icon, err := favicon(context.Background(), c, []string{site(host)}, name, pass)
		if err != nil {
			t.Fatal(err)
		}
		b, _ := os.ReadFile(IconFile(strings.TrimPrefix(icon, iconPrefix)))
		if !strings.HasPrefix(string(b), "<svg") || !strings.Contains(string(b), ">"+want+"</text>") {
			t.Errorf("%s %q: %s, want the letters %q", host, name, b, want)
		}
	}
	status["plain.test"+port] = `{"success":true,"data":{"system_name":"New API","logo":""}}`
	letters("plain.test", "Foo Relay", "FR")
	letters("plain.test", "akash", "Ak")
	letters("plain.test", "中转 A", "中")
	letters("plain.test", "", "Pl") // the panel's default name says nothing: the domain's
	status["named.test"+port] = `{"success":true,"data":{"system_name":"Kiwi API"}}`
	letters("named.test", "", "KA")
	// a logo that can't be fetched leaves the letters too
	status["gone.test"+port] = `{"success":true,"data":{"system_name":"x","logo":"/missing.png"}}`
	letters("gone.test", "Gone", "Go")
	// a junk logo, but an icon its page was given by hand (api.v3.cm)
	status["hand.test"+port] = `{"success":true,"data":{"system_name":"V-API","logo":"svg"}}`
	pages["hand.test"+port] = `<head><link rel="icon" href="/static/touch.png"></head>`
	if icon, err = favicon(context.Background(), c, []string{site("hand.test")}, "", pass); err != nil {
		t.Fatal(err)
	}
	stored(icon, png)
	// a site whose /api/status is something else is looked at as before
	status["other.test"+port] = `{"status":"ok"}`
	pages["other.test"+port] = `<head><link rel="icon" href="/logo.svg"></head>`
	if icon, err = favicon(context.Background(), c, []string{site("other.test")}, "Other", pass); err != nil {
		t.Fatal(err)
	}
	stored(icon, svg)
}
