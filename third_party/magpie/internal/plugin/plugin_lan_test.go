package plugin

import (
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"
	"fmt"
	"io"
	"math/big"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"
)

// lanAddr is a private IPv4 address of this computer: one on the user's
// own network, as a Remote magpie or a NAS is.
func lanAddr(t *testing.T) net.IP {
	t.Helper()
	addrs, _ := net.InterfaceAddrs()
	for _, a := range addrs {
		if n, ok := a.(*net.IPNet); ok && n.IP.To4() != nil && n.IP.IsPrivate() {
			return n.IP
		}
	}
	t.Skip("no private IPv4 address on this computer")
	return nil
}

// askedProxy is an HTTP proxy that keeps the target of every request it
// is asked: an absolute-form one (http://) is answered "proxy <target>",
// a CONNECT (https://, HTTP/2) is turned away with 502.
func askedProxy(t *testing.T) (string, func() []string) {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	var mu sync.Mutex
	var asked []string
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			go func(c net.Conn) {
				defer c.Close()
				var head []byte
				b := make([]byte, 1)
				for !strings.HasSuffix(string(head), "\r\n\r\n") {
					if _, err := c.Read(b); err != nil {
						return
					}
					head = append(head, b[0])
				}
				f := strings.Fields(string(head))
				if len(f) < 2 {
					return
				}
				mu.Lock()
				asked = append(asked, f[1])
				mu.Unlock()
				if f[0] == "CONNECT" {
					fmt.Fprint(c, "HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
					return
				}
				body := "proxy " + f[1]
				fmt.Fprintf(c, "HTTP/1.1 200 OK\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s", len(body), body)
			}(c)
		}
	}()
	return "http://" + ln.Addr().String(), func() []string {
		mu.Lock()
		defer mu.Unlock()
		return slices.Clone(asked)
	}
}

// certFor is a certificate for ip, and the PEM of it a client trusts.
func certFor(t *testing.T, ip net.IP) (tls.Certificate, []byte) {
	t.Helper()
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	tmpl := &x509.Certificate{
		SerialNumber:          big.NewInt(1),
		Subject:               pkix.Name{CommonName: ip.String()},
		IPAddresses:           []net.IP{ip},
		NotBefore:             time.Now().Add(-time.Hour),
		NotAfter:              time.Now().Add(time.Hour),
		KeyUsage:              x509.KeyUsageDigitalSignature | x509.KeyUsageCertSign,
		ExtKeyUsage:           []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		BasicConstraintsValid: true,
		IsCA:                  true,
	}
	der, err := x509.CreateCertificate(rand.Reader, tmpl, tmpl, &key.PublicKey, key)
	if err != nil {
		t.Fatal(err)
	}
	return tls.Certificate{Certificate: [][]byte{der}, PrivateKey: key}, pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der})
}

// #1572: a plugin's requests to a host on the user's own network — a
// Remote magpie, an LLM relay on the NAS — went through the proxy magpie
// gave the host, which can't reach it (502 Bad Gateway), while magpie's
// own requests leave such a host direct (netproxy.onLAN, #982). The
// plugin's fetches, http:// (sent to the proxy in absolute form) and
// https:// (a CONNECT), and its HTTP/2 sessions now go direct too; a
// vendor's still go through the proxy.
func TestPluginLeavesTheUserNetworkDirect(t *testing.T) {
	ip := lanAddr(t)
	sandbox(t)
	for _, k := range []string{"NO_PROXY", "no_proxy", "ALL_PROXY", "all_proxy", "https_proxy", "http_proxy", "FAKE_H2"} {
		t.Setenv(k, "")
	}
	proxy, asked := askedProxy(t)
	t.Setenv("HTTPS_PROXY", proxy)
	t.Setenv("HTTP_PROXY", proxy)

	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprintf(w, "lan %s %s", r.Proto, r.URL.Path)
	})
	listen := func() net.Listener {
		ln, err := net.Listen("tcp", net.JoinHostPort(ip.String(), "0"))
		if err != nil {
			t.Skip("can't listen on", ip, err)
		}
		return ln
	}
	plain := httptest.NewUnstartedServer(handler)
	plain.Listener.Close()
	plain.Listener = listen()
	plain.Start()
	defer plain.Close()
	cert, caPEM := certFor(t, ip)
	secure := httptest.NewUnstartedServer(handler)
	secure.Listener.Close()
	secure.Listener = listen()
	secure.EnableHTTP2 = true
	secure.TLS = &tls.Config{Certificates: []tls.Certificate{cert}}
	secure.StartTLS()
	defer secure.Close()
	// a name the computer resolves to itself: what decides is what it
	// resolves to, as for a NAS's name on the router's DNS
	local := httptest.NewServer(handler)
	defer local.Close()
	ca := filepath.Join(t.TempDir(), "lan-ca.pem")
	if err := os.WriteFile(ca, caPEM, 0o644); err != nil {
		t.Fatal(err)
	}
	t.Setenv("NODE_EXTRA_CA_CERTS", ca)

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("testdata/fake/index.js")
	if _, err := Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if _, err := Providers(ctx); err != nil {
		t.Fatal(err)
	}
	if _, err := APIKey(ctx, "fakeco", 0, nil, "k1", NewAccount); err != nil {
		t.Fatal(err)
	}
	fetch := func(url string) (int, string) {
		fctx, cancel := context.WithTimeout(ctx, 30*time.Second)
		defer cancel()
		res, err := Fetch(fctx, FetchRequest{Provider: "fakeco", Model: "fake-1", NPM: "@ai-sdk/openai-compatible",
			URL: url, Method: "POST", Body: []byte(`{}`)})
		if err != nil {
			return 0, err.Error()
		}
		defer res.Body.Close()
		b, _ := io.ReadAll(res.Body)
		return res.StatusCode, string(b)
	}
	proxied := func(host string) bool {
		for _, a := range asked() {
			if strings.Contains(a, host) {
				return true
			}
		}
		return false
	}

	_, port, _ := net.SplitHostPort(local.Listener.Addr().String())
	for _, url := range []string{
		plain.URL + "/v1/chat/completions",
		secure.URL + "/v1/chat/completions",
		"http://localhost.:" + port + "/v1/chat/completions",
	} {
		code, b := fetch(url)
		if code != 200 || b != "lan HTTP/1.1 /v1/chat/completions" {
			t.Errorf("%s: %d %q, want it reached directly (the proxy was asked for %v)", url, code, b, asked())
		}
	}
	if proxied(ip.String()) || proxied("localhost.") {
		t.Fatalf("the proxy was asked for a host on the user's own network: %v", asked())
	}

	// a vendor still goes through it: an address of the internet in
	// absolute form, and a name that resolves nowhere here as a CONNECT
	if code, b := fetch("http://203.0.113.5:9/v1/chat/completions"); code != 200 || b != "proxy http://203.0.113.5:9/v1/chat/completions" {
		t.Errorf("a public address: %d %q, want it through the proxy", code, b)
	}
	fetch("https://vendor.invalid/v1/chat/completions")
	if !slices.Contains(asked(), "vendor.invalid:443") {
		t.Errorf("a vendor's name: the proxy was asked for %v, want vendor.invalid:443", asked())
	}

	// a plugin's own HTTP/2 session (Cursor's chats), as #1035 made it take
	// the proxy
	t.Setenv("FAKE_H2", secure.URL)
	Restart()
	if code, b := fetch(secure.URL + "/v1/chat/completions"); code != 200 || b != "lan HTTP/2.0 /v1/chat/completions" {
		t.Errorf("an HTTP/2 session to %s: %d %q, want it reached directly (the proxy was asked for %v)", secure.URL, code, b, asked())
	}
	if proxied(ip.String()) {
		t.Fatalf("the proxy was asked for a host on the user's own network: %v", asked())
	}
}
