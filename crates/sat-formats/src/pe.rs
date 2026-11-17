//! The file version of a Windows executable or library (`VS_FIXEDFILEINFO`). @sat:formats.pe
//!
//! Owns: finding the fixed version block by its signature and reading the file version (major, minor, build,
//! revision). Not here: which files to read (`@sat:conflict.kinds.runtime`).
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
}
