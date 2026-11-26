// Generates one exported jump thunk per name of exports.txt: `jmp dword ptr [TABLE + 4*i]`. The thunks use the C
// calling convention only for their symbol (no stdcall `@N` decoration in the export name); they never touch the
// stack, so callers and the system function see the original stdcall frame.
use std::{env, fmt::Write, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=exports.txt");
    let text = fs::read_to_string("exports.txt").expect("exports.txt");
    let names: Vec<&str> =
        text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    let mut out = String::new();
    writeln!(out, "pub const COUNT: usize = {};", names.len()).unwrap();
    writeln!(out, "pub static NAMES: [&[u8]; COUNT] = [").unwrap();
    for n in &names {
        writeln!(out, "    b\"{n}\\0\",").unwrap();
    }
    writeln!(out, "];").unwrap();
    for (i, n) in names.iter().enumerate() {
        writeln!(
            out,
            "#[unsafe(naked)]\n#[export_name = \"{n}\"]\npub unsafe extern \"C\" fn fwd_{i}() {{\n    \
             core::arch::naked_asm!(\"jmp dword ptr [{{t}} + {off}]\", t = sym crate::TABLE)\n}}",
            off = i * 4
        )
        .unwrap();
    }
    let path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("exports.rs");
    fs::write(path, out).unwrap();
}
