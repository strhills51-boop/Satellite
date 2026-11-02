//! Writing a Skyrim SE archive (BSA v105, stored) and the empty light plugin that makes the game load it. @sat:formats.bsa-write
//!
//! Owns: the BSA v105 layout (header, folder records sorted by hash, folder blocks with file records sorted by hash,
//! file names, data), the Bethesda path hash, the content flags, and a minimal ESL-flagged plugin (`TES4` with
//! `HEDR`) — the game loads `<plugin>.bsa` and `<plugin> - Textures.bsa` next to a plugin of the same name.
//! Not here: reading archives (`@sat:formats.archive`), deciding what to pack (`@sat:mo2.bake`).
//!
//! Files are stored uncompressed (the safe choice: textures in SE archives are best left uncompressed, and DDS are
//! compressed already). Not verified in the game yet — the owner tests with a real build.

use std::io::{Read, Seek, SeekFrom, Write};

/// One archive holds at most this much (the format's offsets are 32-bit; the game's own limit is near 2 GiB).
pub const MAX_ARCHIVE: u64 = 0x7FFF_0000;

// @sat:formats.bsa-write.hash
/// The Bethesda hash of a folder path (`ext` empty) or a file name split into stem and extension (`.dds`).
pub fn bsa_hash(stem: &str, ext: &str) -> u64 {
    let r = stem.as_bytes();
    let mut h1: u32 = 0;
    if !r.is_empty() {
        h1 = u32::from(r[r.len() - 1])
            | if r.len() > 2 {
                u32::from(r[r.len() - 2]) << 8
            } else {
                0
            }
            | (r.len() as u32) << 16
            | u32::from(r[0]) << 24;
    }
    match ext {
        ".kf" => h1 |= 0x80,
        ".nif" => h1 |= 0x8000,
        ".dds" => h1 |= 0x8080,
        ".wav" => h1 |= 0x8000_0000,
        _ => {}
    }
    let mut h2: u32 = 0;
    if r.len() > 3 {
        for &c in &r[1..r.len() - 2] {
            h2 = h2.wrapping_mul(0x1003F).wrapping_add(u32::from(c));
        }
    }
    let mut h3: u32 = 0;
    for &c in ext.as_bytes() {
        h3 = h3.wrapping_mul(0x1003F).wrapping_add(u32::from(c));
    }
    (u64::from(h2.wrapping_add(h3)) << 32) | u64::from(h1)
}

fn file_hash(name: &str) -> u64 {
    match name.rfind('.') {
        Some(i) => bsa_hash(&name[..i], &name[i..]),
        None => bsa_hash(name, ""),
    }
}

/// The archive's content flags from its top folders.
fn content_flags<'a>(folders: impl Iterator<Item = &'a str>) -> u32 {
    let mut f = 0;
    for d in folders {
        let top = d.split('\\').next().unwrap_or("");
        f |= match top {
            "meshes" => 0x1,
            "textures" => 0x2,
            "interface" => 0x4,
            "sound" if d.starts_with("sound\\voice") => 0x10,
            "sound" | "music" => 0x8,
            "shaders" | "shadersfx" => 0x20,
            "trees" => 0x40,
            "fonts" => 0x80,
            _ => 0x100,
        };
    }
    f
}

/// A file to pack: its path inside the archive (any case, `/` or `\`) and where to read it.
#[derive(Debug, Clone)]
pub struct PackFile {
    pub path: String,
    pub source: std::path::PathBuf,
}

/// A folder of the archive: hash, name, its files (hash, name, source), sorted by hash.
type Folder<'a> = (u64, String, Vec<(u64, String, &'a PackFile)>);

// @sat:formats.bsa-write.write
/// Writes a BSA v105 with `files` (stored). Returns the bytes written. Fails on an archive larger than
/// [`MAX_ARCHIVE`] or a path without a folder.
pub fn write_bsa<W: Write + Seek>(mut out: W, files: &[PackFile]) -> std::io::Result<u64> {
    let bad = |m: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, m);
    // folder → files
    let mut folders: std::collections::BTreeMap<String, Vec<(String, &PackFile)>> = Default::default();
    for f in files {
        let p = f.path.replace('/', "\\").to_lowercase();
        let (dir, name) = p
            .rsplit_once('\\')
            .ok_or_else(|| bad(format!("{}: a packed file needs a folder", f.path)))?;
        folders
            .entry(dir.to_string())
            .or_default()
            .push((name.to_string(), f));
    }
    let mut dirs: Vec<Folder<'_>> = folders
        .into_iter()
        .map(|(d, fs)| {
            let mut fs: Vec<(u64, String, &PackFile)> =
                fs.into_iter().map(|(n, f)| (file_hash(&n), n, f)).collect();
            fs.sort_by_key(|x| x.0);
            (bsa_hash(&d, ""), d, fs)
        })
        .collect();
    dirs.sort_by_key(|x| x.0);
    let file_count: usize = dirs.iter().map(|d| d.2.len()).sum();
    let dir_names: usize = dirs.iter().map(|d| d.1.len() + 1).sum();
    let file_names: usize = dirs.iter().flat_map(|d| d.2.iter()).map(|f| f.1.len() + 1).sum();
    let flags = content_flags(dirs.iter().map(|d| d.1.as_str()));
    let header = 36u64;
    let records = 24 * dirs.len() as u64;
    let blocks: u64 = dirs
        .iter()
        .map(|d| 1 + d.1.len() as u64 + 1 + 16 * d.2.len() as u64)
        .sum();
    let mut data_at = header + records + blocks + file_names as u64;
    let mut sizes = Vec::with_capacity(file_count);
    for d in &dirs {
        for (_, _, f) in &d.2 {
            let n = std::fs::metadata(&f.source)?.len();
            if n > 0x3FFF_FFFF {
                return Err(bad(format!("{}: too large for an archive", f.path)));
            }
            sizes.push(n);
        }
    }
    let total = data_at + sizes.iter().sum::<u64>();
    if total > MAX_ARCHIVE {
        return Err(bad(format!("{total} bytes: more than one archive holds")));
    }
    let u32le = |v: u32| v.to_le_bytes();
    out.write_all(b"BSA\0")?;
    for v in [
        105,
        36,
        0x3,
        dirs.len() as u32,
        file_count as u32,
        dir_names as u32,
        file_names as u32,
        flags,
    ] {
        out.write_all(&u32le(v))?;
    }
    // folder records: the offset points at the folder's block, plus the file names' length (the format's quirk)
    let mut block_at = header + records;
    for (h, name, fs) in &dirs {
        out.write_all(&h.to_le_bytes())?;
        out.write_all(&u32le(fs.len() as u32))?;
        out.write_all(&u32le(0))?;
        out.write_all(&(block_at + file_names as u64).to_le_bytes())?;
        block_at += 1 + name.len() as u64 + 1 + 16 * fs.len() as u64;
    }
    let mut i = 0;
    for (_, name, fs) in &dirs {
        out.write_all(&[(name.len() + 1) as u8])?;
        out.write_all(name.as_bytes())?;
        out.write_all(&[0])?;
        for (h, _, _) in fs {
            out.write_all(&h.to_le_bytes())?;
            out.write_all(&u32le(sizes[i] as u32))?;
            out.write_all(&u32le(data_at as u32))?;
            data_at += sizes[i];
            i += 1;
        }
    }
    for (_, _, fs) in &dirs {
        for (_, n, _) in fs {
            out.write_all(n.as_bytes())?;
            out.write_all(&[0])?;
        }
    }
    for (_, _, fs) in &dirs {
        for (_, _, f) in fs {
            std::io::copy(&mut std::fs::File::open(&f.source)?, &mut out)?;
        }
    }
    out.flush()?;
    out.seek(SeekFrom::End(0))
}

// @sat:formats.bsa-write.plugin
/// An empty ESL-flagged plugin (Skyrim SE): it loads the archives named after it and takes no full slot.
pub fn empty_light_plugin(author: &str) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(b"HEDR");
    data.extend_from_slice(&12u16.to_le_bytes());
    data.extend_from_slice(&1.71f32.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&0x800u32.to_le_bytes());
    let cnam: Vec<u8> = author.bytes().chain(std::iter::once(0)).collect();
    data.extend_from_slice(b"CNAM");
    data.extend_from_slice(&(cnam.len() as u16).to_le_bytes());
    data.extend_from_slice(&cnam);
    let mut out = Vec::new();
    out.extend_from_slice(b"TES4");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x200u32.to_le_bytes()); // light (ESL)
    out.extend_from_slice(&0u32.to_le_bytes()); // form id
    out.extend_from_slice(&0u32.to_le_bytes()); // version control
    out.extend_from_slice(&44u16.to_le_bytes()); // form version (SE)
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&data);
    out
}

/// Reads back one file's bytes from a stored v105 archive written here (tests, checks).
pub fn read_stored<R: Read + Seek>(mut r: R, offset: u32, size: u32) -> std::io::Result<Vec<u8>> {
    r.seek(SeekFrom::Start(u64::from(offset)))?;
    let mut b = vec![0; size as usize];
    r.read_exact(&mut b)?;
    Ok(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_follow_the_known_shape() {
        // stem "a" (len 1): last char, length, first char; `.dds` sets 0x8080
        let h = bsa_hash("a", ".dds");
        assert_eq!(h as u32, 0x61 | (1 << 16) | (0x61 << 24) | 0x8080);
        // the extension feeds the upper half
        assert_ne!(bsa_hash("abcdef", ".nif") >> 32, bsa_hash("abcdef", ".dds") >> 32);
        assert_eq!(bsa_hash("meshes", "") as u32 & 0xFF, u32::from(b's'));
    }

    #[test]
    fn an_archive_reads_back_with_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let mk = |p: &str, body: &[u8]| {
            let s = dir.path().join(p.replace('/', "_"));
            std::fs::write(&s, body).unwrap();
            PackFile {
                path: p.into(),
                source: s,
            }
        };
        let files = vec![
            mk("Textures/Costume/a.dds", b"AAAA"),
            mk("meshes/costume/a.nif", b"NIF!"),
            mk("textures/costume/b.dds", b"BB"),
        ];
        let path = dir.path().join("Costumes.bsa");
        let n = write_bsa(std::fs::File::create(&path).unwrap(), &files).unwrap();
        assert_eq!(n, std::fs::metadata(&path).unwrap().len());
        let m = crate::archive::read_archive(std::fs::File::open(&path).unwrap()).unwrap();
        let mut paths: Vec<String> = m.entries.iter().map(|e| e.path.clone()).collect();
        paths.sort();
        assert_eq!(
            paths,
            [
                "meshes/costume/a.nif",
                "textures/costume/a.dds",
                "textures/costume/b.dds"
            ]
        );
        // every folder record points (minus the names' length) at its block; every file record at its data
        let bytes = std::fs::read(&path).unwrap();
        let u32at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        let (folders, names_len) = (u32at(16) as usize, u32at(28) as usize);
        let mut seen = Vec::new();
        for i in 0..folders {
            let rec = 36 + 24 * i;
            let count = u32at(rec + 8) as usize;
            let off = u64::from_le_bytes(bytes[rec + 16..rec + 24].try_into().unwrap()) as usize - names_len;
            let nl = bytes[off] as usize;
            for j in 0..count {
                let fr = off + 1 + nl + 16 * j;
                let (size, at) = (u32at(fr + 8), u32at(fr + 12));
                seen.push(read_stored(std::io::Cursor::new(&bytes), at, size).unwrap());
            }
        }
        seen.sort();
        assert_eq!(seen, [b"AAAA".to_vec(), b"BB".to_vec(), b"NIF!".to_vec()]);
        let p = empty_light_plugin("Satellite");
        let h = crate::plugin::read_plugin_header(&p[..]).unwrap();
        assert!(h.light_flag && !h.master_flag);
    }
}
