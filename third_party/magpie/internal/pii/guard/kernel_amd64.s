#include "textflag.h"

// The kernels here use SSE2 only, which every amd64 has.

// func dot4sse(a, b *int8, k int, out *[4]int32)
// out[j] = Σ a[i]*b[j*k+i] for i < k, k a multiple of 16.
TEXT ·dot4sse(SB), NOSPLIT, $0-32
	MOVQ a+0(FP), BX
	MOVQ b+8(FP), SI
	MOVQ k+16(FP), DX
	MOVQ out+24(FP), DI
	LEAQ (SI)(DX*1), R8
	LEAQ (R8)(DX*1), R9
	LEAQ (R9)(DX*1), R10
	PXOR X8, X8
	PXOR X9, X9
	PXOR X10, X10
	PXOR X11, X11
	XORQ CX, CX
	TESTQ DX, DX
	JEQ  ddone
dloop:
	MOVOU (BX)(CX*1), X1
	MOVO  X1, X2
	PUNPCKLBW X1, X1
	PUNPCKHBW X2, X2
	PSRAW $8, X1
	PSRAW $8, X2
	MOVOU (SI)(CX*1), X3
	MOVO  X3, X4
	PUNPCKLBW X3, X3
	PUNPCKHBW X4, X4
	PSRAW $8, X3
	PSRAW $8, X4
	PMADDWL X1, X3
	PMADDWL X2, X4
	PADDL X3, X8
	PADDL X4, X8
	MOVOU (R8)(CX*1), X3
	MOVO  X3, X4
	PUNPCKLBW X3, X3
	PUNPCKHBW X4, X4
	PSRAW $8, X3
	PSRAW $8, X4
	PMADDWL X1, X3
	PMADDWL X2, X4
	PADDL X3, X9
	PADDL X4, X9
	MOVOU (R9)(CX*1), X3
	MOVO  X3, X4
	PUNPCKLBW X3, X3
	PUNPCKHBW X4, X4
	PSRAW $8, X3
	PSRAW $8, X4
	PMADDWL X1, X3
	PMADDWL X2, X4
	PADDL X3, X10
	PADDL X4, X10
	MOVOU (R10)(CX*1), X3
	MOVO  X3, X4
	PUNPCKLBW X3, X3
	PUNPCKHBW X4, X4
	PSRAW $8, X3
	PSRAW $8, X4
	PMADDWL X1, X3
	PMADDWL X2, X4
	PADDL X3, X11
	PADDL X4, X11
	ADDQ $16, CX
	CMPQ CX, DX
	JLT  dloop
ddone:
	PSHUFD $0x4e, X8, X0
	PADDL  X0, X8
	PSHUFD $0xb1, X8, X0
	PADDL  X0, X8
	MOVQ   X8, AX
	MOVL   AX, 0(DI)
	PSHUFD $0x4e, X9, X0
	PADDL  X0, X9
	PSHUFD $0xb1, X9, X0
	PADDL  X0, X9
	MOVQ   X9, AX
	MOVL   AX, 4(DI)
	PSHUFD $0x4e, X10, X0
	PADDL  X0, X10
	PSHUFD $0xb1, X10, X0
	PADDL  X0, X10
	MOVQ   X10, AX
	MOVL   AX, 8(DI)
	PSHUFD $0x4e, X11, X0
	PADDL  X0, X11
	PSHUFD $0xb1, X11, X0
	PADDL  X0, X11
	MOVQ   X11, AX
	MOVL   AX, 12(DI)
	RET

// func scoresT(q, kt *float32, np int, out *float32)
// out[u] = Σ q[i]*kt[i*np+u] for i < 32, u < np, np a multiple of 16.
TEXT ·scoresT(SB), NOSPLIT, $0-32
	MOVQ q+0(FP), SI
	MOVQ kt+8(FP), BX
	MOVQ np+16(FP), DX
	MOVQ out+24(FP), DI
	MOVQ DX, R8
	SHLQ $2, R8 // a row of kt in bytes
	TESTQ DX, DX
	JEQ  tdone
tblock:
	XORPS X0, X0
	XORPS X1, X1
	XORPS X2, X2
	XORPS X3, X3
	MOVQ SI, R9
	MOVQ BX, R10
	MOVQ $32, CX
trow:
	MOVSS  (R9), X4
	SHUFPS $0, X4, X4
	MOVUPS (R10), X5
	MOVUPS 16(R10), X6
	MOVUPS 32(R10), X7
	MOVUPS 48(R10), X8
	MULPS  X4, X5
	MULPS  X4, X6
	MULPS  X4, X7
	MULPS  X4, X8
	ADDPS  X5, X0
	ADDPS  X6, X1
	ADDPS  X7, X2
	ADDPS  X8, X3
	ADDQ   $4, R9
	ADDQ   R8, R10
	DECQ   CX
	JNE    trow
	MOVUPS X0, (DI)
	MOVUPS X1, 16(DI)
	MOVUPS X2, 32(DI)
	MOVUPS X3, 48(DI)
	ADDQ $64, DI
	ADDQ $64, BX
	SUBQ $16, DX
	JNE  tblock
tdone:
	RET

// func mix32(c, p, v *float32, n int)
// c[i] += Σ p[u]*v[u*32+i] for u < n, i < 32.
TEXT ·mix32(SB), NOSPLIT, $0-32
	MOVQ c+0(FP), DI
	MOVQ p+8(FP), SI
	MOVQ v+16(FP), DX
	MOVQ n+24(FP), CX
	MOVUPS (DI), X0
	MOVUPS 16(DI), X1
	MOVUPS 32(DI), X2
	MOVUPS 48(DI), X3
	MOVUPS 64(DI), X4
	MOVUPS 80(DI), X5
	MOVUPS 96(DI), X6
	MOVUPS 112(DI), X7
	TESTQ CX, CX
	JEQ  mdone
mloop:
	MOVSS  (SI), X15
	SHUFPS $0, X15, X15
	MOVUPS 0(DX), X8
	MULPS  X15, X8
	ADDPS  X8, X0
	MOVUPS 16(DX), X9
	MULPS  X15, X9
	ADDPS  X9, X1
	MOVUPS 32(DX), X8
	MULPS  X15, X8
	ADDPS  X8, X2
	MOVUPS 48(DX), X9
	MULPS  X15, X9
	ADDPS  X9, X3
	MOVUPS 64(DX), X8
	MULPS  X15, X8
	ADDPS  X8, X4
	MOVUPS 80(DX), X9
	MULPS  X15, X9
	ADDPS  X9, X5
	MOVUPS 96(DX), X8
	MULPS  X15, X8
	ADDPS  X8, X6
	MOVUPS 112(DX), X9
	MULPS  X15, X9
	ADDPS  X9, X7
	ADDQ $4, SI
	ADDQ $128, DX
	DECQ CX
	JNE  mloop
mdone:
	MOVUPS X0, (DI)
	MOVUPS X1, 16(DI)
	MOVUPS X2, 32(DI)
	MOVUPS X3, 48(DI)
	MOVUPS X4, 64(DI)
	MOVUPS X5, 80(DI)
	MOVUPS X6, 96(DI)
	MOVUPS X7, 112(DI)
	RET

// The kernels below need AVX2 (kernel_amd64.go checks for it). Each does
// what the one above it, or the Go in kernel.go, does: the int8 sums
// exactly, the float32 ones lane by lane with the same operations in the
// same order and no fused multiply-add, so the same bits.

// func dot4avx2(a, b *int8, k int, out *[4]int32)
// out[j] = Σ a[i]*b[j*k+i] for i < k, k a multiple of 32.
TEXT ·dot4avx2(SB), NOSPLIT, $0-32
	MOVQ a+0(FP), BX
	MOVQ b+8(FP), SI
	MOVQ k+16(FP), DX
	MOVQ out+24(FP), DI
	LEAQ (SI)(DX*1), R8
	LEAQ (R8)(DX*1), R9
	LEAQ (R9)(DX*1), R10
	VPXOR Y8, Y8, Y8
	VPXOR Y9, Y9, Y9
	VPXOR Y10, Y10, Y10
	VPXOR Y11, Y11, Y11
	XORQ CX, CX
	TESTQ DX, DX
	JEQ  vdone
vloop:
	VPMOVSXBW (BX)(CX*1), Y0
	VPMOVSXBW 16(BX)(CX*1), Y1
	VPMOVSXBW (SI)(CX*1), Y2
	VPMOVSXBW 16(SI)(CX*1), Y3
	VPMADDWD Y0, Y2, Y2
	VPMADDWD Y1, Y3, Y3
	VPADDD Y2, Y8, Y8
	VPADDD Y3, Y8, Y8
	VPMOVSXBW (R8)(CX*1), Y4
	VPMOVSXBW 16(R8)(CX*1), Y5
	VPMADDWD Y0, Y4, Y4
	VPMADDWD Y1, Y5, Y5
	VPADDD Y4, Y9, Y9
	VPADDD Y5, Y9, Y9
	VPMOVSXBW (R9)(CX*1), Y2
	VPMOVSXBW 16(R9)(CX*1), Y3
	VPMADDWD Y0, Y2, Y2
	VPMADDWD Y1, Y3, Y3
	VPADDD Y2, Y10, Y10
	VPADDD Y3, Y10, Y10
	VPMOVSXBW (R10)(CX*1), Y4
	VPMOVSXBW 16(R10)(CX*1), Y5
	VPMADDWD Y0, Y4, Y4
	VPMADDWD Y1, Y5, Y5
	VPADDD Y4, Y11, Y11
	VPADDD Y5, Y11, Y11
	ADDQ $32, CX
	CMPQ CX, DX
	JLT  vloop
vdone:
	VEXTRACTI128 $1, Y8, X0
	VPADDD X0, X8, X8
	VPSHUFD $0x4e, X8, X0
	VPADDD X0, X8, X8
	VPSHUFD $0xb1, X8, X0
	VPADDD X0, X8, X8
	VMOVD X8, 0(DI)
	VEXTRACTI128 $1, Y9, X0
	VPADDD X0, X9, X9
	VPSHUFD $0x4e, X9, X0
	VPADDD X0, X9, X9
	VPSHUFD $0xb1, X9, X0
	VPADDD X0, X9, X9
	VMOVD X9, 4(DI)
	VEXTRACTI128 $1, Y10, X0
	VPADDD X0, X10, X10
	VPSHUFD $0x4e, X10, X0
	VPADDD X0, X10, X10
	VPSHUFD $0xb1, X10, X0
	VPADDD X0, X10, X10
	VMOVD X10, 8(DI)
	VEXTRACTI128 $1, Y11, X0
	VPADDD X0, X11, X11
	VPSHUFD $0x4e, X11, X0
	VPADDD X0, X11, X11
	VPSHUFD $0xb1, X11, X0
	VPADDD X0, X11, X11
	VMOVD X11, 12(DI)
	VZEROUPPER
	RET

// func scoresAVX2(q, kt *float32, np int, out *float32)
// scoresT, 32 keys a block where it can, 16 where not.
TEXT ·scoresAVX2(SB), NOSPLIT, $0-32
	MOVQ q+0(FP), SI
	MOVQ kt+8(FP), BX
	MOVQ np+16(FP), DX
	MOVQ out+24(FP), DI
	MOVQ DX, R8
	SHLQ $2, R8 // a row of kt in bytes
sblock:
	CMPQ DX, $32
	JLT  shalf
	VXORPS Y0, Y0, Y0
	VXORPS Y1, Y1, Y1
	VXORPS Y2, Y2, Y2
	VXORPS Y3, Y3, Y3
	MOVQ SI, R9
	MOVQ BX, R10
	MOVQ $32, CX
srow:
	VBROADCASTSS (R9), Y4
	VMULPS (R10), Y4, Y5
	VMULPS 32(R10), Y4, Y6
	VMULPS 64(R10), Y4, Y7
	VMULPS 96(R10), Y4, Y8
	VADDPS Y5, Y0, Y0
	VADDPS Y6, Y1, Y1
	VADDPS Y7, Y2, Y2
	VADDPS Y8, Y3, Y3
	ADDQ $4, R9
	ADDQ R8, R10
	DECQ CX
	JNE  srow
	VMOVUPS Y0, (DI)
	VMOVUPS Y1, 32(DI)
	VMOVUPS Y2, 64(DI)
	VMOVUPS Y3, 96(DI)
	ADDQ $128, DI
	ADDQ $128, BX
	SUBQ $32, DX
	JMP  sblock
shalf:
	TESTQ DX, DX
	JEQ  sdone
	VXORPS Y0, Y0, Y0
	VXORPS Y1, Y1, Y1
	MOVQ SI, R9
	MOVQ BX, R10
	MOVQ $32, CX
shrow:
	VBROADCASTSS (R9), Y4
	VMULPS (R10), Y4, Y5
	VMULPS 32(R10), Y4, Y6
	VADDPS Y5, Y0, Y0
	VADDPS Y6, Y1, Y1
	ADDQ $4, R9
	ADDQ R8, R10
	DECQ CX
	JNE  shrow
	VMOVUPS Y0, (DI)
	VMOVUPS Y1, 32(DI)
sdone:
	VZEROUPPER
	RET

// EXPF sets R to expf(X), lane by lane, as kernel.go's expf: k (R11) holds
// avxK's constants, eight lanes each. T1..T3 are overwritten.
#define EXPF(X, R, T1, T2, T3) \
	VMULPS 0(R11), X, T1 \
	; VADDPS 32(R11), T1, T1 \
	; VROUNDPS $1, T1, T1 \
	; VMULPS 64(R11), T1, T2 \
	; VSUBPS T2, X, T2 \
	; VMULPS 96(R11), T1, T3 \
	; VSUBPS T3, T2, T2 \
	; VMULPS 128(R11), T2, R \
	; VADDPS 160(R11), R, R \
	; VMULPS T2, R, R \
	; VADDPS 192(R11), R, R \
	; VMULPS T2, R, R \
	; VADDPS 224(R11), R, R \
	; VMULPS T2, R, R \
	; VADDPS 256(R11), R, R \
	; VMULPS T2, R, R \
	; VADDPS 288(R11), R, R \
	; VMULPS T2, T2, T3 \
	; VMULPS T3, R, R \
	; VADDPS T2, R, R \
	; VADDPS 320(R11), R, R \
	; VCVTTPS2DQ T1, T1 \
	; VPSLLD $23, T1, T1 \
	; VPADDD T1, R, R \
	; VCMPPS $1, 352(R11), X, T3 \
	; VANDNPS R, T3, R \
	; VCMPPS $14, 384(R11), X, T3 \
	; VBLENDVPS T3, 416(R11), R, R

// func expSubAVX2(p *float32, n int, mx float32, k *[23][8]uint32)
// p[u] = expf(p[u]-mx) for u < n, n a multiple of 8.
TEXT ·expSubAVX2(SB), NOSPLIT, $0-32
	MOVQ p+0(FP), DI
	MOVQ n+8(FP), CX
	VBROADCASTSS mx+16(FP), Y7
	MOVQ k+24(FP), R11
	TESTQ CX, CX
	JEQ  edone
eloop:
	VMOVUPS (DI), Y0
	VSUBPS Y7, Y0, Y0
	EXPF(Y0, Y1, Y2, Y3, Y4)
	VMOVUPS Y1, (DI)
	ADDQ $32, DI
	SUBQ $8, CX
	JNE  eloop
edone:
	VZEROUPPER
	RET

// func geluAVX2(row, bias, smooth *float32, n int, k *[23][8]uint32)
// row[i] = gelu(row[i]+bias[i])*smooth[i] for i < n, n a multiple of 8.
TEXT ·geluAVX2(SB), NOSPLIT, $0-40
	MOVQ row+0(FP), DI
	MOVQ bias+8(FP), SI
	MOVQ smooth+16(FP), DX
	MOVQ n+24(FP), CX
	MOVQ k+32(FP), R11
	TESTQ CX, CX
	JEQ  gdone
gloop:
	VMOVUPS (DI), Y0
	VADDPS (SI), Y0, Y0         // x = row + bias
	VMULPS 480(R11), Y0, Y5     // z = x·(1/√2)
	VANDPS 704(R11), Y5, Y6     // a = |z|
	VMULPS 512(R11), Y6, Y8
	VADDPS 320(R11), Y8, Y8
	VMOVUPS 320(R11), Y9
	VDIVPS Y8, Y9, Y9           // t = 1/(1+p·a)
	VMULPS 544(R11), Y9, Y10
	VSUBPS 576(R11), Y10, Y10
	VMULPS Y9, Y10, Y10
	VADDPS 608(R11), Y10, Y10
	VMULPS Y9, Y10, Y10
	VSUBPS 640(R11), Y10, Y10
	VMULPS Y9, Y10, Y10
	VADDPS 672(R11), Y10, Y10
	VMULPS Y9, Y10, Y10         // poly·t
	VMULPS Y6, Y6, Y1
	VXORPS 448(R11), Y1, Y1     // -a·a
	EXPF(Y1, Y2, Y3, Y4, Y11)
	VMULPS Y2, Y10, Y10
	VMOVUPS 320(R11), Y12
	VSUBPS Y10, Y12, Y12        // e = 1 - poly·t·expf(-a·a)
	VXORPS Y13, Y13, Y13
	VCMPPS $1, Y13, Y5, Y13     // z < 0
	VANDPS 448(R11), Y13, Y13
	VXORPS Y13, Y12, Y12        // e = -e there
	VMULPS 32(R11), Y0, Y0      // 0.5·x
	VADDPS 320(R11), Y12, Y12   // 1 + e
	VMULPS Y12, Y0, Y0
	VMULPS (DX), Y0, Y0         // ·smooth
	VMOVUPS Y0, (DI)
	ADDQ $32, DI
	ADDQ $32, SI
	ADDQ $32, DX
	SUBQ $8, CX
	JNE  gloop
gdone:
	VZEROUPPER
	RET
