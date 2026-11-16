//! Game saves for the preview: who, where, when, the screenshot, the plugins the save needs. @sat:formats.preview.save
//!
//! Owns: reading the header of Skyrim LE / SE saves (`.ess`) and Fallout 4 saves (`.fos`), the screenshot, the
//! plugin list (in SE the body is LZ4 or zlib compressed: only its head is unpacked).
//! Not here: the change forms (the save's game state is not read).

use std::io::Read;

use super::{Cur, Kind, Preview};

fn wstr(c: &mut Cur<'_>) -> Option<String> {
    let n = c.u16()? as usize;
    c.text(n)
}

/// FILETIME → `YYYY-MM-DD hh:mm` (UTC).
fn filetime(ft: u64) -> Option<String> {
    let unix = (ft / 10_000_000).checked_sub(11_644_473_600)?;
    Some(super::pex::date(unix))
}

pub(super) fn read(b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Save, "сохранение");
    let (fo4, magic) = if b.starts_with(b"TESV_SAVEGAME") {
        (false, 13)
    } else if b.starts_with(b"FO4_SAVEGAME") {
        (true, 12)
    } else {
        p.note("это не сохранение Skyrim / Fallout 4");
        return p;
    };
    let mut c = Cur::new(b);
    c.at = magic;
    if header(&mut c, &mut p, fo4).is_none() {
        p.note("сохранение прочитано не полностью");
    }
    p
}

fn header(c: &mut Cur<'_>, p: &mut Preview, fo4: bool) -> Option<()> {
    let hsize = c.u32()? as usize;
    let body_at = c.at.checked_add(hsize)?;
    let version = c.u32()?;
    let number = c.u32()?;
    let name = wstr(c)?;
    let level = c.u32()?;
    let location = wstr(c)?;
    let date = wstr(c)?;
    let race = wstr(c)?;
    let sex = c.u16()?;
    c.u32()?;
    c.u32()?;
    let ft = c.u64()?;
    let (w, h) = (c.u32()?, c.u32()?);
    let compression = if !fo4 && version >= 12 { c.u16()? } else { 0 };
    let game = match (fo4, version) {
        (true, _) => "Fallout 4",
        (false, v) if v >= 12 => "Skyrim SE / AE",
        _ => "Skyrim LE",
    };
    p.format = format!("Сохранение {game} · {name}, ур. {level} · {location}");
    p.fact("Игра", game);
    p.fact("Версия сохранения", version);
    p.fact("Номер", number);
    p.fact("Персонаж", &name);
    p.fact("Уровень", level);
    p.fact("Раса", race);
    p.fact(
        "Пол",
        if sex == 1 {
            "женский"
        } else {
            "мужской"
        },
    );
    p.fact("Место", &location);
    p.fact("Время в игре", date);
    if let Some(t) = filetime(ft) {
        p.fact("Сохранено", t);
    }
    // the screenshot: RGB in LE, RGBA in SE and Fallout 4
    c.at = body_at;
    let bpp = if fo4 || version >= 11 { 4 } else { 3 };
    let shot = (w as usize).checked_mul(h as usize)?.checked_mul(bpp)?;
    if w > 0 && h > 0 && w <= 4096 && h <= 4096 {
        let px = c.take(shot)?;
        let rgba: Vec<u8> = if bpp == 4 {
            px.as_chunks::<4>()
                .0
                .iter()
                .flat_map(|q| [q[0], q[1], q[2], 255])
                .collect()
        } else {
            px.as_chunks::<3>()
                .0
                .iter()
                .flat_map(|q| [q[0], q[1], q[2], 255])
                .collect()
        };
        super::image::show(p, w, h, rgba, String::new());
    } else {
        c.skip(shot)?;
    }
    // the body: SE compresses it
    let body: Vec<u8> = if !fo4 && version >= 12 && compression != 0 {
        let raw = c.u32()? as usize;
        let packed = c.u32()? as usize;
        let data = c.take(packed.min(c.b.len() - c.at))?;
        let want = raw.min(1 << 20);
        let out = match compression {
            2 => lz4_head(data, raw, want),
            1 => {
                let mut v = Vec::new();
                flate2::read::ZlibDecoder::new(data)
                    .take(want as u64)
                    .read_to_end(&mut v)
                    .ok()?;
                Some(v)
            }
            _ => None,
        };
        p.fact("Сжатие", if compression == 2 { "LZ4" } else { "zlib" });
        out?
    } else {
        c.b[c.at..].to_vec()
    };
    plugins(&body, p, fo4)
}

/// The head of an LZ4 block (the whole block is unpacked: lz4 blocks do not stream; capped by the raw size).
fn lz4_head(data: &[u8], raw: usize, want: usize) -> Option<Vec<u8>> {
    if raw > 512 << 20 {
        return None;
    }
    let mut v = lz4_flex::block::decompress(data, raw).ok()?;
    v.truncate(want);
    Some(v)
}

fn plugins(body: &[u8], p: &mut Preview, fo4: bool) -> Option<()> {
    let mut c = Cur::new(body);
    let form = c.u8()?;
    if fo4 {
        let gv = wstr(&mut c)?;
        p.fact("Версия игры", gv);
    }
    c.u32()?;
    let n = c.u8()?;
    let mut list = Vec::new();
    for _ in 0..n {
        list.push(wstr(&mut c)?);
    }
    let light_from = if fo4 { 68 } else { 78 };
    let mut light = Vec::new();
    if form >= light_from {
        let n = c.u16()?;
        for _ in 0..n.min(4096) {
            light.push(wstr(&mut c)?);
        }
    }
    p.fact("Плагинов", format!("{} + лёгких {}", list.len(), light.len()));
    for l in list.iter().chain(&light) {
        p.link(l);
    }
    p.section("Плагины", list);
    p.section("Лёгкие плагины", light);
    Some(())
}

#[cfg(test)]
mod tests {
    use super::super::{Kind, inspect};

    fn w(b: &mut Vec<u8>, s: &str) {
        b.extend_from_slice(&(s.len() as u16).to_le_bytes());
        b.extend_from_slice(s.as_bytes());
    }

    #[test]
    fn a_skyrim_se_save_says_who_where_and_which_plugins() {
        let mut h = Vec::new();
        h.extend_from_slice(&12u32.to_le_bytes());
        h.extend_from_slice(&7u32.to_le_bytes());
        w(&mut h, "Lydia");
        h.extend_from_slice(&30u32.to_le_bytes());
        w(&mut h, "Whiterun");
        w(&mut h, "4E 201");
        w(&mut h, "NordRace");
        h.extend_from_slice(&1u16.to_le_bytes());
        h.extend_from_slice(&[0; 8]);
        h.extend_from_slice(&133_000_000_000_000_000u64.to_le_bytes());
        h.extend_from_slice(&2u32.to_le_bytes());
        h.extend_from_slice(&1u32.to_le_bytes());
        h.extend_from_slice(&2u16.to_le_bytes());
        let mut body = vec![78u8];
        body.extend_from_slice(&0u32.to_le_bytes());
        body.push(2);
        w(&mut body, "Skyrim.esm");
        w(&mut body, "Update.esm");
        body.extend_from_slice(&1u16.to_le_bytes());
        w(&mut body, "light.esl");
        let packed = lz4_flex::block::compress(&body);
        let mut b = b"TESV_SAVEGAME".to_vec();
        b.extend_from_slice(&(h.len() as u32).to_le_bytes());
        b.extend_from_slice(&h);
        b.extend_from_slice(&[255, 0, 0, 255, 0, 255, 0, 255]);
        b.extend_from_slice(&(body.len() as u32).to_le_bytes());
        b.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        b.extend_from_slice(&packed);
        let p = inspect("Save 7.ess", &b);
        assert_eq!(p.kind, Kind::Save);
        assert!(p.notes.is_empty(), "{:?}", p.notes);
        assert!(
            p.format.contains("Lydia") && p.format.contains("SE"),
            "{}",
            p.format
        );
        assert!(p.picture.is_some());
        assert_eq!(p.links, ["skyrim.esm", "update.esm", "light.esl"]);
    }
}
