//! Qt `QSettings` INI as written by MO2. @sat:mo2.ini
//!
//! Owns: Qt INI reading/escaping as MO2 writes it.

use std::collections::BTreeMap;

/// `section -> key -> raw value`; section and key names are lowercased.
pub type Ini = BTreeMap<String, BTreeMap<String, String>>;

pub fn strip_bom(b: &[u8]) -> &[u8] {
    b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b)
}

pub fn parse(data: &[u8]) -> Ini {
    let text = String::from_utf8_lossy(strip_bom(data));
    let mut out = Ini::new();
    let mut section = String::new();
    out.entry(section.clone()).or_default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.to_lowercase();
            out.entry(section.clone()).or_default();
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.entry(section.clone())
                .or_default()
                .insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    out
}

/// Unwraps `@ByteArray(...)`, surrounding quotes and doubled backslashes.
pub fn unquote(v: &str) -> String {
    let mut v = v.trim();
    if let Some(inner) = v.strip_prefix("@ByteArray(").and_then(|s| s.strip_suffix(')')) {
        v = inner;
    }
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        v = &v[1..v.len() - 1];
    }
    v.replace("\\\\", "\\")
}

/// Decodes MO2's `color=@Variant(\0\0\0\x43\x1\xff\xff\x80\x80…)` (a serialized QColor) into `#rrggbb`.
pub fn decode_qcolor(v: &str) -> Option<String> {
    let v = v.trim();
    if v.len() == 7 && v.starts_with('#') && v[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(v.to_ascii_lowercase());
    }
    let inner = v.strip_prefix("@Variant(")?.strip_suffix(')')?;
    let raw = unescape_c(inner);
    if raw.len() < 4 || raw[3] != 0x43 {
        return None; // QMetaType::QColor = 67
    }
    let mut p = &raw[4..];
    if p.len() > 1 && p[0] == 0 && p[1] != 0 {
        p = &p[1..]; // optional isNull byte
    }
    if p.len() < 9 || p[0] == 0 {
        return None; // spec Invalid
    }
    // spec, alpha16, red16, green16, blue16 — big endian, keep the high byte
    Some(format!("#{:02x}{:02x}{:02x}", p[3], p[5], p[7]))
}

fn unescape_c(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' || i + 1 >= b.len() {
            out.push(b[i]);
            i += 1;
            continue;
        }
        i += 1;
        match b[i] {
            b'0' => out.push(0),
            b'x' => {
                let start = i + 1;
                let mut end = start;
                while end < b.len() && end < start + 2 && b[end].is_ascii_hexdigit() {
                    end += 1;
                }
                out.push(u8::from_str_radix(&s[start..end], 16).unwrap_or(0));
                i = end - 1;
            }
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            c => out.push(c),
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qcolor_variants() {
        assert_eq!(
            decode_qcolor(r"@Variant(\0\0\0\x43\x1\xff\xff\x80\x80\x30\x30\xd0\xd0\0\0)").as_deref(),
            Some("#8030d0")
        );
        assert_eq!(
            decode_qcolor(r"@Variant(\0\0\0\x43\0\x1\xff\xff\x10\x10\x20\x20\x30\x30\0\0)").as_deref(),
            Some("#102030")
        );
        assert_eq!(decode_qcolor("#AABBCC").as_deref(), Some("#aabbcc"));
        assert_eq!(decode_qcolor(r"@Variant(\0\0\0\x43\0\0\0)"), None);
        assert_eq!(decode_qcolor("garbage"), None);
    }

    #[test]
    fn sections_and_bytearray() {
        let ini = parse(b"\xEF\xBB\xBF[General]\r\nselected_profile=@ByteArray(Default)\r\n[Settings]\r\nbase_directory=D:\\\\MO2\r\n");
        assert_eq!(unquote(&ini["general"]["selected_profile"]), "Default");
        assert_eq!(unquote(&ini["settings"]["base_directory"]), "D:\\MO2");
    }
}
