package backup

import (
	"bytes"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// Exporting and importing providers must keep the order the user chose,
// which can differ from the order they were added in.
func TestBackupProviderOrderRoundTrip(t *testing.T) {
	for _, tc := range []struct {
		name     string
		arranged bool
	}{
		{"as-added", false},
		{"arranged", true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			home(t)
			added := addOrderProviders(t)
			want := slices.Clone(added)
			if tc.arranged {
				slices.Reverse(want)
				if err := provider.SetOrder(want); err != nil {
					t.Fatal(err)
				}
			}
			if got := listedProviderIDs(); !slices.Equal(got, want) {
				t.Fatalf("source provider order: got %v, want %v", got, want)
			}
			b, err := Collect(true, "test")
			if err != nil {
				t.Fatal(err)
			}
			if len(b.Providers) != len(added) || b.Providers[0].ID != added[0] || b.Providers[1].ID != added[1] {
				t.Fatal("backup did not carry the providers in their stored insertion order")
			}
			if tc.arranged && !slices.Equal(b.Order, want) || !tc.arranged && len(b.Order) != 0 {
				t.Fatalf("collected order: %v, arranged: %v", b.Order, tc.arranged)
			}
			data, err := Seal(b, "provider order passphrase")
			if err != nil {
				t.Fatal(err)
			}
			opened, err := Open(data, "provider order passphrase")
			if err != nil {
				t.Fatal(err)
			}
			if !opened.Keys || !slices.Equal(opened.Order, b.Order) {
				t.Fatalf("sealed backup lost its keys flag or order: keys=%v order=%v", opened.Keys, opened.Order)
			}
			t.Logf("source list=%v; stored insertion order=%v; collected and decrypted order=%v", want, added, opened.Order)

			home(t) // another machine
			if got := listedProviderIDs(); len(got) != 0 {
				t.Fatalf("restore target is not empty: %v", got)
			}
			r, err := Restore(opened, Parts{Providers: true})
			if err != nil {
				t.Fatal(err)
			}
			if r.Added != len(added) || r.Replaced != 0 || len(r.NeedKey) != 0 {
				t.Fatalf("restore result: %+v", r)
			}
			got := listedProviderIDs()
			t.Logf("restore added=%d replaced=%d; restored list=%v", r.Added, r.Replaced, got)
			if !slices.Equal(got, want) {
				t.Fatalf("provider order after backup restore: got %v, want %v", got, want)
			}
		})
	}
}

// Empty backups and backups from before Order was added leave the order
// already chosen on the destination machine alone.
func TestBackupWithoutOrderKeepsLocalOrder(t *testing.T) {
	for _, tc := range []struct {
		name      string
		providers bool
	}{
		{"empty", false},
		{"legacy-without-order", true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			home(t)
			if tc.providers {
				addOrderProviders(t)
			}
			b, err := Collect(true, "test")
			if err != nil {
				t.Fatal(err)
			}
			// With no arrangement, Order is omitted from the encrypted JSON,
			// the same representation a backup from before Order carries.
			if len(b.Order) != 0 {
				t.Fatalf("fixture unexpectedly has an order: %v", b.Order)
			}
			data, err := Seal(b, "provider order passphrase")
			if err != nil {
				t.Fatal(err)
			}
			opened, err := Open(data, "provider order passphrase")
			if err != nil {
				t.Fatal(err)
			}
			if len(opened.Order) != 0 {
				t.Fatalf("opened backup unexpectedly has an order: %v", opened.Order)
			}

			home(t)
			want := addOrderProviders(t)
			slices.Reverse(want)
			if err := provider.SetOrder(want); err != nil {
				t.Fatal(err)
			}
			r, err := Restore(opened, Parts{Providers: true})
			if err != nil {
				t.Fatal(err)
			}
			if r.Added != 0 || r.Replaced != len(opened.Providers) || len(r.NeedKey) != 0 {
				t.Fatalf("restore result: %+v", r)
			}
			if got := listedProviderIDs(); !slices.Equal(got, want) {
				t.Fatalf("backup without order changed local list: got %v, want %v", got, want)
			}
			stored, err := provider.StoredOrder()
			if err != nil || !slices.Equal(stored, want) {
				t.Fatalf("backup without order changed stored order: got %v, want %v, err=%v", stored, want, err)
			}
			t.Logf("backup order=%v; restored added=%d replaced=%d; local list and stored order stay %v", opened.Order, r.Added, r.Replaced, want)
		})
	}
}

// A bare model id must still reach the first provider the user chose
// after the backup has crossed machines.
func TestBackupProviderOrderRestoresRouting(t *testing.T) {
	home(t)
	order := addOrderProviders(t)
	slices.Reverse(order)
	if err := provider.SetOrder(order); err != nil {
		t.Fatal(err)
	}
	if p, model, ok := provider.Resolve("order-model"); !ok || p.ID != order[0] || model != "order-model" {
		t.Fatalf("source routing: provider=%q model=%q ok=%v, want %q", p.ID, model, ok, order[0])
	}
	b, err := Collect(true, "test")
	if err != nil {
		t.Fatal(err)
	}
	data, err := Seal(b, "provider order passphrase")
	if err != nil {
		t.Fatal(err)
	}
	opened, err := Open(data, "provider order passphrase")
	if err != nil {
		t.Fatal(err)
	}

	home(t)
	if _, err := Restore(opened, Parts{Providers: true}); err != nil {
		t.Fatal(err)
	}
	if p, model, ok := provider.Resolve("order-model"); !ok || p.ID != order[0] || model != "order-model" {
		t.Fatalf("restored routing: provider=%q model=%q ok=%v, want %q", p.ID, model, ok, order[0])
	}
}

// Providers unique to the target stay after the backup's providers, in
// the relative order chosen on the target.
func TestBackupProviderOrderKeepsLocalProviders(t *testing.T) {
	home(t)
	order := addOrderProviders(t)
	slices.Reverse(order)
	if err := provider.SetOrder(order); err != nil {
		t.Fatal(err)
	}
	b, err := Collect(true, "test")
	if err != nil {
		t.Fatal(err)
	}

	home(t)
	local := []string{"local-alpha", "local-beta"}
	for _, id := range local {
		if err := provider.Save(provider.Provider{ID: id, Name: id, Chat: "https://local.example.com/v1", Key: "sk-local"}); err != nil {
			t.Fatal(err)
		}
	}
	addOrderProviders(t)
	if err := provider.SetOrder([]string{local[1], order[1], local[0], order[0]}); err != nil {
		t.Fatal(err)
	}
	r, err := Restore(b, Parts{Providers: true})
	if err != nil {
		t.Fatal(err)
	}
	if r.Added != 0 || r.Replaced != len(order) || len(r.NeedKey) != 0 {
		t.Fatalf("restore result: %+v", r)
	}
	want := append(slices.Clone(order), local[1], local[0])
	if got := listedProviderIDs(); !slices.Equal(got, want) {
		t.Fatalf("restored and local providers: got %v, want %v", got, want)
	}
	stored, err := provider.StoredOrder()
	if err != nil || !slices.Equal(stored, want) {
		t.Fatalf("stored order: got %v, want %v, err=%v", stored, want, err)
	}
	for _, id := range local {
		if p, err := provider.Find(id); err != nil || p.Key != "sk-local" || p.Chat != "https://local.example.com/v1" {
			t.Fatalf("local provider %q changed: %+v, %v", id, p, err)
		}
	}
}

// Picking other parts of an arranged backup leaves providers.json alone.
func TestBackupProviderOrderUnpicked(t *testing.T) {
	home(t)
	ids := addOrderProviders(t)
	if err := provider.SetOrder([]string{ids[1], ids[0]}); err != nil {
		t.Fatal(err)
	}
	b, err := Collect(true, "test")
	if err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(b.Order, []string{ids[1], ids[0]}) {
		t.Fatalf("backup order: %v", b.Order)
	}

	home(t)
	local := addOrderProviders(t)
	if err := provider.SetOrder(local); err != nil {
		t.Fatal(err)
	}
	before, err := os.ReadFile(provider.Path())
	if err != nil {
		t.Fatal(err)
	}
	r, err := Restore(b, Parts{Settings: true, Profiles: true, Agents: true, Library: true})
	if err != nil {
		t.Fatal(err)
	}
	if r.Added != 0 || r.Replaced != 0 || len(r.NeedKey) != 0 {
		t.Fatalf("unpicked providers restored: %+v", r)
	}
	after, err := os.ReadFile(provider.Path())
	if err != nil || !bytes.Equal(before, after) {
		t.Fatalf("unpicked providers changed providers.json: %v", err)
	}
	if got := listedProviderIDs(); !slices.Equal(got, local) {
		t.Fatalf("unpicked providers changed local list: got %v, want %v", got, local)
	}
	stored, err := provider.StoredOrder()
	if err != nil || !slices.Equal(stored, local) {
		t.Fatalf("unpicked providers changed stored order: got %v, want %v, err=%v", stored, local, err)
	}
}

// An order-only restore reports a storage failure instead of silently
// succeeding with the old routing priority.
func TestBackupProviderOrderWriteError(t *testing.T) {
	home(t)
	ids := addOrderProviders(t)
	if err := provider.SetOrder(ids); err != nil {
		t.Fatal(err)
	}
	path := provider.Path()
	before, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	blocked := filepath.Dir(path)
	mode := os.FileMode(0o500)
	if runtime.GOOS == "windows" {
		blocked, mode = path, 0o400
	}
	st, err := os.Stat(blocked)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(blocked, mode); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.Chmod(blocked, st.Mode().Perm()) })
	if runtime.GOOS == "windows" {
		f, err := os.OpenFile(blocked, os.O_WRONLY, 0)
		if err == nil {
			f.Close()
			t.Skip("file remains writable")
		}
		if !errors.Is(err, os.ErrPermission) {
			t.Fatal(err)
		}
	} else {
		f, err := os.CreateTemp(blocked, "order-write-probe-*")
		if err == nil {
			f.Close()
			os.Remove(f.Name())
			t.Skip("directory remains writable")
		}
		if !errors.Is(err, os.ErrPermission) {
			t.Fatal(err)
		}
	}
	r, err := Restore(Bundle{Version: 1, Order: []string{ids[1], ids[0]}}, Parts{Providers: true})
	if !errors.Is(err, os.ErrPermission) {
		t.Fatalf("order write error: got %v, want permission error", err)
	}
	if r.Added != 0 || r.Replaced != 0 {
		t.Fatalf("order-only restore changed provider counts: %+v", r)
	}
	after, err := os.ReadFile(path)
	if err != nil || !bytes.Equal(before, after) {
		t.Fatalf("failed order write changed providers.json: %v", err)
	}
}

func addOrderProviders(t *testing.T) []string {
	t.Helper()
	var ids []string
	for _, p := range []provider.Provider{
		{Name: "Order Alpha", Chat: "https://alpha.example.com/v1", Key: "sk-alpha", Models: []string{"order-model"}},
		{Name: "Order Beta", Chat: "https://beta.example.com/v1", Key: "sk-beta", Models: []string{"order-model"}},
	} {
		id, err := provider.Add(p)
		if err != nil {
			t.Fatal(err)
		}
		ids = append(ids, id)
	}
	return ids
}

func listedProviderIDs() []string {
	var ids []string
	for _, p := range provider.All() {
		ids = append(ids, p.ID)
	}
	return ids
}
