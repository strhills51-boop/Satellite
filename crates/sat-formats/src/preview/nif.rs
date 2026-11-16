//! NIF meshes for the preview: header, Bethesda stream, block types, names, texture sets. @sat:formats.preview.nif
//!
//! Owns: reading the NIF header and block table (`Layout`, shared with the geometry reader), texture-set blocks,
//! files a mesh names.
//! Not here: geometry for the 3D view (`@sat:formats.preview.mesh`).

use super::{Cur, Kind, Preview};

fn game_of(bs: u32, user: u32) -> &'static str {
    match bs {
        0 if user == 0 => "Gamebryo",
        1..=34 => "Fallout 3 / New Vegas / Oblivion",
        35..=83 => "Skyrim LE",
        84..=100 => "Skyrim SE / AE",
        101..=139 => "Fallout 4",
        140..=159 => "Fallout 76",
        160..=200 => "Starfield",
        _ => "неизвестная",
    }
}

fn sized(c: &mut Cur<'_>) -> Option<String> {
    let n = c.u32()? as usize;
    if n > 4096 {
        return None;
    }
    c.text(n)
}

fn export(c: &mut Cur<'_>) -> Option<String> {
    let n = c.u8()? as usize;
    c.text(n)
}

pub(super) fn read(b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Mesh, "NIF");
    let Some(nl) = b.iter().take(128).position(|&x| x == b'\n') else {
        p.note("это не NIF: нет строки заголовка");
        return p;
    };
    let line = String::from_utf8_lossy(&b[..nl]).into_owned();
    p.fact("Заголовок", &line);
    let mut c = Cur::new(b);
    c.at = nl + 1;
    match parse(&mut c, &mut p) {
        Some(l) => {
            if let Some(m) = super::mesh::read(b, &l) {
                p.mesh = Some(m);
            }
        }
        None => p.note("заголовок прочитан не полностью"),
    }
    // textures and other files named anywhere in the data (effect shaders, attachments)
    for l in super::mentioned_paths(b, &[".dds", ".tga", ".nif", ".hkx", ".bgsm", ".bgem"]) {
        p.link(&l);
    }
    if !p.links.is_empty() {
        let ls = p.links.clone();
        p.section("Файлы, которые меш называет", ls);
    }
    p
}

/// The block table of a NIF: what the geometry reader needs to walk blocks.
pub(super) struct Layout {
    pub bs: u32,
    pub types: Vec<String>,
    /// Block → its type's index.
    pub idx: Vec<usize>,
    /// Block → its size (20.2.0.5 and later).
    pub sizes: Vec<usize>,
    pub strings: Vec<String>,
    /// Where block 0 starts.
    pub data_at: usize,
}

fn parse(c: &mut Cur<'_>, p: &mut Preview) -> Option<Layout> {
    let ver = c.u32()?;
    let verstr = format!(
        "{}.{}.{}.{}",
        ver >> 24,
        (ver >> 16) & 0xff,
        (ver >> 8) & 0xff,
        ver & 0xff
    );
    p.fact("Версия", &verstr);
    if ver >= 0x14000003 {
        c.u8()?; // endian
    }
    let user = if ver >= 0x0A000108 { c.u32()? } else { 0 };
    let blocks = c.u32()?;
    let mut bs = 0;
    if user >= 3 || ver == 0x14020007 || ver == 0x14000005 || (ver == 0x14000004 && user >= 10) {
        bs = c.u32()?;
        let author = export(c)?;
        if bs > 130 {
            c.u32()?;
        }
        let process = if bs < 131 { export(c)? } else { String::new() };
        let exporter = export(c)?;
        if bs >= 103 {
            export(c)?;
        }
        if !author.is_empty() {
            p.fact("Автор", author);
        }
        if !process.is_empty() {
            p.fact("Скрипт обработки", process);
        }
        if !exporter.is_empty() {
            p.fact("Экспорт", exporter);
        }
    }
    p.fact("Bethesda-версия", format!("{user} / {bs}"));
    p.fact("Игра", game_of(bs, user));
    p.fact("Блоков", blocks);
    p.format = format!("NIF {verstr} · {} · блоков {blocks}", game_of(bs, user));
    if ver < 0x0A000000 {
        return None;
    }
    let ntypes = c.u16()? as usize;
    let mut types = Vec::with_capacity(ntypes.min(2048));
    for _ in 0..ntypes {
        types.push(sized(c)?);
    }
    let mut idx = Vec::with_capacity((blocks as usize).min(1 << 20));
    for _ in 0..blocks {
        idx.push((c.u16()? & 0x7fff) as usize);
    }
    let mut sizes = Vec::new();
    if ver >= 0x14020005 {
        for _ in 0..blocks {
            sizes.push(c.u32()? as usize);
        }
    }
    let mut strings = Vec::new();
    if ver >= 0x14010001 {
        let n = c.u32()?;
        c.u32()?; // max length
        for _ in 0..n.min(1 << 16) {
            strings.push(sized(c)?);
        }
        let groups = c.u32()?;
        c.skip(groups as usize * 4)?;
    }
    // block types by count
    let mut counts: Vec<(String, u32)> = Vec::new();
    for &i in &idx {
        let t = types.get(i).cloned().unwrap_or_else(|| "?".into());
        match counts.iter_mut().find(|x| x.0 == t) {
            Some(x) => x.1 += 1,
            None => counts.push((t, 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let shapes: u32 = counts
        .iter()
        .filter(|(t, _)| t.contains("TriShape") || t.contains("TriStrips") || t.contains("Geometry"))
        .map(|x| x.1)
        .sum();
    p.fact("Фигур (геометрия)", shapes);
    if counts.iter().any(|(t, _)| t.contains("Skin")) {
        p.fact("Скиннинг", "да");
    }
    if counts
        .iter()
        .any(|(t, _)| t.contains("Controller") || t.contains("Interpolator"))
    {
        p.fact("Анимация в меше", "да");
    }
    if counts.iter().any(|(t, _)| t.starts_with("bhk")) {
        p.fact("Коллизия", "да");
    }
    p.section("Блоки", counts.iter().map(|(t, n)| format!("{t} ×{n}")).collect());
    p.section(
        "Имена (узлы, кости)",
        strings.iter().filter(|s| !s.is_empty()).cloned().collect(),
    );
    // texture sets: num textures, then sized strings
    if !sizes.is_empty() {
        let mut at = c.at;
        let mut sets = Vec::new();
        for (k, &ti) in idx.iter().enumerate() {
            let size = sizes[k];
            if types.get(ti).map(String::as_str) == Some("BSShaderTextureSet") {
                let mut t = Cur::new(c.b);
                t.at = at;
                if let Some(n) = t.u32() {
                    let mut tex = Vec::new();
                    for _ in 0..n.min(16) {
                        match sized(&mut t) {
                            Some(s) if !s.is_empty() => tex.push(s),
                            Some(_) => {}
                            None => break,
                        }
                    }
                    if !tex.is_empty() {
                        sets.push(tex.join("  ·  "));
                    }
                }
            }
            at = at.checked_add(size)?;
        }
        p.section("Наборы текстур", sets);
    }
    Some(Layout {
        bs,
        types,
        idx,
        sizes,
        strings,
        data_at: c.at,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{Kind, inspect};

    fn sized(b: &mut Vec<u8>, s: &str) {
        b.extend_from_slice(&(s.len() as u32).to_le_bytes());
        b.extend_from_slice(s.as_bytes());
    }

    /// A minimal SSE NIF: header, two block types, a texture set block.
    pub(crate) fn nif() -> Vec<u8> {
        let mut b = b"Gamebryo File Format, Version 20.2.0.7\n".to_vec();
        b.extend_from_slice(&0x14020007u32.to_le_bytes());
        b.push(1);
        b.extend_from_slice(&12u32.to_le_bytes()); // user version
        b.extend_from_slice(&2u32.to_le_bytes()); // blocks
        b.extend_from_slice(&100u32.to_le_bytes()); // bs version
        for s in ["me", "", "exp"] {
            b.push(s.len() as u8);
            b.extend_from_slice(s.as_bytes());
        }
        b.push(0); // max filepath (bs >= 103? no: 100) — not read
        b.pop();
        b.extend_from_slice(&2u16.to_le_bytes());
        sized(&mut b, "BSTriShape");
        sized(&mut b, "BSShaderTextureSet");
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        let mut set = Vec::new();
        set.extend_from_slice(&2u32.to_le_bytes());
        sized(&mut set, "textures\\armor\\a.dds");
        sized(&mut set, "textures\\armor\\a_n.dds");
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(&(set.len() as u32).to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes()); // strings
        b.extend_from_slice(&8u32.to_le_bytes());
        sized(&mut b, "NPC Root");
        b.extend_from_slice(&0u32.to_le_bytes()); // groups
        b.extend_from_slice(&[0, 0, 0, 0]); // the shape block
        b.extend_from_slice(&set);
        b
    }

    #[test]
    fn a_mesh_says_its_game_blocks_and_textures() {
        let p = inspect("m.nif", &nif());
        assert_eq!(p.kind, Kind::Mesh);
        assert!(p.format.contains("Skyrim SE"), "{}", p.format);
        assert!(p.notes.is_empty(), "{:?}", p.notes);
        let blocks = p.sections.iter().find(|s| s.title == "Блоки").unwrap();
        assert_eq!(blocks.lines, ["BSShaderTextureSet ×1", "BSTriShape ×1"]);
        let sets = p.sections.iter().find(|s| s.title == "Наборы текстур").unwrap();
        assert_eq!(
            sets.lines,
            ["textures\\armor\\a.dds  ·  textures\\armor\\a_n.dds"]
        );
        assert_eq!(p.links, ["textures/armor/a.dds", "textures/armor/a_n.dds"]);
    }
}
