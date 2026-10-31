package usageapi

import (
	"path/filepath"
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/usage"
)

// The Requests page's Gateway key filter lists the keys in the order the
// user gave them on the Gateway page (alextjy on Discord), not by what
// each used; a key removed since goes after them, as it came.
func TestLedgerKeyFilterInKeyOrder(t *testing.T) {
	home := t.TempDir()
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	for _, name := range []string{"Laptop", "Phone", "CI"} {
		if _, err := access.Update("add-key", access.Change{Name: name}); err != nil {
			t.Fatal(err)
		}
	}
	keys, _ := access.List()
	to := 0
	if _, err := access.Update("move-key", access.Change{Key: keys[2].ID, Index: &to}); err != nil {
		t.Fatal(err)
	}
	g := func(id string) Group { return Group{Group: usage.Group{ID: id, CallerKeyID: id}} }
	// by tokens, as the ledger ranks them: gone first, then Phone, Laptop, CI
	got := inKeyOrder([]Group{g("gone"), g(keys[1].ID), g(keys[0].ID), g(keys[2].ID)})
	var ids []string
	for _, x := range got {
		ids = append(ids, x.CallerKeyID)
	}
	if want := []string{keys[2].ID, keys[0].ID, keys[1].ID, "gone"}; !slices.Equal(ids, want) {
		t.Fatalf("filter order %v, want CI, Laptop, Phone, then the removed key: %v", ids, want)
	}
}
