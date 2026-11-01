package pii

import (
	"runtime"

	"golang.org/x/sys/cpu"
)

// osArch is the system magpie was built for.
var osArch = runtime.GOOS + "-" + runtime.GOARCH

// target is this system's name in serveSHA: the one magpie was built
// for, but Apple silicon's where an Intel magpie runs under Rosetta.
// Rosetta before macOS 15 has no AVX, so the Intel pf-serve ended with
// SIGILL on its first instruction of it, on Metal and on the CPU alike
// (echo_yip on X); Apple silicon's runs there natively, on Metal.
func target() string {
	if osArch == "darwin-amd64" && translated() {
		return "darwin-arm64"
	}
	return osArch
}

// fullX86 says the CPU has what pf-serve is built for on x86 (build.sh's
// x86): AVX, AVX2, FMA and BMI2, with F16C, which every CPU with AVX2
// has. Without them pf-serve ends with SIGILL (a Celeron, a Pentium, a
// Mac before Haswell), and pf-serve-base, built for SSE4.2 alone, is run
// instead. Off x86 it is true: no other system has a pf-serve-base.
var fullX86 = func() bool {
	if runtime.GOARCH != "amd64" {
		return true
	}
	return cpu.X86.HasAVX && cpu.X86.HasAVX2 && cpu.X86.HasFMA && cpu.X86.HasBMI2
}
