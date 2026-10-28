package plugin

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha512"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/base64"
	"encoding/json"
	"encoding/pem"
	"fmt"
	"math/big"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/testenv"
)

// reSigningCA is a root as an HTTPS-scanning antivirus or a company proxy
// installs it, and a server whose certificate it signed: what Bun meets
// behind such a box. The server sends its chain with the root in it, as
// those boxes do, and serves an npm registry with one package.
func reSigningCA(t *testing.T) (caPEM []byte, url string) {
	t.Helper()
	caKey, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	caTpl := &x509.Certificate{
		SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: "magpie test HTTPS scan root"},
		NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(time.Hour),
		IsCA: true, BasicConstraintsValid: true, KeyUsage: x509.KeyUsageCertSign | x509.KeyUsageCRLSign,
	}
	caDER, err := x509.CreateCertificate(rand.Reader, caTpl, caTpl, &caKey.PublicKey, caKey)
	if err != nil {
		t.Fatal(err)
	}
	ca, _ := x509.ParseCertificate(caDER)
	leafKey, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	leafTpl := &x509.Certificate{
		SerialNumber: big.NewInt(2), Subject: pkix.Name{CommonName: "127.0.0.1"},
		IPAddresses: []net.IP{net.IPv4(127, 0, 0, 1)}, DNSNames: []string{"localhost"},
		NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(time.Hour),
		KeyUsage: x509.KeyUsageDigitalSignature, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
	}
	leafDER, err := x509.CreateCertificate(rand.Reader, leafTpl, ca, &leafKey.PublicKey, caKey)
	if err != nil {
		t.Fatal(err)
	}

	const pkg = "magpie-ca-probe"
	var tgz bytes.Buffer
	gz := gzip.NewWriter(&tgz)
	tw := tar.NewWriter(gz)
	body := []byte(`{"name":"` + pkg + `","version":"1.0.0"}`)
	_ = tw.WriteHeader(&tar.Header{Name: "package/package.json", Mode: 0o644, Size: int64(len(body)), ModTime: time.Now()})
	_, _ = tw.Write(body)
	_ = tw.Close()
	_ = gz.Close()
	sum := sha512.Sum512(tgz.Bytes())

	var srv *httptest.Server
	srv = httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/" + pkg:
			v := map[string]any{"name": pkg, "version": "1.0.0", "dist": map[string]any{
				"tarball":   srv.URL + "/" + pkg + "-1.0.0.tgz",
				"integrity": "sha512-" + base64.StdEncoding.EncodeToString(sum[:]),
			}}
			w.Header().Set("Content-Type", "application/json")
			_ = json.NewEncoder(w).Encode(map[string]any{"name": pkg, "dist-tags": map[string]string{"latest": "1.0.0"},
				"versions": map[string]any{"1.0.0": v}})
		case "/" + pkg + "-1.0.0.tgz":
			_, _ = w.Write(tgz.Bytes())
		default:
			fmt.Fprint(w, "through")
		}
	}))
	srv.TLS = &tls.Config{Certificates: []tls.Certificate{{Certificate: [][]byte{leafDER, caDER}, PrivateKey: leafKey}}}
	srv.StartTLS()
	t.Cleanup(srv.Close)
	return pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: caDER}), srv.URL
}

// Behind a box that re-signs HTTPS with a root the system trusts, Bun's
// fetch (the plugin host's) and bun add (a plugin's install or update)
// get through (明霞 on Discord: every plugin answered "self signed
// certificate in certificate chain", and an update failed with
// UNABLE_TO_GET_ISSUER_CERT_LOCALLY). They do with the root in the
// system's store, and with it in a NODE_EXTRA_CA_CERTS of the user's
// own; with it in neither, they still refuse the server.
//
// The system's store is put in place without touching the machine's:
// on Linux through SSL_CERT_DIR, which Bun reads as the system's store
// and which magpie's own copy of the roots (the distribution's bundle)
// never took in. macOS and Windows have no such variable, so there only
// the user's file and the refusal are tried.
func TestBunTrustsTheSystemsStore(t *testing.T) {
	bun := testenv.Bun(t)
	sandbox(t)
	for _, k := range []string{"HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy",
		"NODE_EXTRA_CA_CERTS", "NODE_USE_SYSTEM_CA", "NODE_TLS_REJECT_UNAUTHORIZED", "SSL_CERT_FILE", "SSL_CERT_DIR"} {
		t.Setenv(k, "")
	}
	t.Setenv("NO_PROXY", "127.0.0.1,localhost")
	caPEM, url := reSigningCA(t)
	other, _ := reSigningCA(t) // a root the system trusts that isn't this server's
	old := systemCAFile
	t.Cleanup(func() { systemCAFile = old })
	// read again for each case, not once a run
	systemCAFile = func() string { return systemCABundle() }

	write := func(name string, b []byte) string {
		p := filepath.Join(t.TempDir(), name)
		if err := os.WriteFile(p, b, 0o644); err != nil {
			t.Fatal(err)
		}
		return p
	}
	run := func(args ...string) (string, error) {
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
		defer cancel()
		dir := t.TempDir()
		if err := os.WriteFile(filepath.Join(dir, "package.json"), []byte(`{"name":"p","private":true}`), 0o644); err != nil {
			t.Fatal(err)
		}
		cmd := bunCommand(ctx, bun, dir, args...)
		// a cache of its own each time, so no case installs from another's
		cmd.Env = append(cmd.Env, "BUN_INSTALL_CACHE_DIR="+t.TempDir(), "BUN_CONFIG_REGISTRY="+url+"/")
		out, err := cmd.CombinedOutput()
		return string(out), err
	}
	fetch := `const r = await fetch(` + strconv.Quote(url) + `); console.log(await r.text())`
	try := func(t *testing.T, want bool) {
		t.Helper()
		out, err := run("-e", fetch)
		if got := err == nil && strings.Contains(out, "through"); got != want {
			t.Errorf("Bun's fetch got through: %v, want %v (%v)\n%s", got, want, err, out)
		}
		out, err = run("add", "--ignore-scripts", "magpie-ca-probe")
		if got := err == nil; got != want {
			t.Errorf("bun add got through: %v, want %v (%v)\n%s", got, want, err, out)
		}
	}

	t.Run("in neither", func(t *testing.T) {
		if runtime.GOOS == "linux" {
			// the machine's store, with no root of this server's
			t.Setenv("SSL_CERT_FILE", write("bundle.pem", other))
		}
		try(t, false)
	})
	t.Run("in the system's store", func(t *testing.T) {
		if runtime.GOOS != "linux" {
			t.Skip("the system's store here can't be given a root without changing the machine's")
		}
		t.Setenv("SSL_CERT_FILE", write("bundle.pem", other))
		dir := t.TempDir()
		if err := os.WriteFile(filepath.Join(dir, "https-scan-root.pem"), caPEM, 0o644); err != nil {
			t.Fatal(err)
		}
		t.Setenv("SSL_CERT_DIR", dir)
		try(t, true)
	})
	t.Run("in the user's NODE_EXTRA_CA_CERTS", func(t *testing.T) {
		if runtime.GOOS == "linux" {
			t.Setenv("SSL_CERT_FILE", write("bundle.pem", other))
		}
		t.Setenv("NODE_EXTRA_CA_CERTS", write("mine.pem", caPEM))
		try(t, true)
	})
}
