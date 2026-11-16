//! Game files for the preview: plugins, archives, materials, SEQ, TRI, FaceGen, strings, libraries, fonts. @sat:formats.preview.game
//!
//! Owns: plugin header and record counts as a preview, archive manifests as a folder summary, SEQ quests, TRI morphs, localized strings, PE version and SKSE exports, BGSM/BGEM
//! materials, the file list of a zip (7z / RAR named only), PDF and PSD headers.
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

/// Fallout 4 / Skyrim SE materials (`.bgsm`, `.bgem`): version and the textures they name.
pub(super) fn material(e: &str, b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Material, format!("Материал .{e}"));
    let mut c = Cur::new(b);
    match c.take(4) {
        Some(m) if m == b"BGSM" || m == b"BGEM" => {
            if let Some(v) = c.u32() {
                p.fact("Версия", v);
                p.format = format!("{} v{v}", String::from_utf8_lossy(m));
            }
        }
        _ => p.note("это не BGSM / BGEM"),
    }
    let tex = super::mentioned_paths(b, &[".dds", ".tga", ".bgsm"]);
    for t in &tex {
        p.link(t);
    }
    p.section("Текстуры", tex);
    p
}

/// Zip, 7z, RAR in a mod folder: the format; a zip's file list from its central directory.
pub(super) fn packed(e: &str, b: &[u8]) -> Preview {
    let fmt = match b.get(..4) {
        Some([b'P', b'K', 3, 4]) => "ZIP",
        Some([b'7', b'z', 0xBC, 0xAF]) => "7z",
        Some(b"Rar!") => "RAR",
        _ => e,
    };
    let mut p = Preview::new(Kind::Archive, format!("{fmt} (упакованный архив)"));
    if fmt == "ZIP" {
        let names = zip_names(b).unwrap_or_default();
        p.fact("Файлов", names.len());
        p.section("Состав", names);
    } else {
        p.note("состав 7z / RAR здесь не читается");
    }
    p
}

fn zip_names(b: &[u8]) -> Option<Vec<String>> {
    let from = b.len().saturating_sub(65_557);
    let eocd = (from..b.len().saturating_sub(21))
        .rev()
        .find(|&i| b[i..i + 4] == [b'P', b'K', 5, 6])?;
    let mut c = Cur::new(b);
    c.at = eocd + 10;
    let total = c.u16()? as usize;
    c.u32()?;
    let mut at = c.u32()? as usize;
    let mut out = Vec::new();
    for _ in 0..total.min(super::LIST_LIMIT) {
        let mut c = Cur::new(b);
        c.at = at;
        if c.take(4)? != [b'P', b'K', 1, 2] {
            break;
        }
        c.skip(24)?;
        let (n, x, m) = (c.u16()? as usize, c.u16()? as usize, c.u16()? as usize);
        c.skip(12)?;
        out.push(c.text(n)?);
        at = c.at + x + m;
    }
    Some(out)
}

/// PDF and PSD: what they are and the size of a picture.
pub(super) fn document(e: &str, b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Document, format!(".{e}"));
    if b.starts_with(b"%PDF-") {
        let v = super::lossy(&b[5..b.len().min(8)]);
        p.format = format!("PDF {v}");
    } else if b.starts_with(b"8BPS") {
        let mut c = Cur::new(b);
        c.at = 14;
        if let (Some(h), Some(w)) = (c.u32be(), c.u32be()) {
            p.format = format!("PSD · {w}×{h}");
        }
    } else {
        p.note("формат не распознан");
    }
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

    #[test]
    fn materials_zips_and_documents_say_what_they_are() {
        let mut m = b"BGSM".to_vec();
        m.extend_from_slice(&2u32.to_le_bytes());
        m.extend_from_slice(b"\x10\0\0\0textures\\a\\b_d.dds\0");
        let p = super::super::inspect("x.bgsm", &m);
        assert_eq!(p.format, "BGSM v2");
        assert_eq!(p.links, ["textures/a/b_d.dds"]);
        // a zip with one file, stored
        let name = b"readme.txt";
        let mut z = vec![b'P', b'K', 3, 4];
        z.extend_from_slice(&[0; 26]);
        let cd = z.len() as u32;
        z.extend_from_slice(&[b'P', b'K', 1, 2]);
        z.extend_from_slice(&[0; 24]);
        z.extend_from_slice(&(name.len() as u16).to_le_bytes());
        z.extend_from_slice(&[0; 4]);
        z.extend_from_slice(&[0; 12]);
        z.extend_from_slice(name);
        z.extend_from_slice(&[b'P', b'K', 5, 6, 0, 0, 0, 0, 1, 0, 1, 0]);
        z.extend_from_slice(&0u32.to_le_bytes());
        z.extend_from_slice(&cd.to_le_bytes());
        z.extend_from_slice(&[0, 0]);
        let p = super::super::inspect("a.zip", &z);
        assert_eq!(p.sections[0].lines, ["readme.txt"]);
        let p = super::super::inspect("a.pdf", b"%PDF-1.7 ...");
        assert_eq!(p.format, "PDF 1.7");
    }
}
