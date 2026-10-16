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
)

// A computer joining a folder keeps the gateway keys it holds that the
// folder's store has never seen (#1714, 8ugMak1r): the store rides the
// settings part, and a first sync brings a part changed only on the server
// in whole. A key the folder holds by id or by value is the folder's; the
// default key follows the folder's settings; a key removed on the folder
// after the join still goes from every computer.
func TestGatewayKeysJoinKeepsUnseenKeys(t *testing.T) {
	fake := &fakeDAV{files: map[string][]byte{}, etags: map[string]string{}, dirs: map[string]bool{"/dav": true}, cond: true, putETag: true}
	srv := httptest.NewServer(fake)
	defer srv.Close()
	cfg := Config{URL: srv.URL + "/dav/", User: "me", Password: "pw", Passphrase: "gateway join fixture passphrase", Keys: true}
	a, b := newComputer(t), newComputer(t)
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
	remote := func() backup.Bundle {
		t.Helper()
		fake.mu.Lock()
		data := append([]byte(nil), fake.files["/dav/magpie/magpie.magpie-backup"]...)
		fake.mu.Unlock()
		got, err := backup.Open(data, cfg.Passphrase)
		if err != nil {
			t.Fatal(err)
		}
		return got
	}
	storeIDs := func(b backup.Bundle) []string {
		t.Helper()
		if b.GatewayKeys == nil {
			return nil
		}
		var ids []string
		for _, k := range *b.GatewayKeys {
			ids = append(ids, k.ID)
		}
		return ids
	}

	// Computer B configures sync first, holding sharing's default key alone.
	use(b)
	if err := access.ConfigureLAN(true, false); err != nil {
		t.Fatal(err)
	}
	keysB, err := access.Export()
	if err != nil {
		t.Fatal(err)
	}
	if len(keysB) != 1 {
		t.Fatalf("fixture: computer B holds %d keys, want the default alone", len(keysB))
	}
	defaultB := keysB[0]
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	now()

	// Computer A holds several gateway keys of its own and joins after.
	use(a)
	if err := access.ConfigureLAN(true, false); err != nil {
		t.Fatal(err)
	}
	laptop, err := access.Update("add-key", access.Change{Name: "Laptop"})
	if err != nil {
		t.Fatal(err)
	}
	who, ok := access.Authenticate(laptop)
	if !ok {
		t.Fatal("fixture: the added gateway key does not authenticate")
	}
	own, err := access.Update("add-key", access.Change{Name: "Own", Secret: "fixture-own-client-key"})
	if err != nil {
		t.Fatal(err)
	}
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	now()
	if _, ok := access.Authenticate(laptop); !ok {
		t.Error("joining the folder destroyed this computer's own gateway key")
	}
	if _, ok := access.Authenticate(own); !ok {
		t.Error("joining the folder destroyed this computer's own named key value")
	}
	if _, ok := access.Authenticate(defaultB.Secret); !ok {
		t.Error("joining did not bring the folder's default key in")
	}
	got := remote()
	ids := storeIDs(got)
	for _, want := range []string{defaultB.ID, who.KeyID} {
		if !slices.Contains(ids, want) {
			t.Errorf("the store pushed after the join lacks key %s; it holds %v", want, ids)
		}
	}

	// Computer B takes the joined keys in.
	use(b)
	now()
	if _, ok := access.Authenticate(own); !ok {
		t.Error("computer B did not receive computer A's key")
	}

	// A key removed after the join still goes from every computer.
	use(a)
	if _, err := access.Update("remove-key", access.Change{Key: who.KeyID}); err != nil {
		t.Fatal(err)
	}
	now()
	use(b)
	now()
	if _, ok := access.Authenticate(laptop); ok {
		t.Error("a key removed after the join came back on computer B")
	}
	if _, ok := access.Authenticate(own); !ok {
		t.Error("an unrelated key was lost on computer B")
	}
}

// A folder with no gateway-key store at all leaves a joining computer's
// keys as they are: nothing there replaces them.
func TestGatewayKeysJoinWithoutStoreKeepsKeys(t *testing.T) {
	fake := &fakeDAV{files: map[string][]byte{}, etags: map[string]string{}, dirs: map[string]bool{"/dav": true}, cond: true, putETag: true}
	srv := httptest.NewServer(fake)
	defer srv.Close()
	cfg := Config{URL: srv.URL + "/dav/", User: "me", Password: "pw", Passphrase: "gateway join fixture passphrase", Keys: true}
	a, b := newComputer(t), newComputer(t)
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
	remote := func() backup.Bundle {
		t.Helper()
		fake.mu.Lock()
		data := append([]byte(nil), fake.files["/dav/magpie/magpie.magpie-backup"]...)
		fake.mu.Unlock()
		got, err := backup.Open(data, cfg.Passphrase)
		if err != nil {
			t.Fatal(err)
		}
		return got
	}

	use(b)
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	now()
	if got := remote(); got.GatewayKeys != nil {
		t.Fatal("fixture: the folder holds a gateway-key store")
	}

	use(a)
	if err := access.ConfigureLAN(true, false); err != nil {
		t.Fatal(err)
	}
	laptop, err := access.Update("add-key", access.Change{Name: "Laptop"})
	if err != nil {
		t.Fatal(err)
	}
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	now()
	if _, ok := access.Authenticate(laptop); !ok {
		t.Error("joining a folder with no key store destroyed this computer's key")
	}
}
