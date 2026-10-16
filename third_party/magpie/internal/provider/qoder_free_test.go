package provider

import (
	"encoding/json"
	"testing"
)

// A saved account's listing, read back without fetching again, keeps
// Qoder's free models marked free.
func TestQoderSavedModelsFree(t *testing.T) {
	signIn(t)
	c := qoderTestCredential("free")
	c.Models = json.RawMessage(`{"chat":[{"key":"lite","enable":true,"price_factor":0},{"key":"pro","enable":true,"price_factor":1}]}`)
	if err := qoderSave(c); err != nil {
		t.Fatal(err)
	}
	p, ok := qoderAccount()
	if !ok {
		t.Fatal("no Qoder account")
	}
	free := map[string]bool{}
	for _, m := range p.Account.models() {
		free[m.ID] = m.Free
	}
	if len(free) != 2 || !free["lite"] || free["pro"] {
		t.Fatalf("saved models free = %v", free)
	}
}
