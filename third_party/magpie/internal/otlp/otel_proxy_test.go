package otlp

import (
	"crypto/tls"
	"crypto/x509"
	"encoding/binary"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strconv"
	"sync/atomic"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/settings"
)

// A switch of the global proxy left the exporter on the old one: its
// transport is its own, and an HTTPS collector's HTTP/2 connection is
// pooled by the collector alone, so the batches after the switch rode the
// connection dialled through the old proxy (#4824). The send after a move
// drops the idle connections and dials through the proxy in force.
func TestOTelGlobalSwitchDialsThroughTheNewProxy(t *testing.T) {
	var got atomic.Int32
	collector := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.ProtoMajor != 2 {
			t.Errorf("the batch went over %s, want HTTP/2", r.Proto)
		}
		got.Add(1)
		io.WriteString(w, `{}`)
	}))
	collector.EnableHTTP2 = true
	collector.StartTLS()
	t.Cleanup(collector.Close)
	pool := x509.NewCertPool()
	pool.AddCert(collector.Certificate())
	otelConfig(t, settings.OTel{Endpoint: collector.URL})
	a, na := countingSOCKS(t)
	b, nb := countingSOCKS(t)
	var cur atomic.Pointer[url.URL]
	cur.Store(a)
	e := newOTelExporter()
	t.Cleanup(e.cancel)
	t.Cleanup(e.client.CloseIdleConnections)
	tr := e.client.Transport.(*http.Transport)
	// the collector is on the loopback, which Func leaves direct, and its
	// certificate is the test's own; HTTP/2 as with no TLS config set
	tr.Proxy = func(*http.Request) (*url.URL, error) { return cur.Load(), nil }
	tr.TLSClientConfig = &tls.Config{RootCAs: pool}
	tr.ForceAttemptHTTP2 = true
	cfg, _ := settings.OTelExport()
	batch := []otelItem{{record: Record{Time: time.Now(), Status: 200}, config: cfg}}
	if err := settings.Save(settings.Settings{Proxy: a.String()}); err != nil {
		t.Fatal(err)
	}
	e.flush(batch)
	if got.Load() != 1 || na.Load() != 1 || nb.Load() != 0 {
		t.Fatalf("the first batch: %d at the collector, %d connections through A, %d through B", got.Load(), na.Load(), nb.Load())
	}
	cur.Store(b)
	if err := settings.Save(settings.Settings{Proxy: b.String()}); err != nil {
		t.Fatal(err)
	}
	e.flush(batch)
	if got.Load() != 2 || na.Load() != 1 || nb.Load() != 1 {
		t.Fatalf("after the switch: %d at the collector, %d connections through A, %d through B, want the batch dialled through B and no new one through A", got.Load(), na.Load(), nb.Load())
	}
}

// countingSOCKS is a SOCKS5 proxy, no auth, counting the connections it
// carried.
func countingSOCKS(t *testing.T) (*url.URL, *atomic.Int32) {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	var n atomic.Int32
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			go func(c net.Conn) {
				defer c.Close()
				var h [2]byte
				io.ReadFull(c, h[:])
				io.CopyN(io.Discard, c, int64(h[1]))
				c.Write([]byte{5, 0})
				var r [4]byte
				io.ReadFull(c, r[:])
				var host string
				switch r[3] {
				case 1:
					b := make([]byte, 4)
					io.ReadFull(c, b)
					host = net.IP(b).String()
				case 3:
					var l [1]byte
					io.ReadFull(c, l[:])
					b := make([]byte, l[0])
					io.ReadFull(c, b)
					host = string(b)
				}
				var pb [2]byte
				io.ReadFull(c, pb[:])
				up, err := net.Dial("tcp", net.JoinHostPort(host, strconv.Itoa(int(binary.BigEndian.Uint16(pb[:])))))
				if err != nil {
					c.Write([]byte{5, 5, 0, 1, 0, 0, 0, 0, 0, 0})
					return
				}
				defer up.Close()
				n.Add(1)
				c.Write([]byte{5, 0, 0, 1, 127, 0, 0, 1, 0, 0})
				go io.Copy(up, c)
				io.Copy(c, up)
			}(c)
		}
	}()
	return &url.URL{Scheme: "socks5", Host: ln.Addr().String()}, &n
}
