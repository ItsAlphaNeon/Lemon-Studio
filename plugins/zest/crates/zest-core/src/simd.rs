// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The vector operations the engine is written in. On x86-64 they are the processor's own. Elsewhere (Apple silicon)
//! `portable` gives the same operations lane by lane, with the same results, the compiler vectorizing them. The
//! approximate reciprocal is part of the sound, so it comes from a table of an Intel processor's (`data/rcp.in`). The
//! feature `portable` uses all this on x86-64 too, to compare renders.

#[cfg(all(target_arch = "x86_64", not(feature = "portable")))]
pub use core::arch::x86_64::*;

#[cfg(any(not(target_arch = "x86_64"), feature = "portable"))]
pub use portable::*;

/// Whether the AVX path can run.
#[inline]
pub fn has_avx() -> bool {
    #[cfg(all(target_arch = "x86_64", not(feature = "portable")))]
    {
        std::is_x86_feature_detected!("avx")
    }
    #[cfg(any(not(target_arch = "x86_64"), feature = "portable"))]
    {
        false
    }
}

#[cfg(any(not(target_arch = "x86_64"), feature = "portable", test))]
#[allow(non_camel_case_types, clippy::missing_safety_doc)]
pub mod portable {
    #[derive(Clone, Copy, Debug)]
    #[repr(C, align(16))]
    pub struct __m128(pub [f32; 4]);
    #[derive(Clone, Copy, Debug)]
    #[repr(C, align(16))]
    pub struct __m128i(pub [i32; 4]);
    #[derive(Clone, Copy, Debug)]
    #[repr(C, align(16))]
    pub struct __m128d(pub [f64; 2]);
    #[derive(Clone, Copy, Debug)]
    #[repr(C, align(32))]
    pub struct __m256(pub [f32; 8]);

    pub const _CMP_LE_OS: i32 = 0x02;
    pub const _CMP_GE_OS: i32 = 0x0d;

    /// x86's approximate reciprocal at exponent 0, by the top 11 bits of the mantissa: `result bits - 0x3f000000 >> 11`.
    const RCP: [u16; 2048] = include!("../data/rcp.in");

    /// x86's approximate reciprocal: the table scaled by the exponent; subnormal and zero inputs give infinity, and what
    /// would come out subnormal gives zero.
    #[inline(always)]
    pub fn rcp1(x: f32) -> f32 {
        let b = x.to_bits();
        let sign = b & 0x8000_0000;
        let e = (b >> 23 & 0xff) as i32;
        if e == 0xff {
            return if b & 0x7f_ffff != 0 { f32::from_bits(b | 0x40_0000) } else { f32::from_bits(sign) };
        }
        if e == 0 {
            return f32::from_bits(sign | 0x7f80_0000);
        }
        let t = 0x3f00_0000u32 + ((RCP[(b >> 12 & 0x7ff) as usize] as u32) << 11);
        let out_e = (t >> 23) as i32 + 127 - e;
        if out_e <= 0 {
            return f32::from_bits(sign);
        }
        f32::from_bits(sign | (out_e as u32) << 23 | (t & 0x7f_ffff))
    }

    /// All bits set for true, as a comparison gives.
    #[inline(always)]
    fn mask(b: bool) -> f32 {
        f32::from_bits(if b { u32::MAX } else { 0 })
    }
    #[inline(always)]
    fn bits(a: f32, b: f32, f: impl Fn(u32, u32) -> u32) -> f32 {
        f32::from_bits(f(a.to_bits(), b.to_bits()))
    }
    /// x86's truncating conversion: out of range and NaN give the lowest value.
    #[inline(always)]
    fn trunc32(x: f32) -> i32 {
        if x >= -2147483648.0 && x < 2147483648.0 {
            x as i32
        } else {
            i32::MIN
        }
    }
    #[inline(always)]
    fn trunc64(x: f32) -> i64 {
        if x >= -9223372036854775808.0 && x < 9223372036854775808.0 {
            x as i64
        } else {
            i64::MIN
        }
    }
    #[inline(always)]
    fn min(a: f32, b: f32) -> f32 {
        if a < b {
            a
        } else {
            b
        }
    }
    #[inline(always)]
    fn max(a: f32, b: f32) -> f32 {
        if a > b {
            a
        } else {
            b
        }
    }

    macro_rules! map {
        ($n:literal, $t:ident, $a:ident, $x:ident => $e:expr) => {{
            let mut o = [Default::default(); $n];
            for i in 0..$n {
                let $x = $a.0[i];
                o[i] = $e;
            }
            $t(o)
        }};
        ($n:literal, $t:ident, $a:ident, $b:ident, $x:ident, $y:ident => $e:expr) => {{
            let mut o = [Default::default(); $n];
            for i in 0..$n {
                let ($x, $y) = ($a.0[i], $b.0[i]);
                o[i] = $e;
            }
            $t(o)
        }};
    }

    macro_rules! binary {
        ($($name:ident $n:literal $t:ident $x:ident $y:ident => $e:expr;)*) => {$(
            #[inline(always)]
            pub fn $name(a: $t, b: $t) -> $t {
                map!($n, $t, a, b, $x, $y => $e)
            }
        )*};
    }

    binary! {
        _mm_add_ps 4 __m128 x y => x + y;
        _mm_sub_ps 4 __m128 x y => x - y;
        _mm_mul_ps 4 __m128 x y => x * y;
        _mm_div_ps 4 __m128 x y => x / y;
        _mm_min_ps 4 __m128 x y => min(x, y);
        _mm_max_ps 4 __m128 x y => max(x, y);
        _mm_and_ps 4 __m128 x y => bits(x, y, |p, q| p & q);
        _mm_or_ps 4 __m128 x y => bits(x, y, |p, q| p | q);
        _mm_xor_ps 4 __m128 x y => bits(x, y, |p, q| p ^ q);
        _mm_andnot_ps 4 __m128 x y => bits(x, y, |p, q| !p & q);
        _mm_cmpeq_ps 4 __m128 x y => mask(x == y);
        _mm_cmple_ps 4 __m128 x y => mask(x <= y);
        _mm_cmpge_ps 4 __m128 x y => mask(x >= y);
        _mm_add_epi32 4 __m128i x y => x.wrapping_add(y);
        _mm_sub_epi32 4 __m128i x y => x.wrapping_sub(y);
        _mm_and_si128 4 __m128i x y => x & y;
        _mm_add_pd 2 __m128d x y => x + y;
        _mm_mul_pd 2 __m128d x y => x * y;
        _mm256_add_ps 8 __m256 x y => x + y;
        _mm256_sub_ps 8 __m256 x y => x - y;
        _mm256_mul_ps 8 __m256 x y => x * y;
        _mm256_min_ps 8 __m256 x y => min(x, y);
        _mm256_max_ps 8 __m256 x y => max(x, y);
        _mm256_and_ps 8 __m256 x y => bits(x, y, |p, q| p & q);
        _mm256_or_ps 8 __m256 x y => bits(x, y, |p, q| p | q);
        _mm256_xor_ps 8 __m256 x y => bits(x, y, |p, q| p ^ q);
        _mm256_andnot_ps 8 __m256 x y => bits(x, y, |p, q| !p & q);
    }

    #[inline(always)]
    pub fn _mm_set1_ps(x: f32) -> __m128 {
        __m128([x; 4])
    }
    #[inline(always)]
    pub fn _mm_set_ss(x: f32) -> __m128 {
        __m128([x, 0.0, 0.0, 0.0])
    }
    #[inline(always)]
    pub fn _mm_setr_ps(a: f32, b: f32, c: f32, d: f32) -> __m128 {
        __m128([a, b, c, d])
    }
    #[inline(always)]
    pub fn _mm_setzero_ps() -> __m128 {
        __m128([0.0; 4])
    }
    #[inline(always)]
    pub fn _mm_set1_epi32(x: i32) -> __m128i {
        __m128i([x; 4])
    }
    #[inline(always)]
    pub fn _mm_setr_epi32(a: i32, b: i32, c: i32, d: i32) -> __m128i {
        __m128i([a, b, c, d])
    }
    #[inline(always)]
    pub fn _mm_set1_pd(x: f64) -> __m128d {
        __m128d([x; 2])
    }
    #[inline(always)]
    pub fn _mm256_set1_ps(x: f32) -> __m256 {
        __m256([x; 8])
    }
    #[inline(always)]
    pub fn _mm256_set_m128(hi: __m128, lo: __m128) -> __m256 {
        let (h, l) = (hi.0, lo.0);
        __m256([l[0], l[1], l[2], l[3], h[0], h[1], h[2], h[3]])
    }

    #[inline(always)]
    pub fn _mm_rcp_ps(a: __m128) -> __m128 {
        map!(4, __m128, a, x => rcp1(x))
    }
    #[inline(always)]
    pub fn _mm_rcp_ss(a: __m128) -> __m128 {
        let mut o = a;
        o.0[0] = rcp1(a.0[0]);
        o
    }
    #[inline(always)]
    pub fn _mm256_rcp_ps(a: __m256) -> __m256 {
        map!(8, __m256, a, x => rcp1(x))
    }
    #[inline(always)]
    pub fn _mm_sqrt_ps(a: __m128) -> __m128 {
        map!(4, __m128, a, x => x.sqrt())
    }
    #[inline(always)]
    pub fn _mm_cvtss_f32(a: __m128) -> f32 {
        a.0[0]
    }
    #[inline(always)]
    pub fn _mm_cvttss_si32(a: __m128) -> i32 {
        trunc32(a.0[0])
    }
    #[inline(always)]
    pub fn _mm_cvttss_si64(a: __m128) -> i64 {
        trunc64(a.0[0])
    }
    #[inline(always)]
    pub fn _mm_cvttps_epi32(a: __m128) -> __m128i {
        map!(4, __m128i, a, x => trunc32(x))
    }
    #[inline(always)]
    pub fn _mm_cvtepi32_ps(a: __m128i) -> __m128 {
        map!(4, __m128, a, x => x as f32)
    }
    #[inline(always)]
    pub fn _mm_cvtps_pd(a: __m128) -> __m128d {
        __m128d([a.0[0] as f64, a.0[1] as f64])
    }
    #[inline(always)]
    pub fn _mm_cvtpd_ps(a: __m128d) -> __m128 {
        __m128([a.0[0] as f32, a.0[1] as f32, 0.0, 0.0])
    }
    #[inline(always)]
    pub fn _mm_castps_si128(a: __m128) -> __m128i {
        map!(4, __m128i, a, x => x.to_bits() as i32)
    }
    #[inline(always)]
    pub fn _mm_castsi128_ps(a: __m128i) -> __m128 {
        map!(4, __m128, a, x => f32::from_bits(x as u32))
    }
    #[inline(always)]
    pub fn _mm_castps_pd(a: __m128) -> __m128d {
        let l = |i: usize, j: usize| a.0[i].to_bits() as u64 | (a.0[j].to_bits() as u64) << 32;
        __m128d([f64::from_bits(l(0, 1)), f64::from_bits(l(2, 3))])
    }
    #[inline(always)]
    pub fn _mm_castpd_ps(a: __m128d) -> __m128 {
        let (p, q) = (a.0[0].to_bits(), a.0[1].to_bits());
        __m128([f32::from_bits(p as u32), f32::from_bits((p >> 32) as u32), f32::from_bits(q as u32), f32::from_bits((q >> 32) as u32)])
    }
    #[inline(always)]
    pub fn _mm_slli_epi32(a: __m128i, n: i32) -> __m128i {
        map!(4, __m128i, a, x => if (0..32).contains(&n) { x << n } else { 0 })
    }
    #[inline(always)]
    pub fn _mm_srli_epi32(a: __m128i, n: i32) -> __m128i {
        map!(4, __m128i, a, x => if (0..32).contains(&n) { ((x as u32) >> n) as i32 } else { 0 })
    }
    #[inline(always)]
    pub fn _mm_srai_epi32(a: __m128i, n: i32) -> __m128i {
        map!(4, __m128i, a, x => x >> n.clamp(0, 31))
    }
    #[inline(always)]
    pub fn _mm_movelh_ps(a: __m128, b: __m128) -> __m128 {
        __m128([a.0[0], a.0[1], b.0[0], b.0[1]])
    }
    #[inline(always)]
    pub fn _mm_movehl_ps(a: __m128, b: __m128) -> __m128 {
        __m128([b.0[2], b.0[3], a.0[2], a.0[3]])
    }
    #[inline(always)]
    pub fn _mm_unpacklo_epi64(a: __m128i, b: __m128i) -> __m128i {
        __m128i([a.0[0], a.0[1], b.0[0], b.0[1]])
    }
    #[inline(always)]
    pub fn _mm_movemask_ps(a: __m128) -> i32 {
        let mut m = 0;
        for i in 0..4 {
            m |= ((a.0[i].to_bits() >> 31) as i32) << i;
        }
        m
    }
    #[inline(always)]
    pub fn _mm256_cmp_ps<const I: i32>(a: __m256, b: __m256) -> __m256 {
        map!(8, __m256, a, b, x, y => mask(if I == _CMP_GE_OS { x >= y } else { x <= y }))
    }

    #[inline(always)]
    pub unsafe fn _mm_loadu_ps(p: *const f32) -> __m128 {
        __m128(unsafe { (p as *const [f32; 4]).read_unaligned() })
    }
    #[inline(always)]
    pub unsafe fn _mm_storeu_ps(p: *mut f32, a: __m128) {
        unsafe { (p as *mut [f32; 4]).write_unaligned(a.0) }
    }
    #[inline(always)]
    pub unsafe fn _mm256_loadu_ps(p: *const f32) -> __m256 {
        __m256(unsafe { (p as *const [f32; 8]).read_unaligned() })
    }
    #[inline(always)]
    pub unsafe fn _mm256_storeu_ps(p: *mut f32, a: __m256) {
        unsafe { (p as *mut [f32; 8]).write_unaligned(a.0) }
    }
    #[inline(always)]
    pub unsafe fn _mm_loadu_si128(p: *const __m128i) -> __m128i {
        unsafe { p.read_unaligned() }
    }
    #[inline(always)]
    pub unsafe fn _mm_storeu_si128(p: *mut __m128i, a: __m128i) {
        unsafe { p.write_unaligned(a) }
    }
    #[inline(always)]
    pub unsafe fn _mm_load_sd(p: *const f64) -> __m128d {
        __m128d([unsafe { p.read_unaligned() }, 0.0])
    }
    #[inline(always)]
    pub unsafe fn _mm_store_sd(p: *mut f64, a: __m128d) {
        unsafe { p.write_unaligned(a.0[0]) }
    }
}

/// Every portable operation against the processor's own, on values that reach the edge cases.
#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use super::portable as p;
    use core::arch::x86_64 as x;

    const F: [f32; 20] = [
        0.0, -0.0, 1.0, -1.0, 0.5, -0.5, 1.5, -2.5, 3.2825, 1e-30, -1e-30, 1e30, -1e30, 2147483500.0, -2147483648.0, 3e9, -3e9, 1e19, f32::INFINITY, f32::NAN,
    ];
    const I: [i32; 8] = [0, 1, -1, 31, i32::MIN, i32::MAX, 0x4b40_0000, 0x7f12_3456];

    fn fours() -> Vec<[f32; 4]> {
        (0..F.len()).map(|i| std::array::from_fn(|j| F[(i + j * 7) % F.len()])).collect()
    }
    fn ints() -> Vec<[i32; 4]> {
        (0..I.len()).map(|i| std::array::from_fn(|j| I[(i + j * 3) % I.len()])).collect()
    }
    fn real(a: [f32; 4]) -> x::__m128 {
        unsafe { x::_mm_loadu_ps(a.as_ptr()) }
    }
    fn reali(a: [i32; 4]) -> x::__m128i {
        unsafe { x::_mm_loadu_si128(a.as_ptr() as *const x::__m128i) }
    }
    fn out(a: x::__m128) -> [u32; 4] {
        unsafe { core::mem::transmute(a) }
    }
    fn outi(a: x::__m128i) -> [i32; 4] {
        unsafe { core::mem::transmute(a) }
    }
    fn same(got: [f32; 4], want: [u32; 4], what: &str, a: &[f32; 4], b: &[f32; 4]) {
        for i in 0..4 {
            // Any NaN will do: x86 and the arithmetic of other processors pick different ones.
            let both_nan = got[i].is_nan() && f32::from_bits(want[i]).is_nan();
            assert!(both_nan || got[i].to_bits() == want[i], "{what} lane {i}: {:?} vs {:?} on {a:?} {b:?}", got[i], f32::from_bits(want[i]));
        }
    }

    #[test]
    fn four_lanes_as_the_processor() {
        for a in fours() {
            for b in fours() {
                macro_rules! two {
                    ($($n:ident)*) => {$(
                        same(p::$n(p::__m128(a), p::__m128(b)).0, out(unsafe { x::$n(real(a), real(b)) }), stringify!($n), &a, &b);
                    )*};
                }
                two!(_mm_add_ps _mm_sub_ps _mm_mul_ps _mm_div_ps _mm_min_ps _mm_max_ps _mm_movelh_ps _mm_movehl_ps);
                // Bit results, no arithmetic: every bit counts, NaN too.
                macro_rules! exact {
                    ($($n:ident)*) => {$(
                        let got: [u32; 4] = p::$n(p::__m128(a), p::__m128(b)).0.map(f32::to_bits);
                        assert_eq!(got, out(unsafe { x::$n(real(a), real(b)) }), "{} on {a:?} {b:?}", stringify!($n));
                    )*};
                }
                exact!(_mm_and_ps _mm_or_ps _mm_xor_ps _mm_andnot_ps _mm_cmpeq_ps _mm_cmple_ps _mm_cmpge_ps);
            }
            same(p::_mm_sqrt_ps(p::__m128(a)).0, out(unsafe { x::_mm_sqrt_ps(real(a)) }), "sqrt", &a, &a);
            assert_eq!(p::_mm_cvttps_epi32(p::__m128(a)).0, outi(unsafe { x::_mm_cvttps_epi32(real(a)) }), "cvttps {a:?}");
            assert_eq!(p::_mm_cvttss_si32(p::__m128(a)), unsafe { x::_mm_cvttss_si32(real(a)) }, "cvttss32 {a:?}");
            assert_eq!(p::_mm_cvttss_si64(p::__m128(a)), unsafe { x::_mm_cvttss_si64(real(a)) }, "cvttss64 {a:?}");
            assert_eq!(p::_mm_movemask_ps(p::__m128(a)), unsafe { x::_mm_movemask_ps(real(a)) }, "movemask {a:?}");
            // Double lanes and back.
            let d = p::_mm_cvtps_pd(p::__m128(a));
            let dx = unsafe { x::_mm_cvtps_pd(real(a)) };
            same(p::_mm_cvtpd_ps(d).0, out(unsafe { x::_mm_cvtpd_ps(dx) }), "cvtpd", &a, &a);
            let c = p::_mm_castps_pd(p::__m128(a));
            assert_eq!(p::_mm_castpd_ps(c).0.map(f32::to_bits), a.map(f32::to_bits), "cast through double");
            let dx: [f64; 2] = unsafe { core::mem::transmute(x::_mm_castps_pd(real(a))) };
            assert_eq!(c.0.map(f64::to_bits), dx.map(f64::to_bits), "castps_pd");
        }
    }

    #[test]
    fn integer_lanes_as_the_processor() {
        for a in ints() {
            for b in ints() {
                let (pa, pb) = (p::__m128i(a), p::__m128i(b));
                assert_eq!(p::_mm_add_epi32(pa, pb).0, outi(unsafe { x::_mm_add_epi32(reali(a), reali(b)) }));
                assert_eq!(p::_mm_sub_epi32(pa, pb).0, outi(unsafe { x::_mm_sub_epi32(reali(a), reali(b)) }));
                assert_eq!(p::_mm_and_si128(pa, pb).0, outi(unsafe { x::_mm_and_si128(reali(a), reali(b)) }));
                assert_eq!(p::_mm_unpacklo_epi64(pa, pb).0, outi(unsafe { x::_mm_unpacklo_epi64(reali(a), reali(b)) }));
            }
            let pa = p::__m128i(a);
            assert_eq!(p::_mm_slli_epi32(pa, 23).0, outi(unsafe { x::_mm_slli_epi32::<23>(reali(a)) }));
            assert_eq!(p::_mm_srli_epi32(pa, 24).0, outi(unsafe { x::_mm_srli_epi32::<24>(reali(a)) }));
            assert_eq!(p::_mm_srai_epi32(pa, 31).0, outi(unsafe { x::_mm_srai_epi32::<31>(reali(a)) }));
            same(p::_mm_cvtepi32_ps(pa).0, out(unsafe { x::_mm_cvtepi32_ps(reali(a)) }), "cvtepi32", &[0.0; 4], &[0.0; 4]);
        }
    }

    /// The reciprocal's table against the processor's own: within the error x86 states, as other makes of processor
    /// answer differently.
    #[test]
    fn reciprocal_within_the_processors_error() {
        for v in [1e-3f32, 0.1, 0.7, 1.0, 1.5, 3.0, 27.0, 124.0, 9000.0, -2.0] {
            let got = p::_mm_rcp_ps(p::_mm_set1_ps(v)).0[0];
            let want = f32::from_bits(out(unsafe { x::_mm_rcp_ps(x::_mm_set1_ps(v)) })[0]);
            assert!(((got - want) / want).abs() < 3.0 / 4096.0, "{v}: {got} {want}");
            assert_eq!(p::_mm_rcp_ss(p::_mm_setr_ps(v, 2.0, 3.0, 4.0)).0[1..], [2.0, 3.0, 4.0]);
        }
    }

    /// On the Intel processors the table comes from, every value is the processor's own, edge cases too.
    #[test]
    #[ignore = "only on an Intel processor"]
    fn reciprocal_is_intels_exactly() {
        let bits = (0..=0xffu32).flat_map(|e| (0..(1u32 << 23)).step_by(509).map(move |m| e << 23 | m));
        for b in bits.flat_map(|b| [b, b | 0x8000_0000]) {
            let v = f32::from_bits(b);
            let got = p::rcp1(v);
            let want = f32::from_bits(out(unsafe { x::_mm_rcp_ps(x::_mm_set1_ps(v)) })[0]);
            assert!(got.to_bits() == want.to_bits() || (got.is_nan() && want.is_nan()), "{v:e} ({b:#x}): {got:e} {want:e}");
        }
    }

    #[test]
    fn eight_lanes_keep_their_order() {
        let (a, b) = (p::_mm_setr_ps(1.0, 2.0, 3.0, 4.0), p::_mm_setr_ps(5.0, 6.0, 7.0, 8.0));
        let got = p::_mm256_set_m128(b, a).0;
        let want: [f32; 8] = unsafe { core::mem::transmute(x::_mm256_set_m128(x::_mm_setr_ps(5.0, 6.0, 7.0, 8.0), x::_mm_setr_ps(1.0, 2.0, 3.0, 4.0))) };
        assert_eq!(got, want);
        assert_eq!(p::_CMP_GE_OS, x::_CMP_GE_OS);
        assert_eq!(p::_CMP_LE_OS, x::_CMP_LE_OS);
        let c = p::_mm256_cmp_ps::<{ p::_CMP_GE_OS }>(p::_mm256_set1_ps(1.0), p::_mm256_set_m128(p::_mm_set1_ps(2.0), p::_mm_set1_ps(0.5)));
        assert_eq!(c.0[0].to_bits(), u32::MAX);
        assert_eq!(c.0[7].to_bits(), 0);
    }
}
