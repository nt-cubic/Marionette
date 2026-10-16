package gui

import (
	"encoding/json"
	"testing"
)

// A save's priceRate (#819): left out keeps the rate, null clears it, a
// number in range is taken and anything else is refused.
func TestPriceRateOf(t *testing.T) {
	for _, c := range []struct {
		raw  string
		r    float64
		keep bool
		bad  bool
	}{
		{"", 0, true, false},
		{"null", 0, false, false},
		{"0.8", 0.8, false, false},
		{"1.5", 1.5, false, false},
		{"0.1234", 0, false, true},
		{"-1", 0, false, true},
		{"1001", 0, false, true},
		{`"x"`, 0, false, true},
	} {
		r, keep, err := priceRateOf(json.RawMessage(c.raw))
		if (err != nil) != c.bad || r != c.r || keep != c.keep {
			t.Errorf("%q: %v %v %v", c.raw, r, keep, err)
		}
	}
}
