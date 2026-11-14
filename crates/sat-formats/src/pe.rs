//! The file version of a Windows executable or library (`VS_FIXEDFILEINFO`). @sat:formats.pe
//!
//! Owns: finding the fixed version block by its signature and reading the file version (major, minor, build,
//! revision). Not here: which files to read (`@sat:conflict.kinds.runtime`).
//!
//! The block is found by its signature `0xFEEF04BD` rather than by walking the resource tree: one pass over the
//! bytes, no PE parser, the same answer for the game's executable.

/// The file version, if the bytes hold a fixed version block.
pub fn file_version(bytes: &[u8]) -> Option<(u16, u16, u16, u16)> {
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
}
