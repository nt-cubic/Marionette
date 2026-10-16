package settings

import (
	"slices"
	"strings"
	"testing"
)

// The allow list takes IPs and CIDR ranges, v4 and v6, writes each in one
// form, and refuses anything else with the forms it takes.
func TestCleanAllowIPs(t *testing.T) {
	got, err := CleanAllowIPs([]string{" 203.0.113.7 ", "10.1.2.3/8", "2001:DB8::/32", "::ffff:198.51.100.1", "::ffff:198.51.100.0/120", "[2001:db8::5]", "203.0.113.7/32", ""})
	if err != nil {
		t.Fatal(err)
	}
	want := []string{"203.0.113.7", "10.0.0.0/8", "2001:db8::/32", "198.51.100.1", "198.51.100.0/24", "2001:db8::5"}
	if !slices.Equal(got, want) {
		t.Errorf("got %q, want %q", got, want)
	}
	for _, bad := range []string{"localhost", "10.0.0.0/33", "1.2.3", "fe80::1%en0", "fe80::/10%en0", "http://1.2.3.4"} {
		if _, err := CleanAllowIPs([]string{bad}); err == nil || !strings.Contains(err.Error(), "is not an IP address") {
			t.Errorf("%q: %v", bad, err)
		}
	}
	if got := SplitAllowIPs("1.2.3.4,10.0.0.0/8 ; 2001:db8::/32\n"); !slices.Equal(got, []string{"1.2.3.4", "10.0.0.0/8", "2001:db8::/32"}) {
		t.Errorf("split %q", got)
	}
}
