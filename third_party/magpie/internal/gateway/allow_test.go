package gateway

import (
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/lastgood"
	"github.com/yetone/magpie/internal/settings"
)

// lc on Discord, magpie in Docker on a public host: with an allow list,
// a request from an address outside it is refused 403 even with an
// enabled gateway key, saying why; one inside it is judged by its key as
// before; loopback and this computer's own addresses are always let in.
// MAGPIE_ALLOW_IPS comes before Settings' list, and a typo in it keeps
// the list on. No list lets every address in.
func TestAllowList(t *testing.T) {
	fresh(t)
	t.Setenv("MAGPIE_ADDR", "0.0.0.0:3499")
	t.Setenv(AllowIPsEnv, "")
	secret, err := access.Update("add-key", access.Change{Name: "Laptop"})
	if err != nil {
		t.Fatal(err)
	}
	h := lanGuard(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {}))
	call := func(from string, key string) (int, string) {
		r := httptest.NewRequest("POST", "/v1/messages", nil)
		r.RemoteAddr = from
		if key != "" {
			r.Header.Set("x-api-key", key)
		}
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		return w.Code, w.Body.String()
	}
	type want struct {
		from string
		key  string
		code int
	}
	check := func(name string, cases []want) {
		t.Helper()
		for _, c := range cases {
			code, body := call(c.from, c.key)
			if code != c.code {
				t.Errorf("%s: from %s key %v: %d %s, want %d", name, c.from, c.key != "", code, body, c.code)
			}
			if code == http.StatusForbidden && !strings.Contains(body, "allow list") {
				t.Errorf("%s: from %s refused without saying why: %s", name, c.from, body)
			}
		}
	}
	// no list: every address, with a key
	check("no list", []want{
		{"203.0.113.9:5000", secret, 200},
		{"[2001:db8::9]:5000", secret, 200},
		{"203.0.113.9:5000", "", 401},
	})

	s := settings.Load()
	s.AllowIPs = []string{"198.51.100.7", "10.0.0.0/8", "2001:db8:1::/48"}
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	check("settings", []want{
		{"198.51.100.7:5000", secret, 200},
		{"198.51.100.7:5000", "", 401}, // on the list, still needs a key
		{"198.51.100.8:5000", secret, 403},
		{"10.20.30.40:5000", secret, 200},
		{"11.0.0.1:5000", secret, 403},
		{"[::ffff:10.1.2.3]:5000", secret, 200}, // IPv4 as IPv6
		{"[2001:db8:1:ff::2]:5000", secret, 200},
		{"[2001:db8:2::2]:5000", secret, 403},
		{"[fe80::db8:dead:beef:1%en0]:5000", secret, 403},
		{"127.0.0.1:5000", "", 200},
		{"[::1]:5000", "", 200},
		{"127.8.9.10:5000", "", 200},
		{"garbage", secret, 403},
	})

	// the environment's list comes first
	t.Setenv(AllowIPsEnv, "203.0.113.0/24, 2001:db8::5")
	check("env", []want{
		{"203.0.113.200:5000", secret, 200},
		{"[2001:db8::5]:5000", secret, 200},
		{"[2001:db8::6]:5000", secret, 403},
		{"198.51.100.7:5000", secret, 403},
		{"127.0.0.1:5000", "", 200},
	})
	if l, env := AllowIPs(); !env || len(l) != 2 {
		t.Errorf("AllowIPs = %v %v", l, env)
	}
	// a typo lets nothing in, and never opens magpie to all
	t.Setenv(AllowIPsEnv, "203.0.113.0/33")
	check("typo", []want{
		{"203.0.113.9:5000", secret, 403},
		{"198.51.100.7:5000", secret, 403},
		{"127.0.0.1:5000", "", 200},
	})

	// this computer's own address (MAGPIE_ADDR on one interface's) is it
	if a := ownIPv4(); a != "" {
		check("own", []want{{net.JoinHostPort(a, "5000"), secret, 200}})
	}
}

// On the running gateway, a real connection from loopback passes under a
// list that names nothing of this computer's, and the allow list is
// judged before the key: the refusal says so, not that the key is wrong.
func TestAllowListServed(t *testing.T) {
	fresh(t)
	t.Setenv("MAGPIE_ADDR", "0.0.0.0:3499")
	t.Setenv(AllowIPsEnv, "192.0.2.1")
	srv := httptest.NewServer(lanGuard(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { io.WriteString(w, "in") })))
	t.Cleanup(srv.Close)
	res, err := http.Get(srv.URL + "/v1/models")
	if err != nil {
		t.Fatal(err)
	}
	b, _ := io.ReadAll(res.Body)
	res.Body.Close()
	if res.StatusCode != 200 || string(b) != "in" {
		t.Fatalf("loopback: %d %s", res.StatusCode, b)
	}
	// the same guard, a request from elsewhere with no key
	r := httptest.NewRequest("GET", "/v1/models", nil)
	r.RemoteAddr = "198.51.100.1:443"
	w := httptest.NewRecorder()
	lanGuard(http.NotFoundHandler()).ServeHTTP(w, r)
	if w.Code != 403 || !strings.Contains(w.Body.String(), "198.51.100.1") || !strings.Contains(w.Body.String(), AllowIPsEnv) {
		t.Errorf("elsewhere: %d %s", w.Code, w.Body.String())
	}
}

// Settings checking a new list won't shut out the page it is saved from.
func TestKeepsOut(t *testing.T) {
	r := httptest.NewRequest("POST", "/api/settings/allowips", nil)
	r.RemoteAddr = "198.51.100.7:5000"
	if a, out := KeepsOut([]string{"10.0.0.0/8"}, r); !out || a != "198.51.100.7" {
		t.Errorf("leaves out: %v %q", out, a)
	}
	if _, out := KeepsOut([]string{"198.51.100.0/24"}, r); out {
		t.Error("on the list")
	}
	if _, out := KeepsOut(nil, r); out {
		t.Error("no list")
	}
	r.RemoteAddr = "127.0.0.1:5000"
	if _, out := KeepsOut([]string{"10.0.0.0/8"}, r); out {
		t.Error("loopback")
	}
}

// settings.json that can't be read, or doesn't parse with no last good
// generation, holds a list nobody knows, and is never taken for no list:
// the list read last stays in force, and with none read since magpie
// started, only this computer is let in, told why. No file is no list.
func TestAllowListUnreadableSettings(t *testing.T) {
	fresh(t)
	t.Setenv("MAGPIE_ADDR", "0.0.0.0:3499")
	t.Setenv(AllowIPsEnv, "")
	forgetAllow := func() {
		lastAllow.Lock()
		lastAllow.list, lastAllow.read, lastAllow.warned = nil, false, false
		lastAllow.Unlock()
	}
	forgetAllow()
	t.Cleanup(forgetAllow)
	secret, err := access.Update("add-key", access.Change{Name: "Laptop"})
	if err != nil {
		t.Fatal(err)
	}
	h := lanGuard(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {}))
	check := func(name, from string, want int, says string) {
		t.Helper()
		r := httptest.NewRequest("POST", "/v1/messages", nil)
		r.RemoteAddr = from
		r.Header.Set("x-api-key", secret)
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		if w.Code != want || !strings.Contains(w.Body.String(), says) {
			t.Errorf("%s: from %s: %d %s, want %d saying %q", name, from, w.Code, w.Body.String(), want, says)
		}
	}
	garble := func() {
		t.Helper()
		os.Remove(lastgood.Bak(settings.Path()))
		if err := os.WriteFile(settings.Path(), []byte(`{"allowIPs": ["198.51.100.7"`), 0o600); err != nil {
			t.Fatal(err)
		}
		if _, err := settings.LoadChecked(); err == nil {
			t.Fatal("the garbled settings.json read as good")
		}
	}

	os.Remove(settings.Path())
	check("no file", "203.0.113.9:5000", 200, "")

	s := settings.Load()
	s.AllowIPs = []string{"198.51.100.7"}
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	check("read", "203.0.113.9:5000", 403, "allow list")
	check("read", "198.51.100.7:5000", 200, "")

	// read once, then garbled: the list read stays
	garble()
	check("garbled after a read", "203.0.113.9:5000", 403, "allow list")
	check("garbled after a read", "198.51.100.7:5000", 200, "")
	check("garbled after a read", "127.0.0.1:5000", 200, "")
	if l, _ := AllowIPs(); len(l) != 1 || l[0] != "198.51.100.7" {
		t.Errorf("AllowIPs with the file garbled = %v, want the list read last", l)
	}

	// garbled before any read: only this computer
	forgetAllow()
	check("garbled, never read", "203.0.113.9:5000", 403, "settings.json")
	check("garbled, never read", "198.51.100.7:5000", 403, "settings.json")
	check("garbled, never read", "[2001:db8::9]:5000", 403, "settings.json")
	check("garbled, never read", "127.0.0.1:5000", 200, "")
	check("garbled, never read", "[::1]:5000", 200, "")

	// the environment's list is judged as before
	t.Setenv(AllowIPsEnv, "203.0.113.0/24")
	check("env", "203.0.113.9:5000", 200, "")
	check("env", "198.51.100.7:5000", 403, AllowIPsEnv)
	t.Setenv(AllowIPsEnv, "")

	// gone: no list, every address
	os.Remove(settings.Path())
	check("removed", "203.0.113.9:5000", 200, "")
}
