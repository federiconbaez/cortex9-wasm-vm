//! Numeric instructions (opcodes 0x45..=0xbf): type signatures for the
//! validator and semantics for the interpreter. Values are raw 64-bit
//! slots; floats are kept as bits so NaN payloads are preserved where the
//! spec requires it (abs, neg, copysign, reinterpret, load, store).

use crate::error::Trap;
use crate::types::ValType::{self, F32, F64, I32, I64};

/// `(params, result)` of a numeric opcode, or `None` if it is not one.
pub fn signature(op: u8) -> Option<(&'static [ValType], ValType)> {
    Some(match op {
        0x45 => (&[I32], I32),
        0x46..=0x4f => (&[I32, I32], I32),
        0x50 => (&[I64], I32),
        0x51..=0x5a => (&[I64, I64], I32),
        0x5b..=0x60 => (&[F32, F32], I32),
        0x61..=0x66 => (&[F64, F64], I32),
        0x67..=0x69 => (&[I32], I32),
        0x6a..=0x78 => (&[I32, I32], I32),
        0x79..=0x7b => (&[I64], I64),
        0x7c..=0x8a => (&[I64, I64], I64),
        0x8b..=0x91 => (&[F32], F32),
        0x92..=0x98 => (&[F32, F32], F32),
        0x99..=0x9f => (&[F64], F64),
        0xa0..=0xa6 => (&[F64, F64], F64),
        0xa7 => (&[I64], I32),
        0xa8 | 0xa9 => (&[F32], I32),
        0xaa | 0xab => (&[F64], I32),
        0xac | 0xad => (&[I32], I64),
        0xae | 0xaf => (&[F32], I64),
        0xb0 | 0xb1 => (&[F64], I64),
        0xb2 | 0xb3 => (&[I32], F32),
        0xb4 | 0xb5 => (&[I64], F32),
        0xb6 => (&[F64], F32),
        0xb7 | 0xb8 => (&[I32], F64),
        0xb9 | 0xba => (&[I64], F64),
        0xbb => (&[F32], F64),
        0xbc => (&[F32], I32),
        0xbd => (&[F64], I64),
        0xbe => (&[I32], F32),
        0xbf => (&[I64], F64),
        _ => return None,
    })
}

#[inline]
fn f32_of(b: u64) -> f32 {
    f32::from_bits(b as u32)
}
#[inline]
fn f64_of(b: u64) -> f64 {
    f64::from_bits(b)
}
#[inline]
fn of_f32(f: f32) -> u64 {
    f.to_bits() as u64
}
#[inline]
fn of_f64(f: f64) -> u64 {
    f.to_bits()
}
/// Rounding functions return a signalling NaN input unchanged; wasm
/// requires an arithmetic (quiet) NaN.
#[inline]
fn quiet32(f: f32) -> f32 {
    if f.is_nan() {
        f32::from_bits(f.to_bits() | 0x0040_0000)
    } else {
        f
    }
}
#[inline]
fn quiet64(f: f64) -> f64 {
    if f.is_nan() {
        f64::from_bits(f.to_bits() | 0x0008_0000_0000_0000)
    } else {
        f
    }
}
#[inline]
fn b(c: bool) -> u64 {
    c as u64
}

macro_rules! fminmax {
    ($t:ty, $a:expr, $b:expr, $min:expr) => {{
        let (a, b) = ($a, $b);
        if a.is_nan() || b.is_nan() {
            // Propagates a (quieted) NaN operand: an arithmetic NaN.
            a + b
        } else if a == b {
            // Equal, possibly zeros of different sign: min prefers -0.
            let (x, y) = (a.to_bits(), b.to_bits());
            <$t>::from_bits(if $min { x | y } else { x & y })
        } else if $min {
            a.min(b)
        } else {
            a.max(b)
        }
    }};
}

macro_rules! nearest {
    ($x:expr) => {{
        let x = $x;
        if x.is_nan() {
            x + x // quiets the NaN, keeps its payload
        } else if x.is_infinite() || x == 0.0 {
            x
        } else {
            let r = x.round();
            let r = if (r - x).abs() == 0.5 {
                2.0 * (x / 2.0).round()
            } else {
                r
            };
            if r == 0.0 {
                r.copysign(x)
            } else {
                r
            }
        }
    }};
}

/// Pop operands, apply `op`, push the result.
#[inline]
pub fn exec(op: u8, s: &mut Vec<u64>) -> Result<(), Trap> {
    let (params, _) = signature(op).expect("validated numeric opcode");
    let y = if params.len() == 2 {
        s.pop().expect("validated")
    } else {
        0
    };
    let x = s.pop().expect("validated");
    let (a32, b32) = (x as u32 as i32, y as u32 as i32);
    let (a64, b64) = (x as i64, y as i64);
    let r: u64 = match op {
        0x45 => b(a32 == 0),
        0x46 => b(a32 == b32),
        0x47 => b(a32 != b32),
        0x48 => b(a32 < b32),
        0x49 => b((a32 as u32) < b32 as u32),
        0x4a => b(a32 > b32),
        0x4b => b(a32 as u32 > b32 as u32),
        0x4c => b(a32 <= b32),
        0x4d => b(a32 as u32 <= b32 as u32),
        0x4e => b(a32 >= b32),
        0x4f => b(a32 as u32 >= b32 as u32),
        0x50 => b(a64 == 0),
        0x51 => b(a64 == b64),
        0x52 => b(a64 != b64),
        0x53 => b(a64 < b64),
        0x54 => b((a64 as u64) < b64 as u64),
        0x55 => b(a64 > b64),
        0x56 => b(a64 as u64 > b64 as u64),
        0x57 => b(a64 <= b64),
        0x58 => b(a64 as u64 <= b64 as u64),
        0x59 => b(a64 >= b64),
        0x5a => b(a64 as u64 >= b64 as u64),
        0x5b..=0x60 => {
            let (p, q) = (f32_of(x), f32_of(y));
            b(match op {
                0x5b => p == q,
                0x5c => p != q,
                0x5d => p < q,
                0x5e => p > q,
                0x5f => p <= q,
                _ => p >= q,
            })
        }
        0x61..=0x66 => {
            let (p, q) = (f64_of(x), f64_of(y));
            b(match op {
                0x61 => p == q,
                0x62 => p != q,
                0x63 => p < q,
                0x64 => p > q,
                0x65 => p <= q,
                _ => p >= q,
            })
        }
        0x67 => a32.leading_zeros() as u64,
        0x68 => a32.trailing_zeros() as u64,
        0x69 => a32.count_ones() as u64,
        0x6a..=0x78 => {
            let v: i32 = match op {
                0x6a => a32.wrapping_add(b32),
                0x6b => a32.wrapping_sub(b32),
                0x6c => a32.wrapping_mul(b32),
                0x6d => {
                    if b32 == 0 {
                        return Err(Trap::DivByZero);
                    }
                    if a32 == i32::MIN && b32 == -1 {
                        return Err(Trap::IntOverflow);
                    }
                    a32 / b32
                }
                0x6e => {
                    if b32 == 0 {
                        return Err(Trap::DivByZero);
                    }
                    ((a32 as u32) / (b32 as u32)) as i32
                }
                0x6f => {
                    if b32 == 0 {
                        return Err(Trap::DivByZero);
                    }
                    a32.wrapping_rem(b32)
                }
                0x70 => {
                    if b32 == 0 {
                        return Err(Trap::DivByZero);
                    }
                    ((a32 as u32) % (b32 as u32)) as i32
                }
                0x71 => a32 & b32,
                0x72 => a32 | b32,
                0x73 => a32 ^ b32,
                0x74 => a32.wrapping_shl(b32 as u32),
                0x75 => a32.wrapping_shr(b32 as u32),
                0x76 => (a32 as u32).wrapping_shr(b32 as u32) as i32,
                0x77 => a32.rotate_left(b32 as u32 % 32),
                _ => a32.rotate_right(b32 as u32 % 32),
            };
            v as u32 as u64
        }
        0x79 => a64.leading_zeros() as u64,
        0x7a => a64.trailing_zeros() as u64,
        0x7b => a64.count_ones() as u64,
        0x7c..=0x8a => {
            let v: i64 = match op {
                0x7c => a64.wrapping_add(b64),
                0x7d => a64.wrapping_sub(b64),
                0x7e => a64.wrapping_mul(b64),
                0x7f => {
                    if b64 == 0 {
                        return Err(Trap::DivByZero);
                    }
                    if a64 == i64::MIN && b64 == -1 {
                        return Err(Trap::IntOverflow);
                    }
                    a64 / b64
                }
                0x80 => {
                    if b64 == 0 {
                        return Err(Trap::DivByZero);
                    }
                    ((a64 as u64) / (b64 as u64)) as i64
                }
                0x81 => {
                    if b64 == 0 {
                        return Err(Trap::DivByZero);
                    }
                    a64.wrapping_rem(b64)
                }
                0x82 => {
                    if b64 == 0 {
                        return Err(Trap::DivByZero);
                    }
                    ((a64 as u64) % (b64 as u64)) as i64
                }
                0x83 => a64 & b64,
                0x84 => a64 | b64,
                0x85 => a64 ^ b64,
                0x86 => a64.wrapping_shl(b64 as u32),
                0x87 => a64.wrapping_shr(b64 as u32),
                0x88 => (a64 as u64).wrapping_shr(b64 as u32) as i64,
                0x89 => a64.rotate_left((b64 as u64 % 64) as u32),
                _ => a64.rotate_right((b64 as u64 % 64) as u32),
            };
            v as u64
        }
        0x8b => x & 0x7fff_ffff,
        0x8c => (x as u32 ^ 0x8000_0000) as u64,
        0x8d => of_f32(quiet32(f32_of(x).ceil())),
        0x8e => of_f32(quiet32(f32_of(x).floor())),
        0x8f => of_f32(quiet32(f32_of(x).trunc())),
        0x90 => of_f32(nearest!(f32_of(x))),
        0x91 => of_f32(f32_of(x).sqrt()),
        0x92 => of_f32(f32_of(x) + f32_of(y)),
        0x93 => of_f32(f32_of(x) - f32_of(y)),
        0x94 => of_f32(f32_of(x) * f32_of(y)),
        0x95 => of_f32(f32_of(x) / f32_of(y)),
        0x96 => of_f32(fminmax!(f32, f32_of(x), f32_of(y), true)),
        0x97 => of_f32(fminmax!(f32, f32_of(x), f32_of(y), false)),
        0x98 => (x & 0x7fff_ffff) | (y & 0x8000_0000),
        0x99 => x & 0x7fff_ffff_ffff_ffff,
        0x9a => x ^ 0x8000_0000_0000_0000,
        0x9b => of_f64(quiet64(f64_of(x).ceil())),
        0x9c => of_f64(quiet64(f64_of(x).floor())),
        0x9d => of_f64(quiet64(f64_of(x).trunc())),
        0x9e => of_f64(nearest!(f64_of(x))),
        0x9f => of_f64(f64_of(x).sqrt()),
        0xa0 => of_f64(f64_of(x) + f64_of(y)),
        0xa1 => of_f64(f64_of(x) - f64_of(y)),
        0xa2 => of_f64(f64_of(x) * f64_of(y)),
        0xa3 => of_f64(f64_of(x) / f64_of(y)),
        0xa4 => of_f64(fminmax!(f64, f64_of(x), f64_of(y), true)),
        0xa5 => of_f64(fminmax!(f64, f64_of(x), f64_of(y), false)),
        0xa6 => (x & 0x7fff_ffff_ffff_ffff) | (y & 0x8000_0000_0000_0000),
        0xa7 => x as u32 as u64,
        0xa8 => trunc(f32_of(x) as f64, -2147483649.0, 2147483648.0)? as i32 as u32 as u64,
        0xa9 => trunc(f32_of(x) as f64, -1.0, 4294967296.0)? as u32 as u64,
        0xaa => trunc(f64_of(x), -2147483649.0, 2147483648.0)? as i32 as u32 as u64,
        0xab => trunc(f64_of(x), -1.0, 4294967296.0)? as u32 as u64,
        0xac => a32 as i64 as u64,
        0xad => x as u32 as u64,
        0xae => trunc_i64(f32_of(x) as f64)? as u64,
        0xaf => trunc_u64(f32_of(x) as f64)?,
        0xb0 => trunc_i64(f64_of(x))? as u64,
        0xb1 => trunc_u64(f64_of(x))?,
        0xb2 => of_f32(a32 as f32),
        0xb3 => of_f32(a32 as u32 as f32),
        0xb4 => of_f32(a64 as f32),
        0xb5 => of_f32(a64 as u64 as f32),
        0xb6 => of_f32(f64_of(x) as f32),
        0xb7 => of_f64(a32 as f64),
        0xb8 => of_f64(a32 as u32 as f64),
        0xb9 => of_f64(a64 as f64),
        0xba => of_f64(a64 as u64 as f64),
        0xbb => of_f64(f32_of(x) as f64),
        // Reinterpretations: the slot already holds the bits.
        0xbc..=0xbf => x,
        _ => unreachable!("not a numeric opcode: {op:#x}"),
    };
    s.push(r);
    Ok(())
}

/// Truncate toward zero; valid when `lo < x < hi` (exclusive bounds).
/// Every f32 value is exactly representable as f64, so f32 inputs are
/// widened first.
fn trunc(x: f64, lo: f64, hi: f64) -> Result<f64, Trap> {
    if x.is_nan() {
        return Err(Trap::InvalidConversion);
    }
    if !(x > lo && x < hi) {
        return Err(Trap::IntOverflow);
    }
    Ok(x.trunc())
}

fn trunc_i64(x: f64) -> Result<i64, Trap> {
    if x.is_nan() {
        return Err(Trap::InvalidConversion);
    }
    // -2^63 is exact; 2^63 is the first value out of range.
    if !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&x) {
        return Err(Trap::IntOverflow);
    }
    Ok(x as i64)
}

fn trunc_u64(x: f64) -> Result<u64, Trap> {
    if x.is_nan() {
        return Err(Trap::InvalidConversion);
    }
    if !(x > -1.0 && x < 18_446_744_073_709_551_616.0) {
        return Err(Trap::IntOverflow);
    }
    Ok(x as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(op: u8, args: &[u64]) -> Result<u64, Trap> {
        let mut s = args.to_vec();
        exec(op, &mut s)?;
        Ok(s[0])
    }

    #[test]
    fn integer_edge_cases() {
        let min = i32::MIN as u32 as u64;
        let m1 = -1i32 as u32 as u64;
        assert_eq!(run(0x6d, &[min, m1]), Err(Trap::IntOverflow));
        assert_eq!(run(0x6f, &[min, m1]), Ok(0));
        assert_eq!(run(0x6d, &[1, 0]), Err(Trap::DivByZero));
        assert_eq!(run(0x74, &[1, 33]), Ok(2), "shift count masked");
        assert_eq!(run(0x77, &[0x8000_0000, 1]), Ok(1));
        assert_eq!(run(0x67, &[0]), Ok(32));
        assert_eq!(run(0x75, &[min, 31]), Ok(m1));
    }

    #[test]
    fn float_edge_cases() {
        let f = |v: f32| v.to_bits() as u64;
        let nz = f(-0.0);
        let pz = f(0.0);
        assert_eq!(run(0x96, &[pz, nz]), Ok(nz), "min(+0, -0) = -0");
        assert_eq!(run(0x97, &[nz, pz]), Ok(pz), "max(-0, +0) = +0");
        assert!(f32::from_bits(run(0x96, &[f(f32::NAN), f(1.0)]).unwrap() as u32).is_nan());
        assert_eq!(run(0x90, &[f(2.5)]), Ok(f(2.0)), "ties to even");
        assert_eq!(run(0x90, &[f(-0.5)]), Ok(nz), "keeps the sign of zero");
        assert_eq!(run(0x90, &[f(3.5)]), Ok(f(4.0)));
        // neg flips only the sign bit, even of a NaN payload.
        assert_eq!(run(0x8c, &[0x7fa0_0000]), Ok(0xffa0_0000));
    }

    #[test]
    fn truncation_bounds() {
        let f = |v: f64| v.to_bits();
        assert_eq!(run(0xaa, &[f(-2147483648.9)]), Ok(i32::MIN as u32 as u64));
        assert_eq!(run(0xaa, &[f(-2147483649.0)]), Err(Trap::IntOverflow));
        assert_eq!(run(0xab, &[f(-0.9)]), Ok(0));
        assert_eq!(run(0xab, &[f(4294967296.0)]), Err(Trap::IntOverflow));
        assert_eq!(run(0xb0, &[f(f64::NAN)]), Err(Trap::InvalidConversion));
        assert_eq!(
            run(0xb1, &[f(18446744073709549568.0)]),
            Ok(18446744073709549568)
        );
    }
}
