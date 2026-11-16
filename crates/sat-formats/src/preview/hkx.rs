//! Havok files for the preview: packfile header, bits, classes, what the file is. @sat:formats.preview.hkx
//!
//! Owns: packfile / tagfile detection, Havok version, pointer size (LE / SE), class names and the kind they make.

use super::{Cur, Kind, Preview};

pub(super) fn read(b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Animation, "HKX");
    if b.starts_with(b"<?xml") || b.starts_with(b"<hkpackfile") || b.starts_with(b"<hktagfile") {
        let mut t = super::text::detect("xml", b).unwrap_or_else(|| Preview::new(Kind::Text, "HKX XML"));
        t.kind = Kind::Animation;
        t.format = "HKX · XML".into();
        return t;
    }
    let mut c = Cur::new(b);
    let (m0, m1) = (c.u32(), c.u32());
    if (m0, m1) == (Some(0xCAB0_0D1E), Some(0xD011_FACE))
        || (m0, m1) == (Some(0x1E0D_B0CA), Some(0xCEFA_11D0))
    {
        p.format = "HKX · tagfile".into();
        p.fact("Вид", "tagfile (Fallout 4 и новее)");
        classes(&mut p, b);
        return p;
    }
    if (m0, m1) != (Some(0x57E0_E057), Some(0x10C0_C010)) {
        p.note("это не packfile Havok");
        return p;
    }
    let _user = c.u32();
    let file_ver = c.u32().unwrap_or(0);
    let layout: Vec<u8> = c.take(4).map(|x| x.to_vec()).unwrap_or_default();
    let ptr = layout.first().copied().unwrap_or(0);
    c.skip(4 * 5);
    let version = c.text(16).unwrap_or_default();
    p.fact("Havok", &version);
    p.fact("Версия файла", file_ver);
    p.fact(
        "Указатели",
        format!(
            "{} бит{}",
            ptr as u32 * 8,
            match ptr {
                4 => " — Skyrim LE",
                8 => " — Skyrim SE / AE",
                _ => "",
            }
        ),
    );
    let kind = classes(&mut p, b);
    p.format = format!(
        "HKX · {version} · {} бит{}",
        ptr as u32 * 8,
        if kind.is_empty() {
            String::new()
        } else {
            format!(" · {kind}")
        }
    );
    p
}

/// Class names of the `__classnames__` section (`[u32][0x09]name\0` entries), and what they make the file.
fn classes(p: &mut Preview, b: &[u8]) -> &'static str {
    let mut names: Vec<String> = Vec::new();
    let mut i = 0;
    while i + 6 < b.len() {
        if b[i + 4] == 0x09 && b[i + 5] == b'h' && b[i + 6] == b'k' {
            let start = i + 5;
            let end = b[start..]
                .iter()
                .take(128)
                .position(|&c| c == 0)
                .map(|e| start + e);
            if let Some(end) = end {
                let n = String::from_utf8_lossy(&b[start..end]).into_owned();
                if n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !names.contains(&n) {
                    names.push(n);
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    if names.is_empty() {
        // tagfiles and stripped packfiles: class names anywhere
        for s in super::printable_strings(b, 6, 4000) {
            if (s.starts_with("hk") || s.starts_with("BS"))
                && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !names.contains(&s)
            {
                names.push(s);
            }
        }
    }
    let has = |n: &str| names.iter().any(|x| x.starts_with(n));
    let kind = if has("hkbBehaviorGraph") {
        "поведение (behavior)"
    } else if has("hkbCharacterData") {
        "персонаж (character)"
    } else if has("hkbProjectData") {
        "проект поведения"
    } else if has("hkaSplineCompressedAnimation")
        || has("hkaInterleavedUncompressedAnimation")
        || has("hkaAnimationBinding")
    {
        "анимация"
    } else if has("hkaSkeleton") && has("hkpRigidBody") {
        "скелет с рэгдоллом"
    } else if has("hkaSkeleton") {
        "скелет"
    } else if has("hkpRigidBody") || has("hkpPhysicsData") {
        "физика"
    } else {
        ""
    };
    if !kind.is_empty() {
        p.fact("Что это", kind);
    }
    // names of animations / events a behavior refers to
    let refs = super::mentioned_paths(b, &[".hkx"]);
    for r in &refs {
        p.link(r);
    }
    p.section("Классы Havok", names);
    p.section("Файлы, которые называет", refs);
    kind
}

#[cfg(test)]
mod tests {
    use super::super::{Kind, inspect};

    #[test]
    fn a_packfile_says_its_havok_version_bits_and_what_it_is() {
        let mut b = Vec::new();
        b.extend_from_slice(&0x57E0_E057u32.to_le_bytes());
        b.extend_from_slice(&0x10C0_C010u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&8u32.to_le_bytes());
        b.extend_from_slice(&[8, 1, 0, 1]);
        b.extend_from_slice(&[0u8; 20]);
        b.extend_from_slice(b"hk_2010.2.0-r1\0\0");
        b.extend_from_slice(&[0u8; 16]);
        for n in ["hkClass", "hkaAnimationContainer", "hkaSplineCompressedAnimation"] {
            b.extend_from_slice(&[1, 2, 3, 4, 0x09]);
            b.extend_from_slice(n.as_bytes());
            b.push(0);
        }
        b.extend_from_slice(b"\0Animations\\walk.hkx\0");
        let p = inspect("a.hkx", &b);
        assert_eq!(p.kind, Kind::Animation);
        assert!(
            p.format.contains("hk_2010.2.0-r1")
                && p.format.contains("64 бит")
                && p.format.contains("анимация"),
            "{}",
            p.format
        );
        assert_eq!(p.links, ["animations/walk.hkx"]);
    }
}
