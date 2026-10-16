package gui

import (
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/settings"
)

// The allow list (lc on Discord) is saved cleaned; an entry that isn't an
// address, a list that would shut out the page saving it, and any list
// while MAGPIE_ALLOW_IPS sets it are refused with the list left as it
// was; a save of the rest of the Settings page leaves it alone; the page
// is told what the variable sets.
func TestAllowIPsSetting(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	t.Setenv(gateway.AllowIPsEnv, "")
	srv := Handler(nil, nil)
	post := func(path, body, from string) *httptest.ResponseRecorder {
		rec := httptest.NewRecorder()
		r := httptest.NewRequest("POST", path, strings.NewReader(body))
		r.RemoteAddr = from
		srv.ServeHTTP(rec, r)
		return rec
	}
	here := "127.0.0.1:50000"
	rec := post("/api/settings/allowips", `{"ips":[" 203.0.113.7 ","10.1.2.3/8","2001:DB8::/32","203.0.113.7/32"]}`, here)
	want := []string{"203.0.113.7", "10.0.0.0/8", "2001:db8::/32"}
	if rec.Code != http.StatusOK || !slices.Equal(settings.Load().AllowIPs, want) || !strings.Contains(rec.Body.String(), `"allowIPs":["203.0.113.7","10.0.0.0/8","2001:db8::/32"]`) {
		t.Fatalf("%d %s, saved %v", rec.Code, rec.Body, settings.Load().AllowIPs)
	}
	if rec := post("/api/settings/allowips", `{"ips":["my-laptop"]}`, here); rec.Code == http.StatusOK || !strings.Contains(rec.Body.String(), "is not an IP address") || !slices.Equal(settings.Load().AllowIPs, want) {
		t.Errorf("not an address: %d %s, saved %v", rec.Code, rec.Body, settings.Load().AllowIPs)
	}
	// magpie web opened from 203.0.113.7: a list without it would shut it out
	if rec := post("/api/settings/allowips", `{"ips":["10.0.0.0/8"]}`, "203.0.113.7:50000"); rec.Code == http.StatusOK || !strings.Contains(rec.Body.String(), "shut out 203.0.113.7") || !slices.Equal(settings.Load().AllowIPs, want) {
		t.Errorf("shut out: %d %s, saved %v", rec.Code, rec.Body, settings.Load().AllowIPs)
	}
	if rec := post("/api/settings", `{"theme":"dark","lang":"en"}`, here); rec.Code != http.StatusOK || !slices.Equal(settings.Load().AllowIPs, want) {
		t.Fatalf("the Settings page's save changed the list: %d %v", rec.Code, settings.Load().AllowIPs)
	}

	t.Setenv(gateway.AllowIPsEnv, "1.2.3.4, 10.0.0.0/8")
	if rec := post("/api/settings/allowips", `{"ips":[]}`, here); rec.Code == http.StatusOK || !strings.Contains(rec.Body.String(), "MAGPIE_ALLOW_IPS") || !slices.Equal(settings.Load().AllowIPs, want) {
		t.Errorf("under the variable: %d %s, saved %v", rec.Code, rec.Body, settings.Load().AllowIPs)
	}
	st := settingsState()
	if st.AllowIPsEnv != "1.2.3.4, 10.0.0.0/8" || !slices.Equal(st.AllowIPsFromEnv, []string{"1.2.3.4", "10.0.0.0/8"}) {
		t.Errorf("state: %q %v", st.AllowIPsEnv, st.AllowIPsFromEnv)
	}

	t.Setenv(gateway.AllowIPsEnv, "")
	if rec := post("/api/settings/allowips", `{"ips":[]}`, here); rec.Code != http.StatusOK || len(settings.Load().AllowIPs) != 0 {
		t.Fatalf("emptied: %d %v", rec.Code, settings.Load().AllowIPs)
	}
}

// settings.json that doesn't parse, with no last good generation, holds an
// allow list nobody knows: neither the Settings page's save nor the allow
// list's own writes the defaults (no list, every address let in) over it.
func TestAllowIPsUnreadableSettingsNotSavedOver(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	t.Setenv(gateway.AllowIPsEnv, "")
	srv := Handler(nil, nil)
	if err := os.MkdirAll(settings.Dir(), 0o755); err != nil {
		t.Fatal(err)
	}
	garbled := []byte(`{"allowIPs": ["203.0.113.7"`)
	if err := os.WriteFile(settings.Path(), garbled, 0o600); err != nil {
		t.Fatal(err)
	}
	for _, c := range []struct{ path, body string }{
		{"/api/settings", `{"theme":"dark","lang":"en"}`},
		{"/api/settings/allowips", `{"ips":["10.0.0.0/8"]}`},
	} {
		rec := httptest.NewRecorder()
		r := httptest.NewRequest("POST", c.path, strings.NewReader(c.body))
		r.RemoteAddr = "127.0.0.1:50000"
		srv.ServeHTTP(rec, r)
		if rec.Code == http.StatusOK || !strings.Contains(rec.Body.String(), "could not read settings") {
			t.Errorf("%s: %d %s, want refused", c.path, rec.Code, rec.Body)
		}
		if b, _ := os.ReadFile(settings.Path()); string(b) != string(garbled) {
			t.Fatalf("%s wrote over the unreadable settings: %q", c.path, b)
		}
	}
}

// The desktop window's requests reach the API through Wails, which names
// their address 192.0.2.1:1234 when it has none. Saving an allow list from
// the window doesn't call that address the page's and refuse the list
// (Mathresearcher on Discord).
func TestAllowIPsFromTheAppWindow(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	t.Setenv(gateway.AllowIPsEnv, "")
	srv := ownWindow(Handler(nil, nil))
	rec := httptest.NewRecorder()
	r := httptest.NewRequest("POST", "/api/settings/allowips", strings.NewReader(`{"ips":["10.0.0.0/8"]}`))
	r.RemoteAddr = "192.0.2.1:1234" // what Wails' asset server fills in
	srv.ServeHTTP(rec, r)
	if rec.Code != http.StatusOK || strings.Contains(rec.Body.String(), "192.0.2.1") || !slices.Equal(settings.Load().AllowIPs, []string{"10.0.0.0/8"}) {
		t.Fatalf("%d %s, saved %v", rec.Code, rec.Body, settings.Load().AllowIPs)
	}
}
