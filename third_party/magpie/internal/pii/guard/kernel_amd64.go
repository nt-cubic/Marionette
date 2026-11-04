package guard

import (
	"math"

	"golang.org/x/sys/cpu"
)

//go:noescape
func dot4sse(a, b *int8, k int, out *[4]int32)

//go:noescape
func scoresT(q, kt *float32, np int, out *float32)

//go:noescape
func mix32(c, p, v *float32, n int)

//go:noescape
func dot4avx2(a, b *int8, k int, out *[4]int32)

//go:noescape
func scoresAVX2(q, kt *float32, np int, out *float32)

//go:noescape
func expSubAVX2(p *float32, n int, mx float32, k *[23][8]uint32)

//go:noescape
func geluAVX2(row, bias, smooth *float32, n int, k *[23][8]uint32)

// avxK are expf's and gelu's constants as float32 bits, eight lanes each,
// in the order kernel_amd64.s reads them.
var avxK = func() (k [23][8]uint32) {
	f := math.Float32bits
	for i, v := range [23]uint32{
		f(1.44269504088896341), f(0.5), f(0.693359375), f(-2.12194440e-4),
		f(1.9875691500e-4), f(1.3981999507e-3), f(8.3334519073e-3), f(4.1665795894e-2), f(1.6666665459e-1), f(5.0000001201e-1),
		f(1), f(-87.3), f(88.7), 0x7f800000, 0x80000000,
		f(0.70710678118654752), f(0.3275911),
		f(1.061405429), f(1.453152027), f(1.421413741), f(0.284496736), f(0.254829592),
		0x7fffffff,
	} {
		for j := range k[i] {
			k[i][j] = v
		}
	}
	return
}()

// useAVX2 says the kernels below run on AVX2: a var for tests.
var useAVX2 = cpu.X86.HasAVX2

func init() { pickKernels() }

// pickKernels sets the fast kernels for this CPU.
func pickKernels() {
	dot4fast = func(a, b []int8, out *[4]int32) {
		_ = b[4*len(a)-1]
		dot4sse(&a[0], &b[0], len(a), out)
	}
	scoresFast = func(q *[32]float32, kt []float32, out []float32) {
		_ = kt[32*len(out)-1]
		scoresT(&q[0], &kt[0], len(out), &out[0])
	}
	mixFast = func(c *[32]float32, p, v []float32) {
		_ = v[32*len(p)-1]
		mix32(&c[0], &p[0], &v[0], len(p))
	}
	expSubFast, geluFast = nil, nil
	// AVX2 (Haswell, Zen and later): twice the lanes for the int8 dot
	// products, the scores, the softmax's exponentials and the GELU, where
	// a 2-core server read 4 KB of a tool result in about 4 s on SSE2
	// (mrwl666 on Discord, 11-01: a core at 100% for 2-3 s a request).
	if !useAVX2 {
		return
	}
	dot4fast = func(a, b []int8, out *[4]int32) {
		_ = b[4*len(a)-1]
		dot4avx2(&a[0], &b[0], len(a), out)
	}
	scoresFast = func(q *[32]float32, kt []float32, out []float32) {
		_ = kt[32*len(out)-1]
		scoresAVX2(&q[0], &kt[0], len(out), &out[0])
	}
	expSubFast = func(p []float32, mx float32) {
		expSubAVX2(&p[0], len(p), mx, &avxK)
	}
	geluFast = func(row, bias, smooth []float32) {
		_, _ = bias[len(row)-1], smooth[len(row)-1]
		geluAVX2(&row[0], &bias[0], &smooth[0], len(row), &avxK)
	}
}
