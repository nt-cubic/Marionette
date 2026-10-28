package gateway

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// 墨雨藏锋 / mycf_711 (#3950): SuperMagic's accounts are prepaid wallets of
// points with no total. Under smart, pace and least used the account with
// 6,291 points left goes before one with 750, whatever their order; the
// Routing page is told each one's figure, not a share. A balance in
// another unit, or another provider's, is never weighed against it.
func TestWalletWithMoreLeftGoesFirst(t *testing.T) {
	old := allowances
	defer func() { allowances = old }()
	share := map[string]provider.Allowance{}
	allowances = func(string) map[string]provider.Allowance { return share }
	wallet := func(left float64, unit string) provider.Allowance {
		return provider.Allowance{{Name: "积分", Balance: &left, Unit: unit}}
	}
	acct := func(agent, user string) candidate {
		return candidate{p: provider.Provider{ID: agent, Account: &provider.Account{Agent: agent, User: user}}, model: "m", rest: agent + "#" + user}
	}
	for _, routing := range []string{"", provider.Pace, provider.LeastUsed} {
		p := provider.Provider{ID: "magicrew", Routing: routing}
		order := func(cs ...candidate) (string, weighing, []candidate) {
			got, wg := weigh(p, cs, "m", provider.Chat)
			var s []string
			for _, c := range got {
				s = append(s, c.p.Account.User)
			}
			return strings.Join(s, ","), wg, got
		}
		share["small"], share["big"] = wallet(750, "积分"), wallet(6291, "积分")
		got, wg, cs := order(acct("magicrew", "small"), acct("magicrew", "big"))
		if got != "big,small" {
			t.Errorf("%q: %s, want big,small", routing, got)
		}
		if w := weighed(cs[0], p, wg, false, provider.Chat); !w.Known || w.Balance == nil || *w.Balance != 6291 || w.Unit != "积分" || w.Used != 0 {
			t.Errorf("%q: the Routing page's row: %+v", routing, w)
		}
		share["big"] = wallet(6291, "credits")
		if got, _, _ := order(acct("magicrew", "small"), acct("magicrew", "big")); got != "small,big" {
			t.Errorf("%q, another unit: %s, want their order", routing, got)
		}
		share["big"] = wallet(6291, "积分")
		if got, _, _ := order(acct("magicrew", "small"), acct("othercrew", "big")); got != "small,big" {
			t.Errorf("%q, another provider: %s, want their order", routing, got)
		}
	}
}
