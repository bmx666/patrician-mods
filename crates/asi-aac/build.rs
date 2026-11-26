// Compiles the FAAD3 subset (faad3/, LGPL-2.1-or-later, see faad3/UPSTREAM) for SSE2, as its "build without
// Meson": MAX_CHANNELS 2 (mono / stereo, PS on, no coupling.c), no DAB+ (960 frames), no DRC. The flags come
// after the defaults of cc (-march=i686 for i686 targets), so they win.
const LIBFAAD: &[&str] =
    &["decoder", "bits", "asc", "huffman", "syntax", "dequant", "stereo", "tns", "imdct", "sbr", "ps", "sbr_dec_tables"];
const COMMON: &[&str] = &["fft", "sbr_tables", "sfb_tables"];

fn main() {
    println!("cargo:rerun-if-changed=faad3");
    let mut b = cc::Build::new();
    b.files(LIBFAAD.iter().map(|f| format!("faad3/libfaad/{f}.c")))
        .files(COMMON.iter().map(|f| format!("faad3/common/{f}.c")))
        .include("faad3/include")
        .include("faad3/common")
        .include("faad3/libfaad")
        .define("MAX_CHANNELS", "2")
        .define("FAAD_DISABLE_DAB", None)
        .define("FAAD_DISABLE_DRC", None)
        .define("COMMON_PREFIX", "faad_")
        .define("PACKAGE", "\"faad\"")
        .define("PACKAGE_VERSION", "\"3.0.0\"")
        .opt_level(2)
        .flag("-march=pentium4")
        .flag("-msse2")
        .flag_if_supported("-mfpmath=sse")
        .flag_if_supported("-fno-stack-protector")
        .warnings(false)
        .compile("faad3");
}
