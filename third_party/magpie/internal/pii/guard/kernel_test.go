package guard

import (
	"fmt"
	"math"
	"math/rand"
	"testing"
)

// The kernels in kernel_*.s give what the Go beside them gives: int8
// exactly, float32 to rounding. Each case is run with the kernel and with
// it switched off.
func TestKernelsMatchGo(t *testing.T) {
	if dot4fast == nil || scoresFast == nil || mixFast == nil {
		t.Log("no kernels on this CPU; the Go runs both sides")
	}
	r := rand.New(rand.NewSource(1))
	for _, k := range []int{16, 32, 384, 1536, 40} {
		x := make([]int8, k)
		w := make([]int8, 4*k)
		for i := range x {
			x[i] = int8(r.Intn(256) - 128)
		}
		for i := range w {
			w[i] = int8(r.Intn(256) - 128)
		}
		x[0], w[0] = -128, -128 // the extremes
		var got, want [4]int32
		dot4(x, w, &got)
		f := dot4fast
		dot4fast = nil
		dot4(x, w, &want)
		dot4fast = f
		if got != want {
			t.Fatalf("dot4 over %d: %v, Go %v", k, got, want)
		}
	}
	for _, n := range []int{1, 7, 16, 37, 512} {
		np := (n + 15) &^ 15
		var q [32]float32
		for i := range q {
			q[i] = r.Float32()*2 - 1
		}
		kt := make([]float32, 32*np)
		for i := range kt {
			kt[i] = r.Float32()*2 - 1
		}
		got, want := make([]float32, np), make([]float32, np)
		scores(&q, kt, got)
		f := scoresFast
		scoresFast = nil
		scores(&q, kt, want)
		scoresFast = f
		for u := range got {
			if math.Abs(float64(got[u]-want[u])) > 1e-4 {
				t.Fatalf("scores over %d keys, [%d]: %v, Go %v", n, u, got[u], want[u])
			}
		}
		p := make([]float32, n)
		for i := range p {
			p[i] = r.Float32()
		}
		v := make([]float32, 32*n)
		for i := range v {
			v[i] = r.Float32()*2 - 1
		}
		var c1, c2 [32]float32
		c1[31], c2[31] = 1, 1 // added to, not overwritten
		mix(&c1, p, v)
		m := mixFast
		mixFast = nil
		mix(&c2, p, v)
		mixFast = m
		for i := range c1 {
			if math.Abs(float64(c1[i]-c2[i])) > 1e-3 {
				t.Fatalf("mix over %d, [%d]: %v, Go %v", n, i, c1[i], c2[i])
			}
		}
	}
}

func TestExpfAndGelu(t *testing.T) {
	for x := float32(-87); x <= 0; x += 0.0013 {
		w := math.Exp(float64(x))
		if e := math.Abs(float64(expf(x))-w) / w; e > 3e-7 {
			t.Fatalf("expf(%v) = %v, want %v", x, expf(x), w)
		}
	}
	if expf(-100) != 0 {
		t.Fatal("expf(-100) not 0")
	}
	for x := float32(-10); x <= 10; x += 0.0007 {
		w := 0.5 * float64(x) * (1 + math.Erf(float64(x)/math.Sqrt2))
		if e := math.Abs(float64(gelu(x)) - w); e > 2e-6 {
			t.Fatalf("gelu(%v) = %v, want %v", x, gelu(x), w)
		}
	}
}

// The softmax's exponentials, the GELU and the scores give the Go's very
// bits with the kernels (AVX2 on amd64), not only values near them: what
// the model finds, and so what is masked, is the same on every CPU.
func TestKernelsSameBits(t *testing.T) {
	if expSubFast == nil || geluFast == nil {
		t.Log("no exp or gelu kernel on this CPU; the Go runs both sides")
	}
	r := rand.New(rand.NewSource(2))
	same := func(what string, got, want []float32) {
		t.Helper()
		for i := range got {
			if math.Float32bits(got[i]) != math.Float32bits(want[i]) {
				t.Fatalf("%s [%d]: %v (%#x), Go %v (%#x)", what, i, got[i], math.Float32bits(got[i]), want[i], math.Float32bits(want[i]))
			}
		}
	}
	edges := []float32{0, float32(math.Copysign(0, -1)), -87.2, -87.3, -87.4, -100, -1e30, 88.6, 88.7, 88.8, 1e-40, -1e-40, 1, -1}
	for _, n := range []int{1, 8, 13, 512, 1536 + 5} {
		x := make([]float32, n)
		for i := range x {
			switch {
			case i < len(edges) && n > len(edges):
				x[i] = edges[i]
			case i%3 == 0:
				x[i] = -r.Float32() * 90
			default:
				x[i] = (r.Float32()*2 - 1) * 12
			}
		}
		mx := float32(r.Float32())
		got, want := append([]float32(nil), x...), append([]float32(nil), x...)
		expSub(got, mx)
		f := expSubFast
		expSubFast = nil
		expSub(want, mx)
		expSubFast = f
		same(fmt.Sprintf("expSub over %d", n), got, want)

		bias, smooth := make([]float32, n), make([]float32, n)
		for i := range bias {
			bias[i] = r.Float32()*0.2 - 0.1
			smooth[i] = r.Float32() + 0.5
		}
		got, want = append([]float32(nil), x...), append([]float32(nil), x...)
		geluRow(got, bias, smooth)
		g := geluFast
		geluFast = nil
		geluRow(want, bias, smooth)
		geluFast = g
		same(fmt.Sprintf("geluRow over %d", n), got, want)
	}
	for _, n := range []int{1, 16, 17, 33, 48, 512} {
		np := (n + 15) &^ 15
		var q [32]float32
		for i := range q {
			q[i] = r.Float32()*2 - 1
		}
		kt := make([]float32, 32*np)
		for i := range kt {
			kt[i] = r.Float32()*2 - 1
		}
		got, want := make([]float32, np), make([]float32, np)
		scores(&q, kt, got)
		f := scoresFast
		scoresFast = nil
		scores(&q, kt, want)
		scoresFast = f
		same(fmt.Sprintf("scores over %d keys", n), got, want)
	}
}

// maxOf and rangeOf give what the builtins give, to a zero's sign: NaN
// for a NaN anywhere.
func TestMaxAndRangeAsBuiltins(t *testing.T) {
	r := rand.New(rand.NewSource(3))
	nan, neg0 := float32(math.NaN()), float32(math.Copysign(0, -1))
	for c := 0; c < 2000; c++ {
		x := make([]float32, r.Intn(40))
		for i := range x {
			switch r.Intn(12) {
			case 0:
				x[i] = 0
			case 1:
				x[i] = neg0
			case 2:
				if c%10 == 0 {
					x[i] = nan
				}
			default:
				x[i] = (r.Float32()*2 - 1) * 50
			}
		}
		mx, lo, hi := float32(math.Inf(-1)), float32(0), float32(0)
		for _, v := range x {
			mx = max(mx, v)
			lo, hi = min(lo, v), max(hi, v)
		}
		eq := func(a, b float32) bool { return a == b || a != a && b != b }
		gl, gh := rangeOf(x)
		if !eq(maxOf(x), mx) || !eq(gl, lo) || !eq(gh, hi) {
			t.Fatalf("%v: maxOf %v rangeOf %v %v, builtins %v %v %v", x, maxOf(x), gl, gh, mx, lo, hi)
		}
	}
}
