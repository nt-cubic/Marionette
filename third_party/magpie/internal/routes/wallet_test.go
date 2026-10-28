package routes

import (
	"slices"
	"testing"
)

// Prepaid wallets (Seat.Left, #3950) tie on everything Smart, Pace and
// Least used go by, a wallet having no share and no renewal: the one with
// more left goes first, one that told none after them. Balances in two
// providers' or two units' wallets are never weighed against each other:
// the pool keeps its order.
func TestWalletsGoByWhatTheyHaveLeft(t *testing.T) {
	const pts = "magicrew\x00积分"
	wallet := func(key string, left float64, in string) Seat {
		return Seat{Key: key, Account: true, Known: true, Pace: 100.0 / 168, Left: left, LeftIn: in}
	}
	for _, st := range []struct {
		name string
		s    Strategy
	}{{"smart", Smart}, {"pace", Pace}, {"usage", LeastUsed}} {
		s := []Seat{wallet("small", 750, pts), wallet("none", 0, ""), wallet("big", 6291, pts)}
		if got, want := keys(s, Apply(st.s, "p", s)), []string{"big", "small", "none"}; !slices.Equal(got, want) {
			t.Errorf("%s: %v, want %v", st.name, got, want)
		}
		// an empty wallet is a reading of nothing left, not one not told
		s = []Seat{wallet("empty", 0, pts), wallet("some", 1, pts)}
		if got, want := keys(s, Apply(st.s, "p", s)), []string{"some", "empty"}; !slices.Equal(got, want) {
			t.Errorf("%s, empty: %v, want %v", st.name, got, want)
		}
		for _, other := range []string{"magicrew\x00credits", "other\x00积分"} {
			s = []Seat{wallet("small", 750, pts), wallet("big", 6291, other)}
			if got, want := keys(s, Apply(st.s, "p", s)), []string{"small", "big"}; !slices.Equal(got, want) {
				t.Errorf("%s across %q: %v, want %v", st.name, other, got, want)
			}
		}
	}
}

// A wallet's balance never jumps the tiers: one used up (an empty wallet
// counts as all used) or running low waits behind one with room, however
// much it says it has left.
func TestWalletLeftWithinItsTier(t *testing.T) {
	const pts = "magicrew\x00积分"
	s := []Seat{
		{Key: "low", Known: true, Used: 95, Left: 9000, LeftIn: pts},
		{Key: "room", Known: true, Used: 0, Left: 10, LeftIn: pts},
	}
	for _, st := range []Strategy{Smart, Pace, LeastUsed} {
		if got := keys(s, Apply(st, "p", s)); got[0] != "room" {
			t.Errorf("tier: %v", got)
		}
	}
}
