pub const MATH_PI: u8 = 0;
pub const MATH_E: u8 = 1;
pub const MATH_TAU: u8 = 2;
pub const MATH_SQRT: u8 = 3;
pub const MATH_SIN: u8 = 4;
pub const MATH_COS: u8 = 5;
pub const MATH_TAN: u8 = 6;
pub const MATH_ASIN: u8 = 7;
pub const MATH_ACOS: u8 = 8;
pub const MATH_ATAN: u8 = 9;
pub const MATH_EXP: u8 = 10;
pub const MATH_LN: u8 = 11;
pub const MATH_LOG10: u8 = 12;
pub const MATH_FLOOR: u8 = 13;
pub const MATH_CEIL: u8 = 14;
pub const MATH_ROUND: u8 = 15;
pub const MATH_TRUNC: u8 = 16;
pub const MATH_POW: u8 = 17;
pub const MATH_ATAN2: u8 = 18;
pub const MATH_HYPOT: u8 = 19;

#[inline]
pub fn eval_math_op(aux: u8, x: f64, y: f64) -> Option<f64> {
    Some(match aux {
        MATH_PI => std::f64::consts::PI,
        MATH_E => std::f64::consts::E,
        MATH_TAU => std::f64::consts::TAU,
        MATH_SQRT => x.sqrt(),
        MATH_SIN => x.sin(),
        MATH_COS => x.cos(),
        MATH_TAN => x.tan(),
        MATH_ASIN => x.asin(),
        MATH_ACOS => x.acos(),
        MATH_ATAN => x.atan(),
        MATH_EXP => x.exp(),
        MATH_LN => x.ln(),
        MATH_LOG10 => x.log10(),
        MATH_FLOOR => x.floor(),
        MATH_CEIL => x.ceil(),
        MATH_ROUND => x.round(),
        MATH_TRUNC => x.trunc(),
        MATH_POW => x.powf(y),
        MATH_ATAN2 => x.atan2(y),
        MATH_HYPOT => x.hypot(y),
        _ => return None,
    })
}
