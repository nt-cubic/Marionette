package settings

import (
	"slices"
	"strings"
	"testing"
)

// Only warn and block are kept, for a kind masking names; mask, the
// default, is left out, and anything else is refused (#1977).
func TestRedactActions(t *testing.T) {
	got, err := redactActions(map[string]string{"API_KEY": "Block", " EMAIL ": "warn", "PHONE": "mask", "SSN": ""})
	if err != nil || len(got) != 2 || got["API_KEY"] != "block" || got["EMAIL"] != "warn" {
		t.Fatalf("%v %v", got, err)
	}
	if got, err := redactActions(map[string]string{"PHONE": "mask"}); err != nil || got != nil {
		t.Fatalf("all masked: %v %v", got, err)
	}
	if _, err := redactActions(map[string]string{"API_KEY": "drop"}); err == nil {
		t.Fatal("an unknown action was kept")
	}
	if _, err := redactActions(map[string]string{"api key": "warn"}); err == nil {
		t.Fatal("a kind masking can't name was kept")
	}
}

// The allow list is trimmed, each value once whatever its case, and
// bounded.
func TestRedactAllow(t *testing.T) {
	got, err := redactAllow([]string{" ops@acme.io ", "OPS@acme.io", "", "build-01"})
	if err != nil || !slices.Equal(got, []string{"ops@acme.io", "build-01"}) {
		t.Fatalf("%q %v", got, err)
	}
	if _, err := redactAllow([]string{strings.Repeat("x", maxAllowLen+1)}); err == nil {
		t.Fatal("a value too long was kept")
	}
	many := make([]string, maxAllow+1)
	for i := range many {
		many[i] = strings.Repeat("v", i+1)
	}
	if _, err := redactAllow(many); err == nil {
		t.Fatal("too many values were kept")
	}
}
