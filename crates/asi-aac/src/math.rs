//! The C math functions FAAD3 calls, from the `libm` crate (SSE2). Defined here, they win over the import
//! library: msvcrt's versions run on the x87 unit, which Rosetta 2 emulates slowly.

macro_rules! export {
    ($($name:ident($($arg:ident: $t:ty),*) -> $r:ty;)*) => {$(
        #[no_mangle]
        pub extern "C" fn $name($($arg: $t),*) -> $r {
            libm::$name($($arg),*)
        }
    )*};
}

export! {
    sin(x: f64) -> f64;
    cos(x: f64) -> f64;
    exp(x: f64) -> f64;
    log(x: f64) -> f64;
    log2(x: f64) -> f64;
    log10(x: f64) -> f64;
    pow(x: f64, y: f64) -> f64;
    sqrt(x: f64) -> f64;
    floor(x: f64) -> f64;
    atan(x: f64) -> f64;
    atan2(y: f64, x: f64) -> f64;
    acos(x: f64) -> f64;
    sinf(x: f32) -> f32;
    sqrtf(x: f32) -> f32;
    floorf(x: f32) -> f32;
    powf(x: f32, y: f32) -> f32;
    exp2f(x: f32) -> f32;
    hypotf(x: f32, y: f32) -> f32;
    roundf(x: f32) -> f32;
    fabsf(x: f32) -> f32;
}

/// Round to nearest, ties to even (the default MXCSR mode); C `long` is 32-bit on Windows.
#[no_mangle]
pub extern "C" fn lrintf(x: f32) -> i32 {
    libm::rintf(x) as i32
}
