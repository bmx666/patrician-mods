// Compiles minimp3 (c/minimp3.h, CC0) for SSE2: no x87 code, which Rosetta 2 emulates slowly. The flags come
// after the defaults of cc (-march=i686 for i686 targets), so they win.
fn main() {
    println!("cargo:rerun-if-changed=c");
    cc::Build::new()
        .file("c/minimp3.c")
        .include("c")
        .opt_level(2)
        .flag("-march=pentium4")
        .flag("-msse2")
        .flag_if_supported("-mfpmath=sse")
        .flag_if_supported("-fno-stack-protector")
        .warnings(false)
        .compile("minimp3");
}
