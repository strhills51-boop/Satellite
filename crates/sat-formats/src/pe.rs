//! The file version of a Windows executable or library (`VS_FIXEDFILEINFO`), and the names a library exports. @sat:formats.pe
//!
//! Owns: finding the fixed version block by its signature and reading the file version (major, minor, build,
//! revision); the export names of a library, read from its headers with a few seeks (`@sat:formats.pe.exports`).
//! Not here: which files to read and what the names mean (`@sat:conflict.kinds.runtime`).
//!
//! The block is found by its signature `0xFEEF04BD` rather than by walking the resource tree: one pass over the
//! bytes, no PE parser, the same answer for the game's executable.
//!
//! The text version (`FileVersion` in the string table, what Windows' file properties show) wins when it is there:
//! the owner's `SkyrimSE.exe` 1.5.97 carries 1.0.0.0 in its fixed block and "1.5.97.0" as text.

// @sat:formats.pe.version
/// The file version: the string table's `FileVersion` when it reads as numbers, else the fixed version block.
pub fn file_version(bytes: &[u8]) -> Option<(u16, u16, u16, u16)> {
    string_version(bytes).or_else(|| fixed_version(bytes))
}

/// `FileVersion` of the string table: the key in UTF-16, then its value from the next 4-byte boundary.
fn string_version(bytes: &[u8]) -> Option<(u16, u16, u16, u16)> {
    let key: Vec<u8> = "FileVersion\0"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let at = bytes.windows(key.len()).position(|w| w == key.as_slice())?;
    let start = (at + key.len()).next_multiple_of(4);
    let units: Vec<u16> = bytes
        .get(start..)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&u| u != 0)
        .take(64)
        .collect();
    let text = String::from_utf16(&units).ok()?;
    let nums: Vec<u16> = text
        .split(['.', ','])
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    match nums[..] {
        [a, b, c, d] => Some((a, b, c, d)),
        [a, b, c] => Some((a, b, c, 0)),
        _ => None,
    }
}

/// The fixed version block's file version.
fn fixed_version(bytes: &[u8]) -> Option<(u16, u16, u16, u16)> {
    const SIG: [u8; 4] = 0xFEEF_04BDu32.to_le_bytes();
    let at = bytes.windows(4).position(|w| w == SIG)?;
    // signature, struct version, then FileVersionMS, FileVersionLS
    let u = |o: usize| -> Option<u32> {
        Some(u32::from_le_bytes(
            bytes.get(at + o..at + o + 4)?.try_into().ok()?,
        ))
    };
    let (ms, ls) = (u(8)?, u(12)?);
    Some(((ms >> 16) as u16, ms as u16, (ls >> 16) as u16, ls as u16))
}

// @sat:formats.pe.exports
/// The names a library exports (PE32 or PE32+), read with a few seeks: the headers, the section table, the export
/// directory and its names. `None` — not a PE file or no export table. A plugin of the script extender is told apart
/// by these names (`SKSEPlugin_Query` for the old loader, `SKSEPlugin_Version` for the new one).
pub fn exports<R: std::io::Read + std::io::Seek>(r: &mut R) -> Option<Vec<String>> {
    use std::io::SeekFrom;
    let mut at = |off: u64, len: usize| -> Option<Vec<u8>> {
        r.seek(SeekFrom::Start(off)).ok()?;
        let mut b = vec![0u8; len];
        let n = r.read(&mut b).ok()?;
        b.truncate(n);
        Some(b)
    };
    let u16_at =
        |b: &[u8], o: usize| -> Option<u16> { Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?)) };
    let u32_at =
        |b: &[u8], o: usize| -> Option<u32> { Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?)) };
    let head = at(0, 4096)?;
    if head.get(0..2)? != b"MZ" {
        return None;
    }
    let pe = u32_at(&head, 0x3C)? as usize;
    let hdr = at(pe as u64, 24 + 240 + 40 * 96)?;
    if hdr.get(0..4)? != b"PE\0\0" {
        return None;
    }
    let sections = u16_at(&hdr, 6)? as usize;
    let opt_size = u16_at(&hdr, 20)? as usize;
    let opt = 24;
    let dirs = match u16_at(&hdr, opt)? {
        0x20b => opt + 112,
        0x10b => opt + 96,
        _ => return None,
    };
    let (exp_rva, exp_size) = (u32_at(&hdr, dirs)?, u32_at(&hdr, dirs + 4)?);
    if exp_rva == 0 || exp_size == 0 {
        return None;
    }
    let table = opt + opt_size;
    let secs: Vec<(u32, u32, u32)> = (0..sections.min(96))
        .filter_map(|i| {
            let s = table + i * 40;
            let (vsize, va, raw, ptr) = (
                u32_at(&hdr, s + 8)?,
                u32_at(&hdr, s + 12)?,
                u32_at(&hdr, s + 16)?,
                u32_at(&hdr, s + 20)?,
            );
            Some((va, vsize.max(raw), ptr))
        })
        .collect();
    let off = |rva: u32| -> Option<u64> {
        secs.iter()
            .find(|(va, size, _)| rva >= *va && rva < va + size)
            .map(|(va, _, ptr)| u64::from(ptr + (rva - va)))
    };
    // the export directory with the data after it (names usually lie inside the directory's range)
    let len = (exp_size as usize).clamp(40, 1 << 24);
    let dir = at(off(exp_rva)?, len)?;
    let count = (u32_at(&dir, 24)? as usize).min(65536);
    let names_rva = u32_at(&dir, 32)?;
    let in_dir = |rva: u32| -> Option<usize> {
        rva.checked_sub(exp_rva)
            .map(|d| d as usize)
            .filter(|&d| d < dir.len())
    };
    let table_bytes = match in_dir(names_rva) {
        Some(d) if d + count * 4 <= dir.len() => dir[d..d + count * 4].to_vec(),
        _ => at(off(names_rva)?, count * 4)?,
    };
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let rva = u32_at(&table_bytes, i * 4)?;
        let raw = match in_dir(rva) {
            Some(d) => dir[d..].to_vec(),
            None => at(off(rva)?, 256)?,
        };
        let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len()).min(256);
        out.push(String::from_utf8_lossy(&raw[..end]).into_owned());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_fixed_version_block_gives_the_file_version() {
        let mut b = vec![0u8; 40];
        b.extend_from_slice(&0xFEEF_04BDu32.to_le_bytes());
        b.extend_from_slice(&0x0001_0000u32.to_le_bytes());
        b.extend_from_slice(&((1u32 << 16) | 6).to_le_bytes());
        b.extend_from_slice(&(1170u32 << 16).to_le_bytes());
        assert_eq!(super::file_version(&b), Some((1, 6, 1170, 0)));
        assert_eq!(super::file_version(&[0; 8]), None);
    }

    #[test]
    fn the_text_version_wins_over_a_wrong_fixed_block() {
        // fixed block 1.0.0.0, string table "1.5.97.0" — the owner's SkyrimSE.exe
        let mut b = vec![0u8; 40];
        b.extend_from_slice(&0xFEEF_04BDu32.to_le_bytes());
        b.extend_from_slice(&0x0001_0000u32.to_le_bytes());
        b.extend_from_slice(&(1u32 << 16).to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&[0; 6]); // wLength, wValueLength, wType: the key starts 2 bytes past a boundary
        while b.len() % 4 != 2 {
            b.push(0);
        }
        let utf16 = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(u16::to_le_bytes).collect() };
        b.extend(utf16("FileVersion\0"));
        while !b.len().is_multiple_of(4) {
            b.push(0);
        }
        b.extend(utf16("1.5.97.0\0"));
        assert_eq!(super::file_version(&b), Some((1, 5, 97, 0)));
    }
    #[test]
    fn the_export_names_of_a_library_are_read_from_its_headers() {
        // a minimal PE32+: one section at RVA 0x1000 (file offset 0x200) holding the export directory and two names
        let mut b = vec![0u8; 0x400];
        b[0..2].copy_from_slice(b"MZ");
        b[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        b[0x40..0x44].copy_from_slice(b"PE\0\0");
        b[0x46..0x48].copy_from_slice(&1u16.to_le_bytes()); // sections
        b[0x54..0x56].copy_from_slice(&240u16.to_le_bytes()); // optional header size
        let opt = 0x40 + 24;
        b[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        b[opt + 112..opt + 116].copy_from_slice(&0x1000u32.to_le_bytes()); // export RVA
        b[opt + 116..opt + 120].copy_from_slice(&0x60u32.to_le_bytes()); // export size
        let sec = opt + 240;
        b[sec + 8..sec + 12].copy_from_slice(&0x200u32.to_le_bytes()); // virtual size
        b[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // virtual address
        b[sec + 16..sec + 20].copy_from_slice(&0x200u32.to_le_bytes()); // raw size
        b[sec + 20..sec + 24].copy_from_slice(&0x200u32.to_le_bytes()); // raw pointer
        let dir = 0x200;
        b[dir + 24..dir + 28].copy_from_slice(&2u32.to_le_bytes()); // names
        b[dir + 32..dir + 36].copy_from_slice(&0x1028u32.to_le_bytes()); // AddressOfNames
        b[dir + 0x28..dir + 0x2C].copy_from_slice(&0x1030u32.to_le_bytes());
        b[dir + 0x2C..dir + 0x30].copy_from_slice(&0x1048u32.to_le_bytes());
        b[dir + 0x30..dir + 0x30 + 16].copy_from_slice(b"SKSEPlugin_Load\0");
        b[dir + 0x48..dir + 0x48 + 17].copy_from_slice(b"SKSEPlugin_Query\0");
        let names = super::exports(&mut std::io::Cursor::new(&b)).unwrap();
        assert_eq!(names, ["SKSEPlugin_Load", "SKSEPlugin_Query"]);
        assert_eq!(
            super::exports(&mut std::io::Cursor::new(&b"not a library".to_vec())),
            None
        );
    }
}
