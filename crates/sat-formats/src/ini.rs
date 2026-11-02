//! Ini files as a model that keeps the file: sections, keys, values — comments, order and line ends intact. @sat:formats.ini
//!
//! Owns: reading an ini (any mod's settings file: `[Section]`, `key=value`, `;`/`#` comments), reading and setting a
//! value, removing a key, and writing the file back so that only the changed lines differ.
//! Not here: MO2's own ini dialect (`@sat:mo2.ini`), patches as project data (`@sat:patch`).
//!
//! Keys and sections match without regard to letter case (as the game's ini reader does); the file keeps the
//! spelling it had. A key set in a section that does not exist adds the section at the end.

/// One line of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    /// Comments, blank lines, anything that is not a section or a key: kept as is.
    Other(String),
    Section {
        raw: String,
        name: String,
    },
    Key {
        /// The text before `=` as written (spaces kept).
        key_raw: String,
        key: String,
        value: String,
        /// Whether a space surrounded `=` (kept when the value changes).
        spaced: bool,
    },
}

// @sat:formats.ini.model
/// An ini file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IniFile {
    lines: Vec<Line>,
    crlf: bool,
    bom: bool,
    final_newline: bool,
}

fn fold(s: &str) -> String {
    s.trim().to_lowercase()
}

impl IniFile {
    pub fn parse(bytes: &[u8]) -> Self {
        let (bom, body) = match bytes {
            [0xEF, 0xBB, 0xBF, rest @ ..] => (true, rest),
            _ => (false, bytes),
        };
        let text = String::from_utf8_lossy(body);
        let crlf = text.contains("\r\n");
        let final_newline = text.ends_with('\n');
        let lines = text
            .lines()
            .map(|l| {
                let t = l.trim();
                if t.starts_with('[') && t.ends_with(']') && t.len() >= 2 {
                    Line::Section {
                        raw: l.to_string(),
                        name: fold(&t[1..t.len() - 1]),
                    }
                } else if !t.starts_with(';') && !t.starts_with('#') && t.contains('=') {
                    let (k, v) = l.split_once('=').expect("contains =");
                    Line::Key {
                        key_raw: k.to_string(),
                        key: fold(k),
                        spaced: v.starts_with(' '),
                        value: v.trim().to_string(),
                    }
                } else {
                    Line::Other(l.to_string())
                }
            })
            .collect();
        Self {
            lines,
            crlf,
            bom,
            final_newline,
        }
    }

    /// The line index of `key` in `section` (`""` — before the first section).
    fn find(&self, section: &str, key: &str) -> Option<usize> {
        let (section, key) = (fold(section), fold(key));
        let mut cur = String::new();
        for (i, l) in self.lines.iter().enumerate() {
            match l {
                Line::Section { name, .. } => cur = name.clone(),
                Line::Key { key: k, .. } if cur == section && *k == key => return Some(i),
                _ => {}
            }
        }
        None
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        match &self.lines[self.find(section, key)?] {
            Line::Key { value, .. } => Some(value),
            _ => None,
        }
    }

    // @sat:formats.ini.set
    /// Sets a value: in place when the key exists, else at the end of its section (the section is added if
    /// missing). `false` when nothing changed.
    pub fn set(&mut self, section: &str, key: &str, value: &str) -> bool {
        if let Some(i) = self.find(section, key) {
            if let Line::Key { value: v, .. } = &mut self.lines[i] {
                if v == value {
                    return false;
                }
                *v = value.to_string();
            }
            return true;
        }
        let line = Line::Key {
            key_raw: key.to_string(),
            key: fold(key),
            value: value.to_string(),
            spaced: false,
        };
        let s = fold(section);
        // the end of the section: the line after its last key (or its header)
        let mut cur = String::new();
        let mut at = None;
        for (i, l) in self.lines.iter().enumerate() {
            match l {
                Line::Section { name, .. } => {
                    cur = name.clone();
                    if cur == s {
                        at = Some(i + 1);
                    }
                }
                Line::Key { .. } if cur == s => at = Some(i + 1),
                _ => {}
            }
        }
        match at {
            Some(i) => self.lines.insert(i, line),
            None if s.is_empty() => self.lines.insert(0, line),
            None => {
                self.lines.push(Line::Section {
                    raw: format!("[{}]", section.trim()),
                    name: s,
                });
                self.lines.push(line);
            }
        }
        true
    }

    /// Removes a key; `false` when it was not there.
    pub fn remove(&mut self, section: &str, key: &str) -> bool {
        match self.find(section, key) {
            Some(i) => {
                self.lines.remove(i);
                true
            }
            None => false,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let nl = if self.crlf { "\r\n" } else { "\n" };
        let mut out = String::new();
        for (i, l) in self.lines.iter().enumerate() {
            if i > 0 {
                out.push_str(nl);
            }
            match l {
                Line::Other(s) => out.push_str(s),
                Line::Section { raw, .. } => out.push_str(raw),
                Line::Key {
                    key_raw,
                    value,
                    spaced,
                    ..
                } => {
                    out.push_str(key_raw);
                    out.push('=');
                    if *spaced {
                        out.push(' ');
                    }
                    out.push_str(value);
                }
            }
        }
        if self.final_newline && !self.lines.is_empty() {
            out.push_str(nl);
        }
        let mut bytes = Vec::with_capacity(out.len() + 3);
        if self.bom {
            bytes.extend([0xEF, 0xBB, 0xBF]);
        }
        bytes.extend(out.into_bytes());
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_changed_lines_differ() {
        let src =
            b"; physics\r\n[Breast]\r\nfStiffness = 0.5\r\nfDamping=0.2\r\n\r\n[Butt]\r\nfStiffness=0.3\r\n";
        let mut f = IniFile::parse(src);
        assert_eq!(f.get("breast", "FSTIFFNESS"), Some("0.5"));
        assert_eq!(f.to_bytes(), src, "an untouched file round-trips byte for byte");
        assert!(f.set("Breast", "fStiffness", "0.8"));
        assert!(!f.set("Breast", "fStiffness", "0.8"), "same value: no change");
        assert!(f.set("Butt", "fNew", "1"));
        assert!(f.set("Belly", "fStiffness", "0.1"));
        assert!(f.remove("Breast", "fDamping"));
        assert_eq!(
            String::from_utf8(f.to_bytes()).unwrap(),
            "; physics\r\n[Breast]\r\nfStiffness = 0.8\r\n\r\n[Butt]\r\nfStiffness=0.3\r\nfNew=1\r\n[Belly]\r\nfStiffness=0.1\r\n"
        );
    }
}
