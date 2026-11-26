//! `mods\settings.ini`: `[section]` / `key=value` lines, `;` or `#` comments. Section and key names ignore case; a
//! repeated key takes its last value. A missing file or key means the default (everything enabled).
use std::path::Path;

#[derive(Default)]
pub struct Ini {
    entries: Vec<(String, String, String)>,
}

impl Ini {
    pub fn load(path: Option<&Path>) -> Ini {
        let text = path.and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
        Ini::parse(&String::from_utf8_lossy(&text))
    }

    pub fn parse(text: &str) -> Ini {
        let mut ini = Ini::default();
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim().trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.trim().to_ascii_lowercase();
            } else if let Some((k, v)) = line.split_once('=') {
                ini.entries.push((section.clone(), k.trim().to_ascii_lowercase(), v.trim().to_string()));
            }
        }
        ini
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|(s, k, _)| s.eq_ignore_ascii_case(section) && k.eq_ignore_ascii_case(key))
            .map(|(_, _, v)| v.as_str())
    }

    /// `1 / true / yes / on` = true, `0 / false / no / off` = false, missing or anything else = `default`.
    pub fn flag(&self, section: &str, key: &str, default: bool) -> bool {
        match self.get(section, key).map(|v| v.to_ascii_lowercase()) {
            Some(v) if matches!(v.as_str(), "1" | "true" | "yes" | "on") => true,
            Some(v) if matches!(v.as_str(), "0" | "false" | "no" | "off") => false,
            _ => default,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Ini;

    #[test]
    fn flags() {
        let ini = Ini::parse("; c\n[QoL]\nWasd=0\nwasd = 1\nspeed_keys=off\n[other]\nx=0\n");
        assert!(ini.flag("qol", "wasd", false));
        assert!(!ini.flag("qol", "speed_keys", true));
        assert!(ini.flag("qol", "missing", true));
        assert!(!ini.flag("other", "x", true));
    }
}
