package coreml

import "testing"

// Most is the device with the largest share, none where no share is.
func TestMost(t *testing.T) {
	for _, c := range []struct {
		sh   [3]float64
		want string
	}{
		{[3]float64{.9, .05, .05}, "Neural Engine"},
		{[3]float64{.1, .7, .2}, "GPU"},
		{[3]float64{0, 0, 1}, "CPU"},
		{[3]float64{}, ""},
	} {
		if got := Most(c.sh); got != c.want {
			t.Errorf("Most(%v) = %q, want %q", c.sh, got, c.want)
		}
	}
}
