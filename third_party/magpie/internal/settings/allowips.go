package settings

import (
	"fmt"
	"net/netip"
	"slices"
	"strings"
)

// SplitAllowIPs splits MAGPIE_ALLOW_IPS ("1.2.3.4,10.0.0.0/8 2001:db8::/32")
// into its entries, at commas and spaces.
func SplitAllowIPs(v string) []string {
	return strings.FieldsFunc(v, func(r rune) bool { return r == ',' || r == ';' || r == ' ' || r == '\t' || r == '\n' })
}

// ParseAllowIP reads one entry of the gateway's allow list: an IP address
// (IPv4 or IPv6, for itself alone) or a range in CIDR form. An IPv4
// address written as IPv6 (::ffff:1.2.3.4) is the IPv4 one.
func ParseAllowIP(raw string) (netip.Prefix, error) {
	raw = strings.TrimSpace(raw)
	bad := fmt.Errorf("%q is not an IP address or range: write it as 203.0.113.7, 10.0.0.0/8 or 2001:db8::/32", raw)
	if raw == "" {
		return netip.Prefix{}, bad
	}
	if strings.Contains(raw, "/") {
		p, err := netip.ParsePrefix(raw)
		if err != nil || p.Addr().Zone() != "" {
			return netip.Prefix{}, bad
		}
		if p.Addr().Is4In6() && p.Bits() >= 96 {
			p = netip.PrefixFrom(p.Addr().Unmap(), p.Bits()-96)
		}
		return p.Masked(), nil
	}
	a, err := netip.ParseAddr(strings.Trim(raw, "[]"))
	if err != nil || a.Zone() != "" {
		return netip.Prefix{}, bad
	}
	a = a.Unmap()
	return netip.PrefixFrom(a, a.BitLen()), nil
}

// CleanAllowIPs checks each entry of an allow list and writes it in one
// form (an address alone, a range by its first address), without repeats.
func CleanAllowIPs(in []string) ([]string, error) {
	var out []string
	for _, raw := range in {
		if strings.TrimSpace(raw) == "" {
			continue
		}
		p, err := ParseAllowIP(raw)
		if err != nil {
			return nil, err
		}
		s := p.String()
		if p.IsSingleIP() {
			s = p.Addr().String()
		}
		if !slices.Contains(out, s) {
			out = append(out, s)
		}
	}
	return out, nil
}
