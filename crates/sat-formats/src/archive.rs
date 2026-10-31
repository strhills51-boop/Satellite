//! BSA (v103/104/105) and BA2 (GNRL/DX10, v1/2/3/7/8) manifests. @sat:formats.archive
//!
//! Only the file table is read: paths, sizes and — for BA2 DX10 — texture dimensions,
//! which give exact VRAM without decompressing anything.
//!
//! Owns: BSA/BA2 manifest reading.

use std::io::{BufReader, Read, Seek, SeekFrom};

use serde::{Deserialize, Serialize};

use crate::read::{read_exact_vec, u16le, u32le, u64le, zstring};
use crate::{FormatError, Result, dds, normalize_path};

const MAX_FILES: u32 = 1 << 24;
const MAX_FOLDERS: u32 = 1 << 22;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArchiveKind {
    Bsa { version: u32 },
    Ba2General { version: u32 },
    Ba2Textures { version: u32 },
}

/// One file inside an archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveEntry {
    /// Normalized: lowercase, forward slashes.
    pub path: String,
    /// Stored (possibly compressed) or unpacked size, whichever the format exposes.
    pub size: u64,
    /// Exact GPU bytes when the format carries texture metadata (BA2 DX10); 0 otherwise.
    pub vram: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveManifest {
    pub kind: ArchiveKind,
    pub entries: Vec<ArchiveEntry>,
}

/// Reads an archive manifest, dispatching on the magic bytes.
pub fn read_archive<R: Read + Seek>(r: R) -> Result<ArchiveManifest> {
    let mut r = BufReader::with_capacity(1 << 20, r);
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    r.seek(SeekFrom::Start(0))?;
    match &magic {
        b"BSA\0" => read_bsa(r),
        b"BTDX" => read_ba2(r),
        _ => Err(FormatError::Signature {
            found: magic,
            expected: "BSA\\0 or BTDX",
        }),
    }
}

fn read_bsa<R: Read + Seek>(mut r: R) -> Result<ArchiveManifest> {
    let h = read_exact_vec(&mut r, 36)?;
    let version = u32le(&h, 4);
    let flags = u32le(&h, 12);
    let folders = u32le(&h, 16);
    let files = u32le(&h, 20);
    let file_names_len = u32le(&h, 28) as usize;
    if !matches!(version, 103..=105) {
        return Err(FormatError::Version(version));
    }
    if folders > MAX_FOLDERS || files > MAX_FILES {
        return Err(FormatError::Corrupt("BSA: implausible counts"));
    }
    let rec_size = if version == 105 { 24 } else { 16 };
    let mut counts = Vec::with_capacity(folders as usize);
    let recs = read_exact_vec(&mut r, rec_size * folders as usize)?;
    for i in 0..folders as usize {
        counts.push(u32le(&recs, i * rec_size + 8) as usize);
    }
    let has_dir_names = flags & 0x1 != 0;
    let has_file_names = flags & 0x2 != 0;
    let mut pending: Vec<(usize, u64)> = Vec::with_capacity(files as usize); // (dir index, size)
    let mut dirs: Vec<String> = Vec::with_capacity(folders as usize);
    for (fi, &count) in counts.iter().enumerate() {
        if has_dir_names {
            let mut len = [0u8; 1];
            r.read_exact(&mut len)?;
            dirs.push(zstring(&read_exact_vec(&mut r, len[0] as usize)?));
        } else {
            dirs.push(String::new());
        }
        let block = read_exact_vec(&mut r, 16 * count)?;
        for j in 0..count {
            let size = u32le(&block, j * 16 + 8) & 0x3FFF_FFFF;
            pending.push((fi, u64::from(size)));
        }
    }
    if pending.len() != files as usize {
        return Err(FormatError::Corrupt(
            "BSA: folder counts disagree with file count",
        ));
    }
    let mut entries = Vec::with_capacity(pending.len());
    if has_file_names {
        let names = read_exact_vec(&mut r, file_names_len)?;
        let mut it = names.split(|&b| b == 0);
        for (dir, size) in pending {
            let name = it.next().ok_or(FormatError::Corrupt("BSA: missing file names"))?;
            let name = zstring(name);
            let d = &dirs[dir];
            let path = if d.is_empty() || d == "." {
                name
            } else {
                format!("{d}/{name}")
            };
            entries.push(ArchiveEntry {
                path: normalize_path(&path),
                size,
                vram: 0,
            });
        }
    }
    Ok(ArchiveManifest {
        kind: ArchiveKind::Bsa { version },
        entries,
    })
}

fn read_ba2<R: Read + Seek>(mut r: R) -> Result<ArchiveManifest> {
    let h = read_exact_vec(&mut r, 24)?;
    let version = u32le(&h, 4);
    let typ: [u8; 4] = h[8..12].try_into().expect("4 bytes");
    let count = u32le(&h, 12);
    let name_table = u64le(&h, 16);
    if count > MAX_FILES {
        return Err(FormatError::Corrupt("BA2: implausible file count"));
    }
    let extra = match version {
        1 | 7 | 8 => 0,
        2 => 8,
        3 => 12,
        v => return Err(FormatError::Version(v)),
    };
    if extra > 0 {
        read_exact_vec(&mut r, extra)?;
    }
    let n = count as usize;
    let mut entries: Vec<ArchiveEntry> = Vec::with_capacity(n);
    let kind = match &typ {
        b"GNRL" => {
            let recs = read_exact_vec(&mut r, 36 * n)?;
            for i in 0..n {
                let packed = u32le(&recs, i * 36 + 24);
                let full = u32le(&recs, i * 36 + 28);
                let size = if full != 0 { full } else { packed };
                entries.push(ArchiveEntry {
                    path: String::new(),
                    size: u64::from(size),
                    vram: 0,
                });
            }
            ArchiveKind::Ba2General { version }
        }
        b"DX10" => {
            for _ in 0..n {
                let rec = read_exact_vec(&mut r, 24)?;
                let chunks = rec[13] as usize;
                let height = u32::from(u16le(&rec, 16));
                let width = u32::from(u16le(&rec, 18));
                let mips = u32::from(rec[20]);
                let format = u32::from(rec[21]);
                let cube = rec[22] == 1;
                let chunk_bytes = read_exact_vec(&mut r, 24 * chunks)?;
                let full: u64 = (0..chunks)
                    .map(|c| u64::from(u32le(&chunk_bytes, c * 24 + 12)))
                    .sum();
                let vram = dds::texture_bytes(width, height, 1, mips, format, if cube { 6 } else { 1 });
                entries.push(ArchiveEntry {
                    path: String::new(),
                    size: full,
                    vram,
                });
            }
            ArchiveKind::Ba2Textures { version }
        }
        _ => {
            return Err(FormatError::Signature {
                found: typ,
                expected: "GNRL or DX10",
            });
        }
    };
    r.seek(SeekFrom::Start(name_table))?;
    for e in &mut entries {
        let mut len = [0u8; 2];
        r.read_exact(&mut len)?;
        let raw = read_exact_vec(&mut r, u16::from_le_bytes(len) as usize)?;
        e.path = normalize_path(&zstring(&raw));
    }
    Ok(ArchiveManifest { kind, entries })
}

/// Test/fixture writers. Public so other crates (scanner tests, synthetic generators) can build real archives.
pub mod fixtures {
    /// Writes a minimal valid BSA v105 manifest (file data is not stored).
    pub fn bsa_v105(folders: &[(&str, &[&str])]) -> Vec<u8> {
        let files: usize = folders.iter().map(|f| f.1.len()).sum();
        let dir_len: usize = folders.iter().map(|f| f.0.len() + 1).sum();
        let name_len: usize = folders.iter().flat_map(|f| f.1.iter()).map(|n| n.len() + 1).sum();
        let mut out = Vec::new();
        out.extend_from_slice(b"BSA\0");
        for v in [
            105u32,
            36,
            0x3,
            folders.len() as u32,
            files as u32,
            dir_len as u32,
            name_len as u32,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&[0; 4]);
        for (_, names) in folders {
            let mut rec = [0u8; 24];
            rec[8..12].copy_from_slice(&(names.len() as u32).to_le_bytes());
            out.extend_from_slice(&rec);
        }
        for (dir, names) in folders {
            out.push((dir.len() + 1) as u8);
            out.extend_from_slice(dir.as_bytes());
            out.push(0);
            for _ in *names {
                let mut fr = [0u8; 16];
                fr[8..12].copy_from_slice(&1234u32.to_le_bytes());
                out.extend_from_slice(&fr);
            }
        }
        for (_, names) in folders {
            for n in *names {
                out.extend_from_slice(n.as_bytes());
                out.push(0);
            }
        }
        out
    }

    /// Writes a BA2 v1 DX10 manifest with one chunk per texture: `(path, width, height, mips, dxgi)`.
    pub fn ba2_dx10(textures: &[(&str, u16, u16, u8, u8)]) -> Vec<u8> {
        let mut recs = Vec::new();
        for &(_, w, h, mips, fmt) in textures {
            let mut rec = [0u8; 24];
            rec[13] = 1;
            rec[14..16].copy_from_slice(&24u16.to_le_bytes());
            rec[16..18].copy_from_slice(&h.to_le_bytes());
            rec[18..20].copy_from_slice(&w.to_le_bytes());
            rec[20] = mips;
            rec[21] = fmt;
            recs.extend_from_slice(&rec);
            let mut chunk = [0u8; 24];
            chunk[12..16].copy_from_slice(&4096u32.to_le_bytes());
            recs.extend_from_slice(&chunk);
        }
        let name_table = 24 + recs.len() as u64;
        let mut out = Vec::new();
        out.extend_from_slice(b"BTDX");
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(b"DX10");
        out.extend_from_slice(&(textures.len() as u32).to_le_bytes());
        out.extend_from_slice(&name_table.to_le_bytes());
        out.extend(recs);
        for &(p, ..) in textures {
            out.extend_from_slice(&(p.len() as u16).to_le_bytes());
            out.extend_from_slice(p.as_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn bsa_paths_are_normalized() {
        let bytes = fixtures::bsa_v105(&[
            ("textures\\Armor\\Steel", &["Cuirass.dds", "cuirass_n.dds"]),
            ("meshes", &["a.nif"]),
        ]);
        let m = read_archive(Cursor::new(bytes)).unwrap();
        assert_eq!(m.kind, ArchiveKind::Bsa { version: 105 });
        let paths: Vec<_> = m.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "textures/armor/steel/cuirass.dds",
                "textures/armor/steel/cuirass_n.dds",
                "meshes/a.nif"
            ]
        );
        assert!(m.entries.iter().all(|e| e.size == 1234));
    }

    #[test]
    fn ba2_dx10_gives_exact_vram() {
        let bytes = fixtures::ba2_dx10(&[("Textures\\Sky\\Moon.dds", 512, 512, 1, 77)]);
        let m = read_archive(Cursor::new(bytes)).unwrap();
        assert_eq!(m.kind, ArchiveKind::Ba2Textures { version: 1 });
        assert_eq!(m.entries[0].path, "textures/sky/moon.dds");
        assert_eq!(m.entries[0].vram, 512 * 512); // BC3 = 1 byte/pixel, one mip
        assert_eq!(m.entries[0].size, 4096);
    }

    #[test]
    fn unknown_magic() {
        assert!(read_archive(Cursor::new(b"ZIP!....".to_vec())).is_err());
    }

    #[test]
    fn truncated_bsa_is_error_not_panic() {
        let mut bytes = fixtures::bsa_v105(&[("a", &["b.dds"])]);
        bytes.truncate(60);
        assert!(read_archive(Cursor::new(bytes)).is_err());
    }
}
