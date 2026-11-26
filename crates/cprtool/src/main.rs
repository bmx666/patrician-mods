//! `cprtool pack <out.cpr> <dir> [--skip <path>]...`: archive of every file below `dir`. Entry name = the relative
//! path, lower case (ASCII letters), `\` separators, written as Latin-1 bytes (the ANSI names the game asks for,
//! e.g. `ü` = 0xFC; other characters are refused); hidden files (`.name`) and everything below a `--skip` path
//! (relative to `dir`, e.g. `p2/musik`) are left out; equal contents are stored once.
//!
//! `cprtool unpack <in.cpr> <dir>`: writes every stored entry to `dir\<name>`.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: cprtool pack <out.cpr> <dir> [--skip <path>]...\n       cprtool unpack <in.cpr> <dir>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("pack") if args.len() >= 3 => pack(Path::new(&args[1]), Path::new(&args[2]), &args[3..]),
        Some("unpack") if args.len() == 3 => unpack(Path::new(&args[1]), Path::new(&args[2])),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(msg) => {
            println!("{msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("cprtool: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Entry name of a relative path: lower case, `\` separators.
fn key(rel: &str) -> String {
    rel.replace('/', "\\").trim_matches('\\').to_ascii_lowercase()
}

fn pack(out: &Path, dir: &Path, rest: &[String]) -> Result<String, String> {
    let mut skip = Vec::new();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--skip", Some(p)) => skip.push(key(p)),
            _ => return Err(USAGE.to_string()),
        }
    }
    let mut files = Vec::new();
    walk(dir, String::new(), &skip, &mut files)?;
    files.sort();
    if let Some(w) = files.windows(2).find(|w| w[0].0 == w[1].0) {
        return Err(format!("{}: two files differ only in case", w[0].0));
    }
    let mut entries = Vec::with_capacity(files.len());
    for (name, path) in &files {
        let data = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        entries.push((name.chars().map(|c| c as u8).collect::<Vec<u8>>(), data));
    }
    let bytes = cpr::write(&entries).map_err(|e| format!("{}: {e}", dir.display()))?;
    fs::write(out, &bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    let total: usize = entries.iter().map(|(_, d)| d.len()).sum();
    Ok(format!("{}: {} files, {} -> {} bytes", out.display(), entries.len(), total, bytes.len()))
}

fn walk(dir: &Path, rel: String, skip: &[String], out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let rd = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for e in rd {
        let e = e.map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = e.file_name().into_string().map_err(|n| format!("{}: not a UTF-8 name", Path::new(&n).display()))?;
        if name.starts_with('.') {
            continue;
        }
        if name.chars().any(|c| c as u32 > 0xFF) {
            return Err(format!("{}: entry names must be Latin-1", e.path().display()));
        }
        let k = if rel.is_empty() { key(&name) } else { format!("{rel}\\{}", key(&name)) };
        if skip.iter().any(|s| k == *s || k.starts_with(&format!("{s}\\"))) {
            continue;
        }
        let t = e.file_type().map_err(|err| format!("{}: {err}", e.path().display()))?;
        if t.is_dir() {
            walk(&e.path(), k, skip, out)?;
        } else {
            out.push((k, e.path()));
        }
    }
    Ok(())
}

fn unpack(input: &Path, dir: &Path) -> Result<String, String> {
    let fail = |e: std::io::Error| format!("{}: {e}", input.display());
    let mut f = fs::File::open(input).map_err(fail)?;
    let entries = cpr::index(&mut f).map_err(fail)?;
    for e in &entries {
        let name: String = e.name.iter().map(|&c| c as char).collect();
        let parts: Vec<&str> = name.split(['\\', '/']).collect();
        if parts.iter().any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains(':')) {
            return Err(format!("{}: unsafe entry name {name:?}", input.display()));
        }
        let path = parts.iter().fold(dir.to_path_buf(), |p, c| p.join(c));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
        }
        let data = cpr::read(&mut f, e).map_err(fail)?;
        fs::write(&path, data).map_err(|err| format!("{}: {err}", path.display()))?;
    }
    Ok(format!("{}: {} files -> {}", input.display(), entries.len(), dir.display()))
}
