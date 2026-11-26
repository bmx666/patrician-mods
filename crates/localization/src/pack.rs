//! Files of one language for the running game: the archive `mods\localization\<lang>.cpr` (both games in one file,
//! entries `p2\...` / `p3\...`, format in the `cpr` crate, made by `cprtool pack`) and the loose folder
//! `mods\localization\<lang>\<p2|p3>\`, which wins over the archive (tutorial voice-over, files under test).
//! Keys: path below the game part, lower case (ASCII), `\` separators; archive names are Latin-1 bytes (the ANSI
//! names the game asks for, e.g. `ü` = 0xFC). Both are indexed once at start.
use pcore::game::Base;
use pcore::paths;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};

pub struct Pack {
    /// Loose folder (need not exist).
    dir: Option<PathBuf>,
    loose: HashSet<String>,
    /// Archive with entries of the running game.
    archive: Option<PathBuf>,
    entries: HashMap<String, cpr::Entry>,
    /// Why the archive could not be read.
    pub error: Option<String>,
}

impl Pack {
    pub fn open(lang: &str, base: Base) -> Pack {
        let dir = paths::localization_dir(lang, base);
        let mut loose = HashSet::new();
        if let Some(d) = &dir {
            walk(d, &mut String::new(), &mut loose);
        }
        let archive = paths::localization_archive(lang).filter(|p| p.is_file());
        let (entries, error) = match archive.as_deref().map(|p| entries(p, base)) {
            Some(Ok(e)) => (e, None),
            Some(Err(e)) => (HashMap::new(), Some(e)),
            None => (HashMap::new(), None),
        };
        let archive = archive.filter(|_| !entries.is_empty());
        Pack { dir, loose, archive, entries, error }
    }

    /// Keys of the loose folder.
    pub fn loose(&self) -> impl Iterator<Item = &str> {
        self.loose.iter().map(String::as_str)
    }

    /// Keys of the archive with their entry names.
    pub fn packed(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.entries.iter().map(|(k, e)| (k.as_str(), e.name.as_slice()))
    }

    /// The archive, when it has entries of the running game.
    pub fn archive(&self) -> Option<&Path> {
        self.archive.as_deref()
    }

    /// Contents of `key`: the loose file, else the archive entry.
    pub fn read(&self, key: &str) -> Option<Vec<u8>> {
        if self.loose.contains(key) {
            if let Some(d) = &self.dir {
                if let Ok(data) = std::fs::read(d.join(key)) {
                    return Some(data);
                }
            }
        }
        let e = self.entries.get(key)?;
        let mut f = File::open(self.archive.as_ref()?).ok()?;
        cpr::read(&mut f, e).ok()
    }

    /// Keys of the files directly in `dir` (a key, e.g. `exe`) with extension `ext`, sorted.
    pub fn list(&self, dir: &str, ext: &str) -> Vec<String> {
        let tail = format!(".{ext}");
        let mut v: Vec<String> = self
            .loose()
            .chain(self.entries.keys().map(String::as_str))
            .filter(|k| {
                k.strip_prefix(dir)
                    .and_then(|r| r.strip_prefix('\\'))
                    .is_some_and(|f| !f.contains('\\') && f.len() > tail.len() && f.ends_with(&tail))
            })
            .map(str::to_string)
            .collect();
        v.sort();
        v.dedup();
        v
    }
}

/// Whether `lang` has files of `base`: its loose folder or entries in its archive.
pub fn has_game(lang: &str, base: Base) -> bool {
    paths::localization_dir(lang, base).is_some_and(|d| d.is_dir())
        || paths::localization_archive(lang)
            .filter(|p| p.is_file())
            .is_some_and(|p| entries(&p, base).is_ok_and(|e| !e.is_empty()))
}

/// Entries of `base` in the archive at `path`, by key.
fn entries(path: &Path, base: Base) -> Result<HashMap<String, cpr::Entry>, String> {
    let fail = |e: std::io::Error| format!("{}: {e}", path.file_name().unwrap_or_default().to_string_lossy());
    let mut f = File::open(path).map_err(fail)?;
    let prefix = format!("{}\\", base.dir());
    let mut out = HashMap::new();
    for e in cpr::index(&mut f).map_err(fail)? {
        let name: String = e.name.iter().map(|&c| c as char).collect();
        let name = name.replace('/', "\\").to_ascii_lowercase();
        if let Some(k) = name.strip_prefix(&prefix).filter(|k| !k.is_empty()) {
            out.insert(k.to_string(), e);
        }
    }
    Ok(out)
}

fn walk(dir: &Path, rel: &mut String, out: &mut HashSet<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_ascii_lowercase();
        let keep = rel.len();
        if !rel.is_empty() {
            rel.push('\\');
        }
        rel.push_str(&name);
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(&e.path(), rel, out),
            Ok(_) => {
                out.insert(rel.clone());
            }
            Err(_) => {}
        }
        rel.truncate(keep);
    }
}
