//! DDS header decoding and GPU memory estimation.

use serde::{Deserialize, Serialize};

use crate::read::u32le;

/// Size of the header we need: 4 (magic) + 124 (DDS_HEADER) + 20 (DDS_HEADER_DXT10).
pub const DDS_HEADER_PROBE: usize = 148;

/// Decoded DDS header.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DdsInfo {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub mips: u32,
    /// Faces × array size (6 for a cubemap).
    pub layers: u32,
    pub format: String,
    /// Estimated GPU bytes for the full mip chain; 0 when unknown.
    pub vram: u64,
}

impl DdsInfo {
    pub fn known(&self) -> bool {
        self.vram > 0
    }

    /// Parses the first [`DDS_HEADER_PROBE`] bytes of a file (shorter input is fine for non-DX10 files).
    pub fn parse(b: &[u8]) -> Option<DdsInfo> {
        if b.len() < 128 || &b[0..4] != b"DDS " {
            return None;
        }
        let mut d = DdsInfo {
            height: u32le(b, 12),
            width: u32le(b, 16),
            depth: u32le(b, 24).max(1),
            mips: u32le(b, 28).max(1),
            layers: 1,
            ..Default::default()
        };
        let pf_flags = u32le(b, 80);
        let four_cc = &b[84..88];
        let bits = u32le(b, 88);
        let caps2 = u32le(b, 112);
        if caps2 & 0x200 != 0 {
            d.layers = 6;
        }
        const DDPF_FOURCC: u32 = 0x4;
        let dxgi = if pf_flags & DDPF_FOURCC != 0 {
            match four_cc {
                b"DX10" => {
                    if b.len() < DDS_HEADER_PROBE {
                        return Some(d);
                    }
                    let misc = u32le(b, 136);
                    let array = u32le(b, 140).max(1);
                    if misc & 0x4 != 0 {
                        d.layers = 6 * array;
                    } else {
                        d.layers *= array;
                    }
                    u32le(b, 128)
                }
                b"DXT1" => 71,
                b"DXT2" | b"DXT3" => 74,
                b"DXT4" | b"DXT5" => 77,
                b"ATI1" | b"BC4U" => 80,
                b"ATI2" | b"BC5U" => 83,
                other => {
                    d.format = String::from_utf8_lossy(other).into_owned();
                    return Some(d);
                }
            }
        } else {
            if bits == 0 {
                return Some(d);
            }
            d.format = format!("RGB{bits}");
            d.vram = uncompressed_bytes(d.width, d.height, d.depth, d.mips, bits) * u64::from(d.layers);
            return Some(d);
        };
        d.format = dxgi_name(dxgi);
        d.vram = texture_bytes(d.width, d.height, d.depth, d.mips, dxgi, d.layers);
        Some(d)
    }
}

fn uncompressed_bytes(w: u32, h: u32, depth: u32, mips: u32, bits: u32) -> u64 {
    (0..mips)
        .map(|m| {
            let (mw, mh, md) = ((w >> m).max(1), (h >> m).max(1), (depth >> m).max(1));
            u64::from(mw) * u64::from(mh) * u64::from(md) * u64::from(bits) / 8
        })
        .sum()
}

/// Estimated GPU memory of a DXGI-format texture with its mip chain.
pub fn texture_bytes(w: u32, h: u32, depth: u32, mips: u32, dxgi: u32, layers: u32) -> u64 {
    let (block, bpp) = dxgi_block(dxgi);
    if block == 0 && bpp == 0 {
        return 0;
    }
    let (mips, depth) = (mips.max(1), depth.max(1));
    let per_layer: u64 = (0..mips)
        .map(|m| {
            let (mw, mh, md) = ((w >> m).max(1), (h >> m).max(1), (depth >> m).max(1));
            if block > 0 {
                u64::from(mw.div_ceil(4)) * u64::from(mh.div_ceil(4)) * u64::from(block) * u64::from(md)
            } else {
                u64::from(mw) * u64::from(mh) * u64::from(md) * u64::from(bpp) / 8
            }
        })
        .sum();
    per_layer * u64::from(layers.max(1))
}

/// `(bytes per 4×4 block, 0)` for block-compressed formats, `(0, bits per pixel)` otherwise.
fn dxgi_block(f: u32) -> (u32, u32) {
    match f {
        70..=72 | 79..=81 => (8, 0),
        73..=78 | 82..=84 | 94..=99 => (16, 0),
        2..=4 => (0, 128),
        6..=8 => (0, 96),
        10..=14 | 16..=18 => (0, 64),
        24..=32 | 34..=43 | 87..=93 => (0, 32),
        49..=59 | 85 | 86 | 115 => (0, 16),
        60..=65 => (0, 8),
        _ => (0, 0),
    }
}

/// Short human name for common DXGI formats.
pub fn dxgi_name(f: u32) -> String {
    match f {
        70..=72 => "BC1",
        73..=75 => "BC2",
        76..=78 => "BC3",
        79..=81 => "BC4",
        82..=84 => "BC5",
        94..=96 => "BC6H",
        97..=99 => "BC7",
        28 | 29 | 87 | 91 => "RGBA8",
        _ => return format!("DXGI{f}"),
    }
    .to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn header(w: u32, h: u32, mips: u32, four_cc: &[u8; 4]) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[0..4].copy_from_slice(b"DDS ");
        b[4..8].copy_from_slice(&124u32.to_le_bytes());
        b[12..16].copy_from_slice(&h.to_le_bytes());
        b[16..20].copy_from_slice(&w.to_le_bytes());
        b[28..32].copy_from_slice(&mips.to_le_bytes());
        b[76..80].copy_from_slice(&32u32.to_le_bytes());
        b[80..84].copy_from_slice(&4u32.to_le_bytes());
        b[84..88].copy_from_slice(four_cc);
        b
    }

    #[test]
    fn bc3_1024_full_chain() {
        let d = DdsInfo::parse(&header(1024, 1024, 11, b"DXT5")).unwrap();
        assert_eq!(d.format, "BC3");
        // 1 byte per pixel for BC3: 1024² + 512² + … + 4×4 blocks for the tail
        let expect: u64 = (0..11u32)
            .map(|m| {
                let s = (1024u32 >> m).max(1);
                u64::from(s.div_ceil(4)).pow(2) * 16
            })
            .sum();
        assert_eq!(d.vram, expect);
    }

    #[test]
    fn dx10_cubemap() {
        let mut b = header(256, 256, 1, b"DX10");
        b.resize(DDS_HEADER_PROBE, 0);
        b[128..132].copy_from_slice(&98u32.to_le_bytes()); // BC7
        b[136..140].copy_from_slice(&4u32.to_le_bytes()); // TEXTURECUBE
        b[140..144].copy_from_slice(&1u32.to_le_bytes());
        let d = DdsInfo::parse(&b).unwrap();
        assert_eq!(d.format, "BC7");
        assert_eq!(d.layers, 6);
        assert_eq!(d.vram, 256 * 256 * 6);
    }

    #[test]
    fn not_dds() {
        assert!(DdsInfo::parse(&[0u8; 200]).is_none());
    }
}
