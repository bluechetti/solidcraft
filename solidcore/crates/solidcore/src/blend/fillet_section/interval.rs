//! Rounded interval arithmetic for the G^n section's certificate: every result widened
//! outward past its rounding error, so it holds the exact result of the operation on any
//! numbers in the operands. An empty or undefined result (a division by an interval holding
//! 0, a NaN) is the whole line, which no test passes.
//!
//! Fork: `sm_Ivl` and its helpers in `source/SMLib/src/SmFilletSection.cpp`
//! (tangency-labs/smlib@11ea2d9); see [`super`] for the table.
//!
//! Deviations: the widening and the products whose result is subnormal are computed in
//! integers where the floating operation would be subnormal, bit for bit the same values
//! (an Intel core's microcode assist on every subnormal result made them 17 to 25 times
//! slower; [`round_up`], `product`). The product and the outward rounding take a short
//! path where no end can meet those cases (`products_are_floating`, `round_out`), the same
//! values bit for bit: one test instead of one per product, nothing spilled for the rare
//! integer path, and inline, where the two calls were most of a G^n certificate's time;
//! the widening of a tiny end is one scaled product rounded to an integer
//! (`widening_tiny_float`), the integers only about a tie or for a subnormal end.

use core::ops::{Add, Div, Mul, Sub};

use crate::geom::certified::next_up;

/// `std::min(a, b)`: `(b < a) ? b : a` (a NaN `a` is kept).
#[inline]
pub(crate) fn cmin(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

/// `std::max(a, b)`: `(a < b) ? b : a` (a NaN `a` is kept).
#[inline]
pub(crate) fn cmax(a: f64, b: f64) -> f64 {
    if a < b {
        b
    } else {
        a
    }
}

/// `sm_Dn`: moved down by twice the rounding error of an operation that gave `x`
/// (`|x| 2^-53`, or `2^-1075` below the normal range).
#[inline]
#[must_use]
pub fn round_down(x: f64) -> f64 {
    x - widening(x)
}

/// `sm_Up`: moved up by twice the rounding error of an operation that gave `x`.
#[inline]
#[must_use]
pub fn round_up(x: f64) -> f64 {
    x + widening(x)
}

/// The relative part of [`widening`].
const WIDEN_REL: f64 = 2.3e-16;
/// The absolute part of [`widening`].
const WIDEN_ABS: f64 = 1.0e-300;
/// Below this, `|x| WIDEN_REL` is under the normal range (`9e-293 * 2.3e-16 = 2.07e-308`,
/// less than `f64::MIN_POSITIVE`, 2.2250738585072014e-308).
const WIDEN_TINY: f64 = 9.0e-293;

/// `|x| * 2.3e-16 + 1.0e-300`, bit for bit, the amount [`round_down`] and [`round_up`]
/// move `x` by (not the fork's, which writes the expression; the value is the same).
///
/// For `0 < |x| <` [`WIDEN_TINY`] (every end of an interval that holds 0 is about 1e-300
/// from it) the product `|x| * 2.3e-16` is subnormal, and an Intel core takes a microcode
/// assist for every subnormal result: 28 ns against 1.6 ns for a normal one, which was
/// most of a G^n certificate's time. There it is computed in integers as the floating
/// operations round it: the product rounded once to the subnormal grid (`2^-1074`, ties
/// to even), then the sum with 1e-300 rounded to 1e-300's grid (ties to an even
/// significand). Every other `x` (0, a normal product, infinities, NaN) takes the
/// expression itself.
#[inline]
fn widening(x: f64) -> f64 {
    let a = x.abs();
    if is_tiny(a) {
        if let Some(w) = widening_tiny_float(a).or_else(|| widening_tiny(a)) {
            return w;
        }
    }
    a * WIDEN_REL + WIDEN_ABS
}

/// [`WIDEN_REL`] `2^1049`, exact: `2^-1049` is the grid of [`WIDEN_ABS`]'s binade
/// (`2^-997 <= 1e-300 < 2^-996`, 52 bits below).
const WIDEN_REL_GRID: f64 = f64::from_bits(WIDEN_REL.to_bits() + (1049 << 52));
/// `2^52`: a non-negative double under `2^51` plus it, less it, is rounded to an integer.
const TWO_52: f64 = 4_503_599_627_370_496.0;
/// `2^-24`: how far from a tie [`widening_tiny_float`] decides.
const TIE_MARGIN: f64 = 5.960_464_477_539_063e-8;

/// [`widening`] for a positive finite `a` under [`WIDEN_TINY`] with floating operations that
/// meet no subnormal, bit for bit [`widening_tiny`]'s; `None` for a subnormal `a` (whose
/// product would take the assist) and within `2^-24` of a tie (about one `a` in eight
/// million), which it leaves to the integers.
///
/// The value is `1e-300 + k 2^-1049`, `k` the integer nearest `n / 2^25`, where `n 2^-1074`
/// is `a WIDEN_REL` rounded to the subnormal grid (ties to an even significand). `y =
/// a WIDEN_REL 2^1049` is that quotient's exact value `X < 2^27` rounded once (within
/// `2^-26`), and `n / 2^25` is within `2^-26` of `X`, so within `2^-25` of `y`: where `y`
/// is farther than `2^-24` from every half-integer, `n / 2^25` is not a tie and has `y`'s
/// nearest integer. `k < 2^27` keeps the sum in 1e-300's binade, so it is `k` added to
/// its bits.
#[inline]
fn widening_tiny_float(a: f64) -> Option<f64> {
    if a < f64::MIN_POSITIVE {
        return None;
    }
    let y = a * WIDEN_REL_GRID;
    let k = (y + TWO_52) - TWO_52;
    if 0.5 - (y - k).abs() > TIE_MARGIN {
        Some(f64::from_bits(WIDEN_ABS.to_bits() + k as u64))
    } else {
        None
    }
}

/// The integer significand and the exponent of a positive finite `x`:
/// `x = m 2^(e - 1075)` (a subnormal `x` has no hidden bit and `e = 1`).
fn significand_exponent(x: f64) -> (u64, u64) {
    let bits = x.to_bits();
    let e = (bits >> 52) & 0x7ff;
    let f = bits & ((1 << 52) - 1);
    if e == 0 {
        (f, 1)
    } else {
        (f | (1 << 52), e)
    }
}

/// `a * b`, bit for bit (not the fork's, which writes the product). Where the product is
/// subnormal, an Intel core takes a microcode assist (41 ns against 2 ns; a G^n
/// certificate meets one wherever an end about 1e-300 from 0 meets a number under about
/// 2e-8): there it is the product of the significands in integers, rounded once to the
/// subnormal grid `2^-1074` (ties to even). Every other product is the floating one.
#[inline]
fn product(a: f64, b: f64) -> f64 {
    let (ba, bb) = (a.to_bits(), b.to_bits());
    let (ea, eb) = ((ba >> 52) & 0x7ff, (bb >> 52) & 0x7ff);
    // |a b| < 2^(ea + eb - 2044): under 2^-1075 (rounds to 0, no assist) for ea + eb <= 969;
    // at least 2^-1022 (normal) for ea + eb >= 1024; subnormal inputs (e = 0) and
    // infinities or NaNs (e = 0x7ff) take the floating product
    if ea == 0 || eb == 0 || ea == 0x7ff || eb == 0x7ff || ea + eb <= 969 || ea + eb >= 1024 {
        return a * b;
    }
    product_subnormal(a, b).unwrap_or(a * b)
}

/// [`product`] in integers, for normal `a`, `b` with `969 < ea + eb < 1024`; `None` where
/// the product is not under the normal range after all.
fn product_subnormal(a: f64, b: f64) -> Option<f64> {
    let (ma, ea) = significand_exponent(a.abs());
    let (mb, eb) = significand_exponent(b.abs());
    // |a b| = ma mb 2^(ea + eb - 2150): in units of 2^-1074, ma mb 2^(ea + eb - 1076)
    let sh = 1076_u64.checked_sub(ea + eb)?;
    if !(1..128).contains(&sh) {
        return None;
    }
    let p = u128::from(ma) * u128::from(mb);
    if (p >> sh) >= 1 << 52 {
        return None; // normal
    }
    let n = u64::try_from(shift_rounded(p, sh, |q| q & 1 == 1)).ok()?;
    let sign = (a.to_bits() ^ b.to_bits()) & (1 << 63);
    Some(f64::from_bits(sign | n))
}

/// `q >> sh` rounded to nearest, a tie to the side `odd` does not call odd.
fn shift_rounded(q: u128, sh: u64, odd: impl Fn(u128) -> bool) -> u128 {
    let lo = q >> sh;
    let rest = q & ((1 << sh) - 1);
    let half = 1 << (sh - 1);
    if rest > half || (rest == half && odd(lo)) {
        lo + 1
    } else {
        lo
    }
}

/// [`widening`] for a positive finite `a` under [`WIDEN_TINY`], in integers; `None` where the
/// exponents leave the cases worked out (never for these constants).
fn widening_tiny(a: f64) -> Option<f64> {
    let (ma, ea) = significand_exponent(a);
    let (mc, ec) = significand_exponent(WIDEN_REL);
    let (mf, ef) = significand_exponent(WIDEN_ABS);
    // a * WIDEN_REL = ma mc 2^(ea + ec - 2150), in units of the subnormal grid 2^-1074:
    // ma mc 2^(ea + ec - 1076), rounded once (ties to even)
    let sh = 1076_u64.checked_sub(ea + ec)?;
    if !(1..128).contains(&sh) {
        return None;
    }
    let n = shift_rounded(u128::from(ma) * u128::from(mc), sh, |q| q & 1 == 1);
    // t = n 2^-1074; WIDEN_ABS = mf 2^(ef - 1075), its grid 2^(ef - 1075): t is n 2^(1 - ef)
    // of those, and the sum mf + that is rounded to an integer (ties to an even
    // significand), staying in WIDEN_ABS's binade
    let sh2 = ef.checked_sub(1)?;
    if !(1..64).contains(&sh2) {
        return None;
    }
    let mf = u128::from(mf);
    if (n >> sh2) + mf >= 1 << 53 {
        return None;
    }
    let k = shift_rounded(n, sh2, |q| (mf + q) & 1 == 1);
    let k = u64::try_from(k).ok()?;
    Some(f64::from_bits(WIDEN_ABS.to_bits() + k))
}

/// Whether `0 < a <` [`WIDEN_TINY`] for an `a >= 0` (or NaN), where [`widening`] takes its
/// integer path: one unsigned compare of the bits (0 wraps past the bound, and a NaN's
/// bits are above it).
#[inline]
fn is_tiny(a: f64) -> bool {
    a.to_bits().wrapping_sub(1) < WIDEN_TINY.to_bits() - 1
}

/// `[round_down(lo), round_up(hi)]` for ends that are not NaN, bit for bit: where neither
/// end is tiny, [`widening`]'s expression inline (no call, nothing spilled for the integer
/// path); otherwise [`round_out_tiny`].
#[inline]
fn round_out(lo: f64, hi: f64) -> Ivl {
    let (a, b) = (lo.abs(), hi.abs());
    if is_tiny(a) | is_tiny(b) {
        return round_out_tiny(lo, hi);
    }
    Ivl::new(
        lo - (a * WIDEN_REL + WIDEN_ABS),
        hi + (b * WIDEN_REL + WIDEN_ABS),
    )
}

/// [`round_out`] where an end is tiny: [`round_down`] and [`round_up`] themselves.
#[inline(never)]
fn round_out_tiny(lo: f64, hi: f64) -> Ivl {
    Ivl::new(round_down(lo), round_up(hi))
}

/// `2^512`.
const SCALE_UP: f64 = f64::from_bits((1023 + 512) << 52);
/// `2^-508`: `2^-1020` times [`SCALE_UP`].
const SAFE_SCALED: f64 = f64::from_bits((1023 - 508) << 52);

/// Whether every [`product`] of an end of one interval and an end of another is the
/// floating product, from the least magnitudes of their ends `ma`, `mb`: true where
/// `ma mb >= 2^-1020`, computed as `(ma 2^512) mb` so that no subnormal is formed (the
/// scaling is exact; an overflow is infinity, which passes). Then every product of ends is
/// at least `ma mb > 2^-1021` exactly (the rounding moves it by at most 2^-53 relative), so
/// two normal ends have `ea + eb >= 1024`, which [`product`] takes the floating way, as it
/// does every other end. False, and the exact path (the same values), for a NaN, a 0 or
/// a product under the bound.
#[inline]
fn products_are_floating(ma: f64, mb: f64) -> bool {
    (ma * SCALE_UP) * mb >= SAFE_SCALED
}

/// `2^-565`: `2^-1077` times [`SCALE_UP`].
const UNDER_SCALED: f64 = f64::from_bits((1023 - 565) << 52);

/// Whether every [`product`] of ends is the floating product because every one rounds to
/// 0 (under half the subnormal step, `2^-1075`), from the greatest magnitudes of the ends `ma`, `mb`:
/// true where `ma mb < 2^-1077`, computed as `(ma 2^512) mb`. Then every product of ends is
/// under `2^-1076` exactly, so two normal ends have `ea + eb <= 969` (`|a b| >=
/// 2^(ea + eb - 2046)`), which [`product`] takes the floating way (a product rounding to
/// 0 takes no assist). False for a NaN.
#[inline]
fn products_underflow(ma: f64, mb: f64) -> bool {
    (ma * SCALE_UP) * mb < UNDER_SCALED
}

/// `sm_Ivl`: the closed interval `[lo, hi]`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ivl {
    /// The lower end.
    pub lo: f64,
    /// The upper end.
    pub hi: f64,
}

impl From<f64> for Ivl {
    /// `sm_Ivl(double d)`: the point interval `[d, d]`.
    fn from(d: f64) -> Self {
        Self { lo: d, hi: d }
    }
}

impl Ivl {
    /// `sm_Ivl(double a, double b)`.
    #[must_use]
    pub const fn new(lo: f64, hi: f64) -> Self {
        Self { lo, hi }
    }

    /// `sm_Whole`: the whole line.
    #[must_use]
    pub const fn whole() -> Self {
        Self::new(f64::NEG_INFINITY, f64::INFINITY)
    }

    /// `sm_Checked`: `[a, b]` rounded outward, or the whole line if either is NaN.
    #[must_use]
    #[inline]
    pub fn checked(a: f64, b: f64) -> Self {
        if a.is_nan() | b.is_nan() {
            Self::whole()
        } else {
            round_out(a, b)
        }
    }

    /// `sm_Positive(const sm_Ivl &)`: every number in it is positive.
    #[must_use]
    pub fn is_positive(self) -> bool {
        self.lo > 0.0
    }

    /// `sm_Sqrt(const sm_Ivl &)`: the square roots of its non-negative part (the whole line
    /// if it has none).
    #[must_use]
    pub fn sqrt(self) -> Self {
        // !(x.hi >= 0.0): a NaN end fails it too
        let has_non_negative = self.hi >= 0.0;
        if !has_non_negative {
            return Self::whole();
        }
        Self::checked(
            crate::geom::vector3d::sqrt(cmax(self.lo, 0.0)),
            crate::geom::vector3d::sqrt(self.hi),
        )
    }

    /// `sm_Mag`: the largest magnitude in it.
    #[must_use]
    pub fn mag(self) -> f64 {
        cmax(self.lo.abs(), self.hi.abs())
    }

    /// `sm_Mig`: the least magnitude in it, 0 where it holds 0.
    #[must_use]
    pub fn mig(self) -> f64 {
        if self.lo > 0.0 {
            self.lo
        } else if self.hi < 0.0 {
            -self.hi
        } else {
            0.0
        }
    }

    /// `sm_Mid`: its middle.
    #[must_use]
    pub fn mid(self) -> f64 {
        0.5 * (self.lo + self.hi)
    }

    /// `sm_Hull`: the smallest interval holding both.
    #[must_use]
    pub fn hull(self, b: Self) -> Self {
        Self::new(cmin(self.lo, b.lo), cmax(self.hi, b.hi))
    }

    /// Both ends are finite and in order.
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.lo.is_finite() && self.hi.is_finite() && self.lo <= self.hi
    }

    /// `[lo, hi]` moved outward by one ulp at each end, or the whole line unless both
    /// ends are finite and in order.
    ///
    /// Soundness: one ulp outward covers round-to-nearest for + - * /. A correctly
    /// rounded operation returns the double nearest its exact result, so the exact
    /// result lies between that double's neighbours (an overflow past the largest
    /// double moves the end to infinity, and the result is the whole line).
    ///
    /// This is the tighter sibling of [`Ivl::checked`], whose relative `2.3e-16 |x|`
    /// step can reach two ulps and widens exact zeros by `1e-300`. Proofs that must
    /// close at an exactly attained bound use the `*_ulp` operations built on it.
    #[must_use]
    pub fn outward_ulp(lo: f64, hi: f64) -> Self {
        if lo.is_finite() && hi.is_finite() && lo <= hi {
            Self::new(-next_up(-lo), next_up(hi))
        } else {
            Self::whole()
        }
    }

    /// `self + b` rounded one ulp outward ([`Ivl::outward_ulp`]). Adding an exact 0 is
    /// exact in IEEE arithmetic and is returned unwidened. Not finite: the whole line.
    #[must_use]
    pub fn add_ulp(self, b: Self) -> Self {
        if !self.is_finite() || !b.is_finite() {
            Self::whole()
        } else if self == Self::from(0.0) {
            b
        } else if b == Self::from(0.0) {
            self
        } else {
            Self::outward_ulp(self.lo + b.lo, self.hi + b.hi)
        }
    }

    /// `self - b` rounded one ulp outward. Subtracting an exact 0 is exact, and a point
    /// interval `[x, x]` holds only the double `x`, so `[x, x] - [x, x]` is exactly 0
    /// (IEEE `x - x == 0` for finite `x`). Not finite: the whole line.
    #[must_use]
    pub fn sub_ulp(self, b: Self) -> Self {
        if !self.is_finite() || !b.is_finite() {
            Self::whole()
        } else if self.lo == self.hi && self == b {
            Self::from(0.0)
        } else if b == Self::from(0.0) {
            self
        } else {
            Self::outward_ulp(self.lo - b.hi, self.hi - b.lo)
        }
    }

    /// `self * b` rounded one ulp outward. A product with an exact 0 or 1 is exact and
    /// is returned unwidened. Not finite: the whole line.
    #[must_use]
    pub fn mul_ulp(self, b: Self) -> Self {
        if !self.is_finite() || !b.is_finite() {
            return Self::whole();
        }
        if self == Self::from(0.0) || b == Self::from(0.0) {
            return Self::from(0.0);
        }
        if self == Self::from(1.0) {
            return b;
        }
        if b == Self::from(1.0) {
            return self;
        }
        // Finite operands: a product overflows to infinity but is never NaN.
        let products = [
            self.lo * b.lo,
            self.lo * b.hi,
            self.hi * b.lo,
            self.hi * b.hi,
        ];
        Self::outward_ulp(
            products.into_iter().fold(f64::INFINITY, f64::min),
            products.into_iter().fold(f64::NEG_INFINITY, f64::max),
        )
    }

    /// `self / b` rounded one ulp outward, for a divisor that excludes 0 (otherwise the
    /// whole line). Dividing by an exact 1, dividing an exact 0, and `[x, x] / [x, x]`
    /// are exact and are returned unwidened. Not finite: the whole line.
    #[must_use]
    pub fn div_ulp(self, b: Self) -> Self {
        let excludes_zero = b.lo > 0.0 || b.hi < 0.0;
        if !self.is_finite() || !b.is_finite() || !excludes_zero {
            return Self::whole();
        }
        if self == Self::from(0.0) || b == Self::from(1.0) {
            return self;
        }
        if self.lo == self.hi && self == b {
            return Self::from(1.0);
        }
        let quotients = [
            self.lo / b.lo,
            self.lo / b.hi,
            self.hi / b.lo,
            self.hi / b.hi,
        ];
        Self::outward_ulp(
            quotients.into_iter().fold(f64::INFINITY, f64::min),
            quotients.into_iter().fold(f64::NEG_INFINITY, f64::max),
        )
    }
}

impl Add for Ivl {
    type Output = Self;
    #[inline]
    fn add(self, b: Self) -> Self {
        Self::checked(self.lo + b.lo, self.hi + b.hi)
    }
}

impl Sub for Ivl {
    type Output = Self;
    #[inline]
    fn sub(self, b: Self) -> Self {
        Self::checked(self.lo - b.hi, self.hi - b.lo)
    }
}

/// `sm_Ivl`'s product, [`product`] at every end: [`Ivl`]'s `*` where an end may meet its
/// integer path.
#[inline(never)]
fn mul_exact_products(a: Ivl, b: Ivl) -> Ivl {
    let p0 = product(a.lo, b.lo);
    let rest = [
        product(a.lo, b.hi),
        product(a.hi, b.lo),
        product(a.hi, b.hi),
    ];
    let mut lo = p0;
    let mut hi = p0;
    for pi in rest {
        if pi.is_nan() {
            return Ivl::whole();
        }
        lo = cmin(lo, pi);
        hi = cmax(hi, pi);
    }
    Ivl::checked(lo, hi)
}

impl Mul for Ivl {
    type Output = Self;
    /// [`mul_exact_products`] bit for bit: where [`products_are_floating`] or
    /// [`products_underflow`], the four floating products, a NaN among them the whole line
    /// (`std::min` keeps a NaN first one, which `sm_Checked` catches; a later one returns
    /// at once), their least and greatest in the same order, rounded outward.
    #[inline]
    fn mul(self, b: Self) -> Self {
        let (a0, a1, b0, b1) = (self.lo.abs(), self.hi.abs(), b.lo.abs(), b.hi.abs());
        if !products_are_floating(cmin(a0, a1), cmin(b0, b1))
            && !products_underflow(cmax(a0, a1), cmax(b0, b1))
        {
            return mul_exact_products(self, b);
        }
        let (p0, p1, p2, p3) = (
            self.lo * b.lo,
            self.lo * b.hi,
            self.hi * b.lo,
            self.hi * b.hi,
        );
        if p0.is_nan() | p1.is_nan() | p2.is_nan() | p3.is_nan() {
            return Self::whole();
        }
        let lo = cmin(cmin(cmin(p0, p1), p2), p3);
        let hi = cmax(cmax(cmax(p0, p1), p2), p3);
        round_out(lo, hi)
    }
}

impl Div for Ivl {
    type Output = Self;
    #[inline]
    fn div(self, b: Self) -> Self {
        let excludes_zero = b.lo > 0.0 || b.hi < 0.0;
        if !excludes_zero {
            return Self::whole();
        }
        self * Self::checked(1.0 / b.hi, 1.0 / b.lo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The integer product is the floating one bit for bit, over every pair of binades
    /// whose product can be subnormal (random significands and signs, a fixed seed), on
    /// both sides of that window, and on the special values.
    #[test]
    fn the_product_is_the_floating_one_bit_for_bit() {
        let same = |a: f64, b: f64| {
            let (p, e) = (product(a, b), a * b);
            assert!(
                p.to_bits() == e.to_bits() || (p.is_nan() && e.is_nan()),
                "{a:e} * {b:e}: {p:e} against {e:e}"
            );
        };
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for ea in (1_u64..1030).step_by(7) {
            for d in 960_u64..1030 {
                let Some(eb) = d.checked_sub(ea).filter(|&e| (1..0x7ff).contains(&e)) else {
                    continue;
                };
                for _ in 0..40 {
                    let a = f64::from_bits((next() & (1 << 63)) | (ea << 52) | (next() >> 12));
                    let b = f64::from_bits((next() & (1 << 63)) | (eb << 52) | (next() >> 12));
                    same(a, b);
                }
                // exact powers of two and all-ones significands: the ties and the edges
                same(f64::from_bits(ea << 52), f64::from_bits(eb << 52));
                same(
                    f64::from_bits((ea << 52) | ((1 << 52) - 1)),
                    f64::from_bits((eb << 52) | ((1 << 52) - 1)),
                );
                same(
                    f64::from_bits((ea << 52) | (1 << 51)),
                    f64::from_bits((eb << 52) | 1),
                );
            }
        }
        for (a, b) in [
            (0.0, 1.0e-300),
            (-0.0, 3.0),
            (f64::MIN_POSITIVE, 0.5),
            (f64::MIN_POSITIVE / 8.0, 3.0),
            (1.0e-300, 1.0e-10),
            (-1.0e-300, 1.0e-300),
            (f64::INFINITY, 1.0e-300),
            (f64::NAN, 1.0),
            (1.0e-160, 1.0e-160),
        ] {
            same(a, b);
            same(b, a);
        }
    }

    /// The integer widening is the floating expression bit for bit: over every binade
    /// from the least normal number to past the threshold (random significands, a fixed
    /// seed), at the ends, and on the special values.
    #[test]
    fn the_widening_is_the_expression_bit_for_bit() {
        let expr = |x: f64| x.abs() * WIDEN_REL + WIDEN_ABS;
        let same = |x: f64| {
            let (w, e) = (widening(x), expr(x));
            assert!(
                w.to_bits() == e.to_bits() || (w.is_nan() && e.is_nan()),
                "{x:e}: {w:e} against {e:e}"
            );
        };
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        // biased exponents 1 (the least normal binade) to 60 (about 1e-290)
        for e in 1_u64..=60 {
            for _ in 0..20_000 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let x = f64::from_bits((e << 52) | (seed & ((1 << 52) - 1)));
                same(x);
                same(-x);
            }
            same(f64::from_bits(e << 52));
            same(f64::from_bits((e << 52) | ((1 << 52) - 1)));
        }
        for x in [
            0.0,
            -0.0,
            f64::MIN_POSITIVE,
            f64::MIN_POSITIVE / 4.0,
            f64::from_bits(1),
            f64::from_bits((1 << 52) - 1),
            1.0e-310,
            -3.0e-320,
            WIDEN_TINY,
            f64::from_bits(WIDEN_TINY.to_bits() - 1),
            1.0e-300,
            -1.0e-300,
            1.0,
            f64::INFINITY,
            f64::NAN,
        ] {
            same(x);
        }
    }

    /// The short paths are the long ones bit for bit: `*` against [`mul_exact_products`]
    /// and [`Ivl::checked`] against [`round_down`] and [`round_up`], over intervals of every
    /// kind (narrow about numbers of every size, about 0 at the widening's 1e-300, zeros,
    /// subnormal, huge, infinite, NaN, out of order) and over every pair of binades about
    /// the products' subnormal window (random significands and signs, a fixed seed).
    #[test]
    fn the_short_paths_are_the_long_ones_bit_for_bit() {
        let bits = |v: Ivl| (v.lo.to_bits(), v.hi.to_bits());
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut ends = vec![
            0.0,
            -0.0,
            1.0,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
            f64::MAX,
            1.0e-300,
            WIDEN_TINY,
            f64::from_bits(WIDEN_TINY.to_bits() - 1),
            2.0e-8,
            1.0e-160,
        ];
        for _ in 0..3000 {
            let r = next();
            let sign = if r & 1 == 0 { 1.0 } else { -1.0 };
            let unit = (next() >> 11) as f64 / (1_u64 << 53) as f64;
            let e = i32::try_from(next() % 640).unwrap_or(0) - 320;
            ends.push(f64::from_bits(next()));
            ends.push(sign * unit * 10f64.powi(e));
            ends.push(sign * 1.0e-300 * (1.0 + unit * 1e-14));
            ends.push(sign * unit * 1.0e-8);
        }
        let mut ivls = Vec::new();
        for (i, &a) in ends.iter().enumerate() {
            let b = ends.get((i * 7 + 3) % ends.len()).copied().unwrap_or(0.0);
            ivls.push(Ivl::new(a, b));
            ivls.push(Ivl::new(round_down(a), round_up(a)));
            let w = 1.0e-300 * (1.0 + a.abs().fract());
            ivls.push(Ivl::new(-w, w));
        }
        let f = |e: u64, r: u64, s: u64| f64::from_bits((s & (1 << 63)) | (e << 52) | (r >> 12));
        for ea in (1_u64..0x7ff).step_by(5) {
            for d in 960_u64..1035 {
                let Some(eb) = d.checked_sub(ea).filter(|&e| (1..0x7ff).contains(&e)) else {
                    continue;
                };
                let a = Ivl::new(f(ea, next(), next()), f(ea, next(), next()));
                let b = Ivl::new(f(eb, next(), next()), f(eb, next(), next()));
                assert_eq!(bits(a * b), bits(mul_exact_products(a, b)), "{a:?} * {b:?}");
                assert_eq!(bits(b * a), bits(mul_exact_products(b, a)), "{b:?} * {a:?}");
            }
        }
        for (i, &a) in ivls.iter().enumerate() {
            for &b in ivls.iter().skip(i % 13).step_by(13) {
                assert_eq!(bits(a * b), bits(mul_exact_products(a, b)), "{a:?} * {b:?}");
            }
            let long = if a.lo.is_nan() || a.hi.is_nan() {
                Ivl::whole()
            } else {
                Ivl::new(round_down(a.lo), round_up(a.hi))
            };
            assert_eq!(bits(Ivl::checked(a.lo, a.hi)), bits(long), "{a:?}");
        }
    }

    /// The floating tiny widening is the integer one bit for bit wherever it decides, over
    /// every normal tiny binade (random significands, a fixed seed) and at `a` just either side of
    /// each `k + 1/2` it refuses; it refuses only near those, and its constants are what
    /// they say.
    #[test]
    fn the_floating_tiny_widening_is_the_integer_one() {
        assert_eq!(TWO_52, 2f64.powi(52));
        assert_eq!(TIE_MARGIN, 2f64.powi(-24));
        assert_eq!(WIDEN_REL_GRID, WIDEN_REL * 2f64.powi(1000) * 2f64.powi(49));
        // 2^-1049 is the grid of 1e-300's binade
        assert_eq!(
            f64::from_bits(WIDEN_ABS.to_bits() + 1) - WIDEN_ABS,
            2f64.powi(-1049)
        );
        assert!(2f64.powi(-997) <= WIDEN_ABS && WIDEN_ABS < 2f64.powi(-996));
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let (mut decided, mut refused) = (0_u32, 0_u32);
        let mut judge = |a: f64| match widening_tiny_float(a) {
            Some(w) => {
                assert_eq!(
                    Some(w.to_bits()),
                    widening_tiny(a).map(f64::to_bits),
                    "{a:e}"
                );
                decided += 1;
            }
            None => refused += 1,
        };
        // a subnormal end is left to the integers
        assert!(widening_tiny_float(f64::from_bits(1)).is_none());
        assert!(widening_tiny_float(f64::MIN_POSITIVE / 2.0).is_none());
        for e in 1_u64..=55 {
            for _ in 0..20_000 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let a = f64::from_bits((e << 52) | (seed & ((1 << 52) - 1)));
                if a > 0.0 && a < WIDEN_TINY {
                    judge(a);
                }
            }
        }
        for k in [0_u32, 1, 2, 3, 7, 1000, 123_456, 99_999_999] {
            let a = (f64::from(k) + 0.5) / WIDEN_REL_GRID;
            let mut x = a;
            let mut y = a;
            for _ in 0..64 {
                judge(x);
                judge(y);
                x = f64::from_bits(x.to_bits() + 1);
                y = f64::from_bits(y.to_bits() - 1);
            }
        }
        // the refusals are the ones about a tie, a few per tie probed and almost none at random
        assert!(decided > 1_000_000 && refused < 1100, "{decided} {refused}");
        assert!(widening_tiny_float(1.0e-300).is_some());
    }

    #[test]
    fn operations_enclose_their_exact_results() {
        let a = Ivl::new(1.0, 2.0);
        let b = Ivl::new(-3.0, 0.5);
        let s = a + b;
        assert!(s.lo < -2.0 && s.hi > 2.5);
        let d = a - b;
        assert!(d.lo < 0.5 && d.hi > 5.0);
        let m = a * b;
        assert!(m.lo < -6.0 && m.hi > 1.0);
        assert!(m.lo > -6.0 - 1e-14 && m.hi < 1.0 + 1e-14);
        // 1 / 3 is not a double: the quotient's interval straddles it
        let q = Ivl::from(1.0) / Ivl::from(3.0);
        assert!(q.lo < 1.0 / 3.0 && q.hi > 1.0 / 3.0);
        assert!(q.is_positive());
    }

    #[test]
    fn undefined_results_are_the_whole_line() {
        assert_eq!(Ivl::from(1.0) / Ivl::new(-1.0, 1.0), Ivl::whole());
        assert_eq!(Ivl::new(-2.0, -1.0).sqrt(), Ivl::whole());
        assert_eq!(Ivl::whole() * Ivl::from(0.0), Ivl::whole());
        assert_eq!(Ivl::from(f64::NAN) + Ivl::from(1.0), Ivl::whole());
        // a NaN in the first product is kept by std::min and caught by sm_Checked
        assert_eq!(Ivl::new(f64::NAN, 1.0) * Ivl::from(2.0), Ivl::whole());
    }

    #[test]
    fn ulp_operations_enclose_one_ulp_outward_and_keep_exact_identities() {
        // 0.1 + 0.2 rounds to 0.30000000000000004; the exact sum of the two doubles is
        // within one ulp of it
        let s = Ivl::from(0.1).add_ulp(Ivl::from(0.2));
        let r: f64 = 0.1 + 0.2;
        assert_eq!(s.hi, f64::from_bits(r.to_bits() + 1));
        assert_eq!(s.lo, f64::from_bits(r.to_bits() - 1));
        let q = Ivl::from(1.0).div_ulp(Ivl::from(3.0));
        assert!(q.lo < 1.0 / 3.0 && q.hi > 1.0 / 3.0);
        assert_eq!(Ivl::from(1.0).div_ulp(Ivl::new(-1.0, 1.0)), Ivl::whole());
        let m = Ivl::new(1.0, 2.0).mul_ulp(Ivl::new(-3.0, 0.5));
        assert!(m.lo < -6.0 && m.hi > 1.0 && m.lo > -6.0 - 1e-14 && m.hi < 1.0 + 1e-14);
        // exact identities stay exact
        let x = Ivl::new(0.3, 0.7);
        assert_eq!(x.add_ulp(Ivl::from(0.0)), x);
        assert_eq!(x.mul_ulp(Ivl::from(1.0)), x);
        assert_eq!(x.mul_ulp(Ivl::from(0.0)), Ivl::from(0.0));
        assert_eq!(x.div_ulp(Ivl::from(1.0)), x);
        assert_eq!(Ivl::from(0.3).sub_ulp(Ivl::from(0.3)), Ivl::from(0.0));
        assert_eq!(Ivl::from(0.3).div_ulp(Ivl::from(0.3)), Ivl::from(1.0));
        // a wide interval minus itself holds every difference of its numbers
        let d = x.sub_ulp(x);
        assert!(d.lo < 0.3 - 0.7 && d.hi > 0.7 - 0.3);
        // not finite: the whole line, even times an exact 0
        assert_eq!(Ivl::whole().mul_ulp(Ivl::from(0.0)), Ivl::whole());
        assert_eq!(
            Ivl::from(f64::MAX).add_ulp(Ivl::from(f64::MAX)),
            Ivl::whole()
        );
        assert_eq!(next_up(-0.0), f64::from_bits(1));
        assert_eq!(next_up(-f64::from_bits(1)), -0.0);
        assert_eq!(next_up(f64::MAX), f64::INFINITY);
    }

    #[test]
    fn sqrt_mag_mid_hull() {
        let r = Ivl::new(-1.0, 4.0).sqrt();
        assert!(r.lo < 0.0 && r.lo > -1e-299 && r.hi > 2.0 && r.hi < 2.0 + 1e-15);
        assert_eq!(Ivl::new(-3.0, 2.0).mag(), 3.0);
        assert_eq!(Ivl::new(-3.0, 2.0).mid(), -0.5);
        assert_eq!(
            Ivl::new(0.0, 1.0).hull(Ivl::new(-1.0, 0.5)),
            Ivl::new(-1.0, 1.0)
        );
    }
}
