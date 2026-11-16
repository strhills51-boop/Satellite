//! Game files for the preview: plugins, archives, SEQ, TRI, FaceGen, strings, libraries, fonts. @sat:formats.preview.game
//!
//! Owns: plugin header and record counts as a preview, archive manifests as a folder summary, SEQ quests, TRI morphs, localized strings, PE version and SKSE exports.
//! Not here: the plugin and archive readers themselves (`@sat:formats.plugin`, `@sat:formats.archive`).

use super::{Cur, Kind, Preview};

pub(super) fn plugin(name: &str, b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Plugin, "плагин");
    match crate::plugin::read_plugin_header(b) {
        Ok(h) => {
            let kind = match h.kind(name) {
                crate::plugin::PluginKind::Esm => "ESM",
                crate::plugin::PluginKind::Esl => "ESL",
                crate::plugin::PluginKind::Esp => "ESP",
            };
            p.format = format!(
                "Плагин {kind} · версия {:.2} · мастеров {}",
                h.version,
                h.masters.len()
            );
            p.fact("Вид", kind);
            p.fact("Флаг ESM", if h.master_flag { "да" } else { "нет" });
            p.fact("Флаг ESL (лёгкий)", if h.light_flag { "да" } else { "нет" });
            p.fact(
                "Локализован",
                if h.localized {
                    "да (строки в Strings)"
                } else {
                    "нет"
                },
            );
            p.fact("Версия заголовка", format!("{:.2}", h.version));
            p.fact("Записей (по заголовку)", h.num_records);
            if let Some(a) = h.author.as_deref().filter(|a| !a.is_empty()) {
                p.fact("Автор", a);
            }
            if let Some(d) = h.description.as_deref().filter(|d| !d.is_empty()) {
                p.fact("Описание", d);
            }
            for m in &h.masters {
                p.link(m);
            }
            p.section("Мастера", h.masters.clone());
        }
        Err(e) => {
            p.note(format!("заголовок не читается: {e}"));
            return p;
        }
    }
    match crate::plugin::scan_records(b) {
        Ok(c) => {
            p.fact("Записей", c.total);
            p.fact("Размещённых ссылок (REFR + ACHR)", c.placed_references());
            let mut v: Vec<(&String, &u32)> = c.by_signature.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            p.section(
                "Записи по видам",
                v.iter().map(|(s, n)| format!("{s} ×{n}")).collect(),
            );
        }
        Err(e) => p.note(format!("записи не обойдены: {e}")),
    }
    p
}

pub(super) fn archive(b: &[u8]) -> Preview {
    archive_from(std::io::Cursor::new(b))
}

pub(super) fn archive_from<R: std::io::Read + std::io::Seek>(r: R) -> Preview {
    let mut p = Preview::new(Kind::Archive, "архив");
    match crate::archive::read_archive(r) {
        Ok(m) => {
            let kind = match m.kind {
                crate::ArchiveKind::Bsa { version } => format!("BSA v{version}"),
                crate::ArchiveKind::Ba2General { version } => format!("BA2 GNRL v{version}"),
                crate::ArchiveKind::Ba2Textures { version } => format!("BA2 DX10 v{version}"),
            };
            p.format = format!("{kind} · файлов {}", m.entries.len());
            p.fact("Вид", &kind);
            p.fact("Файлов", m.entries.len());
            let mut ext: std::collections::BTreeMap<String, u32> = Default::default();
            let mut top: std::collections::BTreeMap<String, u32> = Default::default();
            for e in &m.entries {
                *ext.entry(super::ext(&e.path)).or_default() += 1;
                *top.entry(e.path.split('/').next().unwrap_or("").to_string())
                    .or_default() += 1;
            }
            let sorted = |m: std::collections::BTreeMap<String, u32>| {
                let mut v: Vec<(String, u32)> = m.into_iter().collect();
                v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                v.into_iter()
                    .map(|(k, n)| format!("{} ×{n}", if k.is_empty() { "(без)".to_string() } else { k }))
                    .collect()
            };
            p.section("Папки верхнего уровня", sorted(top));
            p.section("Расширения", sorted(ext));
            p.section("Файлы", m.entries.iter().map(|e| e.path.clone()).collect());
        }
        Err(e) => p.note(format!("архив не читается: {e}")),
    }
    p
}

/// SEQ: the quests that start with the game (form ids).
pub(super) fn seq(b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Plugin, "SEQ — квесты при старте");
    let ids: Vec<String> = b
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| format!("{:08X}", u32::from_le_bytes(*c)))
        .collect();
    p.fact("Квестов", ids.len());
    p.fact(
        "Что это",
        "квесты «Start Game Enabled» плагина — без этого файла они не стартуют",
    );
    p.format = format!("SEQ · квестов {}", ids.len());
    p.section("Form ID квестов", ids);
    p
}

/// TRI: FaceGen morphs (`FRTRI003`).
pub(super) fn tri(b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Facegen, "TRI — морфы");
    let mut c = Cur::new(b);
    let magic = c.text(8).unwrap_or_default();
    p.fact("Подпись", &magic);
    if !magic.starts_with("FRTRI") {
        p.note("это не TRI");
        return p;
    }
    let verts = c.u32().unwrap_or(0);
    let tris = c.u32().unwrap_or(0);
    let quads = c.u32().unwrap_or(0);
    c.skip(8);
    let uvs = c.u32().unwrap_or(0);
    let _flags = c.u32();
    let morphs = c.u32().unwrap_or(0);
    let mods = c.u32().unwrap_or(0);
    p.format = format!("TRI · вершин {verts} · морфов {}", morphs + mods);
    p.fact("Вершин", verts);
    p.fact("Треугольников", tris);
    if quads > 0 {
        p.fact("Четырёхугольников", quads);
    }
    p.fact("UV-вершин", uvs);
    p.fact("Морфов", morphs);
    p.fact("Модификаторов", mods);
    // morph names are length-prefixed strings in the data
    let names: Vec<String> = super::printable_strings(b, 3, 400)
        .into_iter()
        .filter(|s| {
            s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ' ')
        })
        .collect();
    p.section("Имена морфов", names);
    p
}

pub(super) fn facegen(e: &str, b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Facegen, format!("FaceGen .{e}"));
    p.fact("Подпись", super::lossy(&b[..b.len().min(8)]));
    super::binary_tail(&mut p, b);
    p
}

/// Localized strings: `.strings` (NUL-terminated), `.dlstrings` / `.ilstrings` (length-prefixed).
pub(super) fn strings(e: &str, b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Strings, format!("Строки .{e}"));
    let mut c = Cur::new(b);
    let (Some(n), Some(size)) = (c.u32(), c.u32()) else {
        p.note("заголовок не читается");
        return p;
    };
    let dir_end = 8 + n as usize * 8;
    let data_at = dir_end;
    if dir_end > b.len() || data_at + size as usize > b.len() + 4 {
        p.note("каталог строк больше файла");
        return p;
    }
    let mut lines = Vec::new();
    for _ in 0..n.min(super::LIST_LIMIT as u32) {
        let (Some(id), Some(off)) = (c.u32(), c.u32()) else {
            break;
        };
        let at = data_at + off as usize;
        let s = if e == "strings" {
            b.get(at..).map(super::lossy).unwrap_or_default()
        } else {
            let mut d = Cur::new(b);
            d.at = at;
            d.u32().and_then(|len| d.text(len as usize)).unwrap_or_default()
        };
        lines.push(format!("{id:08X}  {s}"));
    }
    p.format = format!("Строки .{e} · {n}");
    p.fact("Строк", n);
    p.section("Строки", lines);
    if n as usize > super::LIST_LIMIT
        && let Some(s) = p.sections.last_mut()
    {
        s.more = n as usize - super::LIST_LIMIT;
    }
    p
}

/// DLL / EXE: machine, file version, and whether it is an SKSE plugin (its exports).
pub(super) fn pe(b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Library, "библиотека");
    if b.get(..2) != Some(b"MZ") {
        p.note("это не PE (нет MZ)");
        return p;
    }
    let pe_at = b
        .get(0x3c..0x40)
        .map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]) as usize)
        .unwrap_or(0);
    let mut c = Cur::new(b);
    c.at = pe_at;
    let machine = if c.take(4) == Some(b"PE\0\0") {
        c.u16()
    } else {
        None
    };
    let arch = match machine {
        Some(0x8664) => "x64",
        Some(0x14c) => "x86",
        Some(0xAA64) => "ARM64",
        _ => "?",
    };
    p.fact("Архитектура", arch);
    let ver = crate::pe::file_version(b);
    if let Some((a, b2, c2, d)) = ver {
        p.fact("Версия файла", format!("{a}.{b2}.{c2}.{d}"));
    }
    let has = |s: &[u8]| b.windows(s.len()).any(|w| w == s);
    let skse = has(b"SKSEPlugin_Load");
    let what = if skse {
        if has(b"SKSEPlugin_Version") {
            "плагин SKSE (AE: версия в данных)"
        } else {
            "плагин SKSE (SE: SKSEPlugin_Query)"
        }
    } else if has(b"F4SEPlugin_Load") {
        "плагин F4SE"
    } else {
        ""
    };
    if !what.is_empty() {
        p.fact("Что это", what);
    }
    if has(b"CommonLibSSE") || has(b"commonlibsse") {
        p.fact("Собран с", "CommonLibSSE");
    }
    p.format = format!(
        "{} · {arch}{}{}",
        if skse { "SKSE-плагин" } else { "PE" },
        ver.map(|(a, b, c, d)| format!(" · {a}.{b}.{c}.{d}"))
            .unwrap_or_default(),
        if what.is_empty() || skse {
            String::new()
        } else {
            format!(" · {what}")
        }
    );
    super::binary_tail(&mut p, b);
    p
}

pub(super) fn font(e: &str, b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Binary, format!("Шрифт .{e}"));
    let sig = b.get(..4).unwrap_or(&[]);
    p.fact(
        "Вид",
        match sig {
            [0, 1, 0, 0] | b"true" => "TrueType",
            b"OTTO" => "OpenType (CFF)",
            _ => "другой",
        },
    );
    super::binary_tail(&mut p, b);
    p
}

#[cfg(test)]
mod tests {
    use super::super::{Kind, inspect};

    #[test]
    fn seq_strings_and_libraries() {
        let p = inspect("a.seq", &[0x01, 0x02, 0x00, 0x00, 0xAA, 0, 0, 0x01]);
        assert_eq!(p.sections[0].lines, ["00000201", "010000AA"]);
        // .strings: 1 string, id 7 at offset 0
        let mut b = Vec::new();
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(&7u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"Hi!\0");
        let p = inspect("skyrim_english.strings", &b);
        assert_eq!(p.kind, Kind::Strings);
        assert_eq!(p.sections[0].lines, ["00000007  Hi!"]);
        let mut d = b"MZ".to_vec();
        d.resize(0x40, 0);
        d[0x3c] = 0x40;
        d.extend_from_slice(b"PE\0\0");
        d.extend_from_slice(&0x8664u16.to_le_bytes());
        d.extend_from_slice(b"..SKSEPlugin_Load..SKSEPlugin_Version..");
        let p = inspect("p.dll", &d);
        assert!(p.format.starts_with("SKSE-плагин · x64"), "{}", p.format);
    }
}
