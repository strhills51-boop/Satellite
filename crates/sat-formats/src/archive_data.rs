//! Bytes of archive entries, unpacked, for the entries asked for. @sat:formats.archive-data
//!
//! Owns: reading the data of BSA (v103–v105: zlib, LZ4 frame for v105, names embedded before the data) and BA2 GNRL
//! (zlib) entries, so a packed copy of a file can be compared with a loose one byte for byte.
//! Not here: archive manifests (`@sat:formats.archive`), hashing (`@sat:scan.content-hash`).
//!
//! BA2 DX10 entries keep a texture without its DDS header, and BA2 v3 may use LZ4 blocks: those entries are given as
//! `None` — not comparable, never a guess.

use std::io::{Read, Seek, SeekFrom};

use crate::archive::bsa_records;
use crate::{FormatError, Result};

/// Largest entry unpacked (a bigger one is reported as not comparable).
const MAX_ENTRY: u64 = 1 << 31;

fn unzlib(data: &[u8], size: u64) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(size.min(MAX_ENTRY) as usize);
    flate2::read::ZlibDecoder::new(data)
        .take(MAX_ENTRY)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

fn unlz4(data: &[u8], size: u64) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(size.min(MAX_ENTRY) as usize);
    lz4_flex::frame::FrameDecoder::new(data)
        .take(MAX_ENTRY)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

// @sat:formats.archive-data.read
/// Calls `each(path, bytes)` for every entry `wanted(path)` accepts (paths normalized like the manifest's), in the
/// archive's order; `None` — the entry cannot be unpacked for comparison.
pub fn read_entries<R: Read + Seek>(
    mut r: R,
    wanted: impl Fn(&str) -> bool,
    mut each: impl FnMut(&str, Option<Vec<u8>>),
) -> Result<()> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    r.seek(SeekFrom::Start(0))?;
    match &magic {
        b"BSA\0" => bsa(r, wanted, each),
        b"BTDX" => ba2(r, wanted, &mut each),
        _ => Err(FormatError::Signature {
            found: magic,
            expected: "BSA or BTDX",
        }),
    }
}

fn bsa<R: Read + Seek>(
    mut r: R,
    wanted: impl Fn(&str) -> bool,
    mut each: impl FnMut(&str, Option<Vec<u8>>),
) -> Result<()> {
    let (version, flags, records) = bsa_records(&mut r)?;
    let compressed_default = flags & 0x4 != 0;
    let embedded_names = flags & 0x100 != 0 && version >= 104;
    for (path, raw, offset) in records {
        if !wanted(&path) {
            continue;
        }
        let size = u64::from(raw & 0x3FFF_FFFF);
        let compressed = compressed_default != (raw & 0x4000_0000 != 0);
        if size > MAX_ENTRY {
            each(&path, None);
            continue;
        }
        r.seek(SeekFrom::Start(u64::from(offset)))?;
        let mut data = vec![0u8; size as usize];
        r.read_exact(&mut data)?;
        let mut body: &[u8] = &data;
        if embedded_names {
            let n = *body.first().ok_or(FormatError::Corrupt("BSA: empty entry"))? as usize;
            body = body
                .get(1 + n..)
                .ok_or(FormatError::Corrupt("BSA: entry shorter than its name"))?;
        }
        let bytes = if compressed {
            let (orig, packed) = body
                .split_at_checked(4)
                .ok_or(FormatError::Corrupt("BSA: entry without its size"))?;
            let orig = u64::from(u32::from_le_bytes(orig.try_into().expect("4 bytes")));
            if version == 105 {
                unlz4(packed, orig)
            } else {
                unzlib(packed, orig)
            }
        } else {
            Some(body.to_vec())
        };
        each(&path, bytes);
    }
    Ok(())
}

fn ba2<R: Read + Seek>(
    mut r: R,
    wanted: impl Fn(&str) -> bool,
    each: &mut impl FnMut(&str, Option<Vec<u8>>),
) -> Result<()> {
    let mut h = [0u8; 24];
    r.read_exact(&mut h)?;
    let version = u32::from_le_bytes(h[4..8].try_into().expect("4"));
    let general = &h[8..12] == b"GNRL";
    let count = u32::from_le_bytes(h[12..16].try_into().expect("4")) as usize;
    let names_at = u64::from_le_bytes(h[16..24].try_into().expect("8"));
    // v2/v3 carry extra header fields; v3's last one is the compression (0 zlib, 3 LZ4 blocks)
    let lz4 = match version {
        2 => {
            r.seek(SeekFrom::Current(8))?;
            false
        }
        3 => {
            let mut x = [0u8; 12];
            r.read_exact(&mut x)?;
            u32::from_le_bytes(x[8..12].try_into().expect("4")) == 3
        }
        _ => false,
    };
    let mut recs = Vec::new();
    if general {
        let raw = {
            let mut b = vec![0u8; 36 * count];
            r.read_exact(&mut b)?;
            b
        };
        for i in 0..count {
            let at = i * 36;
            let offset = u64::from_le_bytes(raw[at + 16..at + 24].try_into().expect("8"));
            let packed = u32::from_le_bytes(raw[at + 24..at + 28].try_into().expect("4"));
            let full = u32::from_le_bytes(raw[at + 28..at + 32].try_into().expect("4"));
            recs.push((offset, packed, full));
        }
    }
    r.seek(SeekFrom::Start(names_at))?;
    let mut paths = Vec::with_capacity(count);
    for _ in 0..count {
        let mut len = [0u8; 2];
        r.read_exact(&mut len)?;
        let mut name = vec![0u8; u16::from_le_bytes(len) as usize];
        r.read_exact(&mut name)?;
        paths.push(crate::normalize_path(&String::from_utf8_lossy(&name)));
    }
    for (i, path) in paths.iter().enumerate() {
        if !wanted(path) {
            continue;
        }
        let Some(&(offset, packed, full)) = recs.get(i) else {
            each(path, None); // DX10: no DDS header in the archive
            continue;
        };
        let stored = if packed != 0 { packed } else { full };
        if u64::from(stored) > MAX_ENTRY || (packed != 0 && lz4) {
            each(path, None);
            continue;
        }
        r.seek(SeekFrom::Start(offset))?;
        let mut data = vec![0u8; stored as usize];
        r.read_exact(&mut data)?;
        each(
            path,
            if packed != 0 {
                unzlib(&data, u64::from(full))
            } else {
                Some(data)
            },
        );
    }
    Ok(())
}

// @sat:formats.archive-data.one
/// The bytes of one entry for viewing (`@sat:formats.preview`): like [`read_entries`], and a BA2 DX10 texture comes
/// back as a whole DDS file — the header rebuilt from the record (format, size, mips, cubemap), the chunks unpacked
/// in order. Comparison keeps DX10 entries "unknown" ([`read_entries`]): a rebuilt header is not the author's.
pub fn read_one<R: Read + Seek>(mut r: R, path: &str) -> Result<Option<Vec<u8>>> {
    let want = crate::normalize_path(path);
    let mut magic = [0u8; 12];
    r.read_exact(&mut magic)?;
    r.seek(SeekFrom::Start(0))?;
    if &magic[0..4] == b"BTDX" && &magic[8..12] == b"DX10" {
        return ba2_dx10_one(r, &want);
    }
    let mut out = None;
    read_entries(r, |p| p == want, |_, b| out = b)?;
    Ok(out)
}

fn ba2_dx10_one<R: Read + Seek>(mut r: R, want: &str) -> Result<Option<Vec<u8>>> {
    let mut h = [0u8; 24];
    r.read_exact(&mut h)?;
    let version = u32::from_le_bytes(h[4..8].try_into().expect("4"));
    let count = u32::from_le_bytes(h[12..16].try_into().expect("4")) as usize;
    let names_at = u64::from_le_bytes(h[16..24].try_into().expect("8"));
    let lz4 = match version {
        2 => {
            r.seek(SeekFrom::Current(8))?;
            false
        }
        3 => {
            let mut x = [0u8; 12];
            r.read_exact(&mut x)?;
            u32::from_le_bytes(x[8..12].try_into().expect("4")) == 3
        }
        _ => false,
    };
    let records_at = r.stream_position()?;
    // names first: find the index
    r.seek(SeekFrom::Start(names_at))?;
    let mut index = None;
    for i in 0..count {
        let mut len = [0u8; 2];
        r.read_exact(&mut len)?;
        let mut name = vec![0u8; u16::from_le_bytes(len) as usize];
        r.read_exact(&mut name)?;
        if crate::normalize_path(&String::from_utf8_lossy(&name)) == want {
            index = Some(i);
            break;
        }
    }
    let Some(index) = index else { return Ok(None) };
    // records: 24 bytes each, then its chunks (24 bytes each)
    r.seek(SeekFrom::Start(records_at))?;
    let mut rec = [0u8; 24];
    let mut chunks: Vec<(u64, u32, u32)> = Vec::new();
    for i in 0..=index {
        r.read_exact(&mut rec)?;
        let n = rec[13] as usize;
        let mut c = vec![0u8; 24 * n];
        r.read_exact(&mut c)?;
        if i == index {
            for k in 0..n {
                let at = k * 24;
                chunks.push((
                    u64::from_le_bytes(c[at..at + 8].try_into().expect("8")),
                    u32::from_le_bytes(c[at + 8..at + 12].try_into().expect("4")),
                    u32::from_le_bytes(c[at + 12..at + 16].try_into().expect("4")),
                ));
            }
        }
    }
    if lz4 {
        return Ok(None);
    }
    let height = u32::from(u16::from_le_bytes([rec[16], rec[17]]));
    let width = u32::from(u16::from_le_bytes([rec[18], rec[19]]));
    let mips = u32::from(rec[20]);
    let format = u32::from(rec[21]);
    let cube = u16::from_le_bytes([rec[22], rec[23]]) & 1 != 0;
    let mut dds = dds_header(width, height, mips, format, cube);
    for (offset, packed, full) in chunks {
        let stored = if packed != 0 { packed } else { full };
        if u64::from(stored) > MAX_ENTRY {
            return Ok(None);
        }
        r.seek(SeekFrom::Start(offset))?;
        let mut data = vec![0u8; stored as usize];
        r.read_exact(&mut data)?;
        match if packed != 0 {
            unzlib(&data, u64::from(full))
        } else {
            Some(data)
        } {
            Some(d) => dds.extend_from_slice(&d),
            None => return Ok(None),
        }
    }
    Ok(Some(dds))
}

/// A DDS header with the DX10 extension.
fn dds_header(w: u32, h: u32, mips: u32, dxgi: u32, cube: bool) -> Vec<u8> {
    let mut b = vec![0u8; 148];
    let put = |b: &mut [u8], at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    b[0..4].copy_from_slice(b"DDS ");
    put(&mut b, 4, 124);
    put(&mut b, 8, 0x1 | 0x2 | 0x4 | 0x1000 | 0x20000);
    put(&mut b, 12, h);
    put(&mut b, 16, w);
    put(&mut b, 28, mips.max(1));
    put(&mut b, 76, 32);
    put(&mut b, 80, 0x4);
    b[84..88].copy_from_slice(b"DX10");
    put(&mut b, 108, 0x1000 | 0x8 | 0x400000);
    if cube {
        put(&mut b, 112, 0xFE00);
    }
    put(&mut b, 128, dxgi);
    put(&mut b, 132, 3);
    put(&mut b, 136, if cube { 4 } else { 0 });
    put(&mut b, 140, 1);
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    #[test]
    fn one_entry_of_a_bsa_and_a_whole_dds_from_a_ba2_dx10() {
        let b = bsa(105, &[("a.txt", b"hello", false)], false);
        assert_eq!(
            read_one(Cursor::new(&b), "D\\A.TXT").unwrap().as_deref(),
            Some(&b"hello"[..])
        );
        assert_eq!(read_one(Cursor::new(&b), "d/none.txt").unwrap(), None);
        let mut ba2 = crate::archive::fixtures::ba2_dx10(&[("textures/a.dds", 4, 4, 1, 71)]);
        ba2.resize(ba2.len() + 4096, 0); // the fixture's chunk: 4096 bytes at offset 0
        let dds = read_one(Cursor::new(&ba2), "textures/a.dds")
            .unwrap()
            .expect("a texture");
        let info = crate::DdsInfo::parse(&dds).expect("a DDS header");
        assert_eq!(
            (info.width, info.height, info.mips, info.format.as_str()),
            (4, 4, 1, "BC1")
        );
    }

    /// A BSA with one folder `d` and files given as (name, data, compressed), names embedded when `embed`.
    fn bsa(version: u32, files: &[(&str, &[u8], bool)], embed: bool) -> Vec<u8> {
        let flags: u32 = 0x3 | if embed { 0x100 } else { 0 };
        let rec = if version == 105 { 24 } else { 16 };
        let names_len: usize = files.iter().map(|f| f.0.len() + 1).sum();
        let header_end = 36 + rec + 1 + 2 + 16 * files.len() + names_len;
        let mut data = Vec::new();
        let mut file_recs = Vec::new();
        for (name, bytes, packed) in files {
            let start = header_end + data.len();
            if embed {
                let full = format!("d\\{name}");
                data.push(full.len() as u8);
                data.extend_from_slice(full.as_bytes());
            }
            if *packed {
                data.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                if version == 105 {
                    let mut w = lz4_flex::frame::FrameEncoder::new(Vec::new());
                    w.write_all(bytes).unwrap();
                    data.extend(w.finish().unwrap());
                } else {
                    let mut w = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                    w.write_all(bytes).unwrap();
                    data.extend(w.finish().unwrap());
                }
            } else {
                data.extend_from_slice(bytes);
            }
            let size = (header_end + data.len() - start) as u32 | if *packed { 0x4000_0000 } else { 0 };
            file_recs.push((size, start as u32));
        }
        let mut out = b"BSA\0".to_vec();
        for v in [version, 36, flags, 1, files.len() as u32, 2, names_len as u32, 0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let mut folder = vec![0u8; rec];
        folder[8..12].copy_from_slice(&(files.len() as u32).to_le_bytes());
        out.extend(folder);
        out.extend_from_slice(&[2, b'd', 0]);
        for (size, offset) in &file_recs {
            out.extend_from_slice(&[0u8; 8]);
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
        }
        for (name, _, _) in files {
            out.extend_from_slice(name.as_bytes());
            out.push(0);
        }
        assert_eq!(out.len(), header_end);
        out.extend(data);
        out
    }

    fn read(bytes: &[u8], want: &str) -> Vec<(String, Option<Vec<u8>>)> {
        let mut got = Vec::new();
        read_entries(
            Cursor::new(bytes),
            |p| p.ends_with(want),
            |p, b| got.push((p.to_string(), b)),
        )
        .unwrap();
        got
    }

    #[test]
    fn bsa_entries_come_out_as_their_files() {
        let files: &[(&str, &[u8], bool)] = &[
            ("a.dds", b"stored bytes", false),
            ("b.nif", b"packed bytes packed", true),
        ];
        for version in [104, 105] {
            for embed in [false, true] {
                let arc = bsa(version, files, embed);
                let all = read(&arc, "");
                assert_eq!(all.len(), 2, "v{version} embed={embed}");
                assert_eq!(all[0], ("d/a.dds".into(), Some(b"stored bytes".to_vec())));
                assert_eq!(all[1], ("d/b.nif".into(), Some(b"packed bytes packed".to_vec())));
                assert_eq!(read(&arc, "b.nif").len(), 1, "only what is asked for");
            }
        }
    }

    #[test]
    fn dx10_textures_are_not_comparable() {
        let arc = crate::archive::fixtures::ba2_dx10(&[("textures/a.dds", 4, 4, 1, 71)]);
        assert_eq!(read(&arc, ""), [("textures/a.dds".to_string(), None)]);
    }
}
