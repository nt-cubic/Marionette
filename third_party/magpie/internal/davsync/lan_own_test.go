package davsync

import (
	"context"
	"net/http/httptest"
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/backup"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// Sharing the gateway on the local network is this computer's own (Lullaby
// on Discord: turned on at home, it came on at work too). Two computers in
// one folder, sharing on at one and off at the other, each keep their own
// through syncs both ways, a join and a Restore; flipping it is no change
// to push; the default key still follows the folder, so the other computer
// turning sharing on takes up the same key. The addresses let in and the
// terminal a session resumes in are this computer's too.
func TestLANSharingStaysOnItsComputer(t *testing.T) {
	fake := &fakeDAV{files: map[string][]byte{}, etags: map[string]string{}, dirs: map[string]bool{"/dav": true}, cond: true, putETag: true}
	srv := httptest.NewServer(fake)
	defer srv.Close()
	cfg := Config{URL: srv.URL + "/dav/", User: "me", Password: "pw", Passphrase: "lan sharing fixture passphrase", Keys: true}
	home, work := newComputer(t), newComputer(t)
	use := func(c computer) {
		t.Helper()
		c.use(t)
		appdir.UseExecutable("")
		provider.ForgetAccounts()
	}
	t.Cleanup(provider.ForgetAccounts)
	now := func() {
		t.Helper()
		if err := Now(context.Background()); err != nil {
			t.Fatal(err)
		}
	}
	puts := func() int {
		fake.mu.Lock()
		defer fake.mu.Unlock()
		return fake.puts
	}
	edit := func(f func(*settings.Settings)) {
		t.Helper()
		s := settings.Load()
		f(&s)
		if err := settings.Save(s); err != nil {
			t.Fatal(err)
		}
	}
	want := func(where string, lan bool, allow []string, term string) {
		t.Helper()
		s := settings.Load()
		if s.LAN != lan {
			t.Errorf("%s: sharing on the local network is %v, want %v (this computer's own)", where, s.LAN, lan)
		}
		if !slices.Equal(s.AllowIPs, allow) {
			t.Errorf("%s: addresses let in are %v, want %v (this computer's own)", where, s.AllowIPs, allow)
		}
		if s.SessionTerminal != term {
			t.Errorf("%s: session terminal is %q, want %q (this computer's own)", where, s.SessionTerminal, term)
		}
	}

	// Home shares the gateway, on its own network, and syncs first.
	use(home)
	if err := access.ConfigureLAN(true, false); err != nil {
		t.Fatal(err)
	}
	homeKey := access.LANSecret()
	if homeKey == "" {
		t.Fatal("fixture: sharing on made no default key")
	}
	edit(func(s *settings.Settings) {
		s.Theme = "dark"
		s.AllowIPs = []string{"192.168.1.0/24"}
		s.SessionTerminal = "com.googlecode.iterm2"
	})
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	now()

	// Work, sharing off, joins: home's theme comes in, home's sharing
	// doesn't.
	use(work)
	edit(func(s *settings.Settings) { s.AllowIPs = []string{"10.0.0.0/8"} })
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	now()
	if s := settings.Load(); s.Theme != "dark" {
		t.Fatalf("fixture: the join brought no settings in (theme %q)", s.Theme)
	}
	want("work after joining", false, []string{"10.0.0.0/8"}, "")
	if access.LANSecret() != "" {
		t.Error("work after joining: the gateway is shared with a key")
	}
	// a Restore of the folder's setup leaves it as it is too
	edit(func(s *settings.Settings) { s.Theme = "light" })
	if _, err := Restore(context.Background()); err != nil {
		t.Fatal(err)
	}
	if s := settings.Load(); s.Theme != "dark" {
		t.Fatalf("fixture: the restore brought no settings in (theme %q)", s.Theme)
	}
	want("work after a restore", false, []string{"10.0.0.0/8"}, "")

	// Work changes a synced setting: it goes up, with work's sharing off,
	// and home takes it in still sharing.
	edit(func(s *settings.Settings) { s.Theme = "light" })
	now()
	use(home)
	now()
	if s := settings.Load(); s.Theme != "light" {
		t.Fatalf("fixture: home didn't take work's change in (theme %q)", s.Theme)
	}
	want("home after work's push", true, []string{"192.168.1.0/24"}, "com.googlecode.iterm2")
	if got := access.LANSecret(); got != homeKey {
		t.Errorf("home after work's push: shared with %q, want its key", got)
	}

	// Home turning sharing off is no change to push.
	before := puts()
	if err := access.ConfigureLAN(false, false); err != nil {
		t.Fatal(err)
	}
	now()
	if n := puts() - before; n != 0 {
		t.Errorf("turning sharing off at home pushed %d times", n)
	}

	// Work turns sharing on: with the folder's default key, and home stays
	// off through syncs both ways.
	use(work)
	if err := access.ConfigureLAN(true, false); err != nil {
		t.Fatal(err)
	}
	if got := access.LANSecret(); got != homeKey {
		t.Errorf("work shares with %q, want the folder's default key", got)
	}
	edit(func(s *settings.Settings) { s.Theme = "system" })
	now()
	use(home)
	now()
	if s := settings.Load(); s.Theme != "system" {
		t.Fatalf("fixture: home didn't take work's change in (theme %q)", s.Theme)
	}
	want("home after work turned sharing on", false, []string{"192.168.1.0/24"}, "com.googlecode.iterm2")
	use(work)
	now()
	want("work after syncing again", true, []string{"10.0.0.0/8"}, "")
}

// The settings part's hash, which tells a change to push, doesn't see
// sharing, the addresses let in or the terminal.
func TestLANSharingIsNoChange(t *testing.T) {
	a := settings.Settings{Theme: "dark"}
	b := a
	b.LAN, b.AllowIPs, b.SessionTerminal = true, []string{"10.0.0.0/8"}, "com.googlecode.iterm2"
	if hashes(backup.Bundle{Settings: &a})["settings"] != hashes(backup.Bundle{Settings: &b})["settings"] {
		t.Error("sharing on the local network changes the settings part's hash")
	}
}
