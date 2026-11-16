//! Pictures for the preview: DDS decoded to a thumbnail, TGA, browser formats. @sat:formats.preview.image
//!
//! Owns: decoding BC1–BC7 and plain DDS formats of one mip level (via `bcdec_rs`), TGA (plain/RLE), PNG encoding, header sizes of PNG/JPEG/BMP/GIF.
//! Not here: DDS header facts and VRAM (`@sat:formats.dds`).

use super::{Cur, Kind, Picture, Preview, THUMB, base64};
use crate::dds::DdsInfo;

/// RGBA pixels → PNG bytes.
pub(crate) fn png(w: u32, h: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        let mut wr = e.write_header().ok()?;
        wr.write_image_data(rgba).ok()?;
    }
    Some(out)
}

/// Nearest-neighbour downscale so the larger side is at most [`THUMB`].
fn fit(w: u32, h: u32, rgba: Vec<u8>) -> (u32, u32, Vec<u8>) {
    let s = w.max(h);
    if s <= THUMB {
        return (w, h, rgba);
    }
    let (nw, nh) = (
        (w as u64 * THUMB as u64 / s as u64).max(1) as u32,
        (h as u64 * THUMB as u64 / s as u64).max(1) as u32,
    );
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        let sy = (y as u64 * h as u64 / nh as u64) as u32;
        for x in 0..nw {
            let sx = (x as u64 * w as u64 / nw as u64) as u32;
            let (d, o) = (((y * nw + x) * 4) as usize, ((sy * w + sx) * 4) as usize);
            out[d..d + 4].copy_from_slice(&rgba[o..o + 4]);
        }
    }
    (nw, nh, out)
}

fn picture(p: &mut Preview, w: u32, h: u32, rgba: Vec<u8>, note: String) {
    let (w, h, rgba) = fit(w, h, rgba);
    match png(w, h, &rgba) {
        Some(b) => {
            p.picture = Some(Picture {
                mime: "image/png".into(),
                data: base64(&b),
                width: w,
                height: h,
                note,
            })
        }
        None => p.note("картинка не собралась"),
    }
}

/// How a DXGI format decodes: block bytes (0 = not block compressed), bytes per pixel otherwise.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Enc {
    Bc1,
    Bc2,
    Bc3,
    Bc4,
    Bc5,
    Bc6,
    Bc7,
    Rgba8,
    Bgra8,
    Bgrx8,
    R8,
    Rg8,
    A8,
    Rgba16f,
    /// Legacy uncompressed with masks: bits per pixel, masks r g b a.
    Masks(u32, [u32; 4]),
}

impl Enc {
    fn block(self) -> usize {
        match self {
            Enc::Bc1 | Enc::Bc4 => 8,
            Enc::Bc2 | Enc::Bc3 | Enc::Bc5 | Enc::Bc6 | Enc::Bc7 => 16,
            _ => 0,
        }
    }
    fn bpp(self) -> usize {
        match self {
            Enc::Rgba8 | Enc::Bgra8 | Enc::Bgrx8 => 4,
            Enc::R8 | Enc::A8 => 1,
            Enc::Rg8 => 2,
            Enc::Rgba16f => 8,
            Enc::Masks(bits, _) => (bits / 8) as usize,
            _ => 0,
        }
    }
    fn of_dxgi(f: u32) -> Option<Enc> {
        Some(match f {
            70..=72 => Enc::Bc1,
            73..=75 => Enc::Bc2,
            76..=78 => Enc::Bc3,
            79..=81 => Enc::Bc4,
            82..=84 => Enc::Bc5,
            94..=96 => Enc::Bc6,
            97..=99 => Enc::Bc7,
            27..=29 => Enc::Rgba8,
            87 | 90 | 91 => Enc::Bgra8,
            88 | 92 | 93 => Enc::Bgrx8,
            61..=64 => Enc::R8,
            65 => Enc::A8,
            48..=52 => Enc::Rg8,
            10 => Enc::Rgba16f,
            _ => return None,
        })
    }
}

/// Bytes of one 2-D mip level.
fn level_bytes(e: Enc, w: u32, h: u32) -> usize {
    let (w, h) = (w.max(1) as usize, h.max(1) as usize);
    match e.block() {
        0 => w * h * e.bpp(),
        b => w.div_ceil(4) * h.div_ceil(4) * b,
    }
}

fn half(h: u16) -> f32 {
    let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let ex = ((h >> 10) & 0x1f) as i32;
    let m = (h & 0x3ff) as f32;
    s * match ex {
        0 => m * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + m / 1024.0) * 2f32.powi(ex - 15),
    }
}

fn to8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Decodes one level to RGBA.
fn decode(e: Enc, w: u32, h: u32, d: &[u8], normal: bool) -> Option<Vec<u8>> {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 4];
    if e.block() > 0 {
        let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
        let bs = e.block();
        let mut tile = [0u8; 64];
        let mut ftile = [0f32; 48];
        for by in 0..bh {
            for bx in 0..bw {
                let src = d.get((by * bw + bx) * bs..(by * bw + bx + 1) * bs)?;
                match e {
                    Enc::Bc1 => bcdec_rs::bc1(src, &mut tile, 16),
                    Enc::Bc2 => bcdec_rs::bc2(src, &mut tile, 16),
                    Enc::Bc3 => bcdec_rs::bc3(src, &mut tile, 16),
                    Enc::Bc7 => bcdec_rs::bc7(src, &mut tile, 16),
                    Enc::Bc4 => {
                        let mut r = [0u8; 16];
                        bcdec_rs::bc4(src, &mut r, 4, false);
                        for i in 0..16 {
                            tile[i * 4..i * 4 + 4].copy_from_slice(&[r[i], r[i], r[i], 255]);
                        }
                    }
                    Enc::Bc5 => {
                        let mut rg = [0u8; 32];
                        bcdec_rs::bc5(src, &mut rg, 8, false);
                        for i in 0..16 {
                            let (r, g) = (rg[i * 2], rg[i * 2 + 1]);
                            // a two-channel normal map: rebuild z for a readable picture
                            let b = if normal {
                                let (x, y) = (r as f32 / 127.5 - 1.0, g as f32 / 127.5 - 1.0);
                                to8(((1.0 - x * x - y * y).max(0.0).sqrt() + 1.0) / 2.0)
                            } else {
                                0
                            };
                            tile[i * 4..i * 4 + 4].copy_from_slice(&[r, g, b, 255]);
                        }
                    }
                    Enc::Bc6 => {
                        bcdec_rs::bc6h_float(src, &mut ftile, 12, false);
                        for i in 0..16 {
                            // a plain tone map: x / (1 + x)
                            let c = |v: f32| to8(v / (1.0 + v));
                            tile[i * 4..i * 4 + 4].copy_from_slice(&[
                                c(ftile[i * 3]),
                                c(ftile[i * 3 + 1]),
                                c(ftile[i * 3 + 2]),
                                255,
                            ]);
                        }
                    }
                    _ => unreachable!(),
                }
                for ty in 0..4 {
                    let y = by * 4 + ty;
                    if y >= h {
                        break;
                    }
                    for tx in 0..4 {
                        let x = bx * 4 + tx;
                        if x >= w {
                            break;
                        }
                        let (o, t) = ((y * w + x) * 4, (ty * 4 + tx) * 4);
                        out[o..o + 4].copy_from_slice(&tile[t..t + 4]);
                    }
                }
            }
        }
        return Some(out);
    }
    let bpp = e.bpp();
    for i in 0..w * h {
        let s = d.get(i * bpp..(i + 1) * bpp)?;
        let px = match e {
            Enc::Rgba8 => [s[0], s[1], s[2], s[3]],
            Enc::Bgra8 => [s[2], s[1], s[0], s[3]],
            Enc::Bgrx8 => [s[2], s[1], s[0], 255],
            Enc::R8 => [s[0], s[0], s[0], 255],
            Enc::A8 => [255, 255, 255, s[0]],
            Enc::Rg8 => [s[0], s[1], 0, 255],
            Enc::Rgba16f => {
                let c = |k: usize| to8(half(u16::from_le_bytes([s[k * 2], s[k * 2 + 1]])));
                [c(0), c(1), c(2), c(3)]
            }
            Enc::Masks(bits, m) => {
                let mut v = 0u32;
                for (k, &b) in s.iter().enumerate().take((bits / 8) as usize) {
                    v |= u32::from(b) << (8 * k);
                }
                let ch = |mask: u32, def: u8| {
                    if mask == 0 {
                        return def;
                    }
                    let shift = mask.trailing_zeros();
                    let max = mask >> shift;
                    (((v & mask) >> shift) * 255 / max.max(1)) as u8
                };
                [ch(m[0], 0), ch(m[1], 0), ch(m[2], 0), ch(m[3], 255)]
            }
            _ => unreachable!(),
        };
        out[i * 4..i * 4 + 4].copy_from_slice(&px);
    }
    Some(out)
}

/// DDS: header facts and a thumbnail of the first mip level not larger than [`THUMB`].
pub(super) fn dds(b: &[u8]) -> Preview {
    let Some(info) = DdsInfo::parse(b) else {
        let mut p = Preview::new(Kind::Image, "DDS");
        p.note("заголовок DDS не читается");
        return p;
    };
    let mut p = Preview::new(
        Kind::Image,
        format!("DDS · {} · {}×{}", info.format, info.width, info.height),
    );
    p.fact("Формат", &info.format);
    p.fact("Размер в пикселях", format!("{}×{}", info.width, info.height));
    p.fact("Мипы", info.mips);
    if info.layers == 6 {
        p.fact("Кубокарта", "да");
    } else if info.layers > 1 {
        p.fact("Слоёв", info.layers);
    }
    if info.depth > 1 {
        p.fact("Глубина", info.depth);
    }
    if info.vram > 0 {
        p.fact("Видеопамять", super::human(info.vram));
    }
    // the format to decode
    let pf_flags = crate::read::u32le(b, 80);
    let four = &b[84..88];
    let (enc, data_at) = if pf_flags & 4 != 0 {
        match four {
            b"DX10" => (
                b.get(128..132)
                    .and_then(|x| Enc::of_dxgi(u32::from_le_bytes([x[0], x[1], x[2], x[3]]))),
                148,
            ),
            b"DXT1" => (Some(Enc::Bc1), 128),
            b"DXT2" | b"DXT3" => (Some(Enc::Bc2), 128),
            b"DXT4" | b"DXT5" => (Some(Enc::Bc3), 128),
            b"ATI1" | b"BC4U" => (Some(Enc::Bc4), 128),
            b"ATI2" | b"BC5U" => (Some(Enc::Bc5), 128),
            _ => (None, 128),
        }
    } else {
        let bits = crate::read::u32le(b, 88);
        let m = [
            crate::read::u32le(b, 92),
            crate::read::u32le(b, 96),
            crate::read::u32le(b, 100),
            crate::read::u32le(b, 104),
        ];
        let alpha = pf_flags & 1 != 0;
        let m = if alpha { m } else { [m[0], m[1], m[2], 0] };
        match bits {
            8 | 16 | 24 | 32 => (Some(Enc::Masks(bits, m)), 128),
            _ => (None, 128),
        }
    };
    let Some(enc) = enc else {
        p.note(format!("формат {} не распаковывается для просмотра", info.format));
        return p;
    };
    // the first level small enough
    let (mut w, mut h, mut at, mut lv) = (info.width.max(1), info.height.max(1), data_at, 0u32);
    while (w > THUMB || h > THUMB) && lv + 1 < info.mips {
        at += level_bytes(enc, w, h) * info.depth.max(1) as usize;
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        lv += 1;
    }
    let need = level_bytes(enc, w, h);
    let Some(data) = b.get(at..at + need) else {
        p.note("данных уровня меньше, чем обещает заголовок");
        return p;
    };
    let normal = enc == Enc::Bc5 || info.format.contains("BC5");
    match decode(enc, w, h, data, normal) {
        Some(rgba) => {
            let note = if lv > 0 {
                format!("мип {lv} из {}, {w}×{h}", info.mips)
            } else {
                String::new()
            };
            picture(&mut p, w, h, rgba, note)
        }
        None => p.note("не распаковалось"),
    }
    p
}

/// TGA: truecolor and grayscale, plain or RLE.
pub(super) fn tga(b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Image, "TGA");
    let mut c = Cur::new(b);
    let (Some(id_len), Some(cmap), Some(ty)) = (c.u8(), c.u8(), c.u8()) else {
        p.note("заголовок TGA не читается");
        return p;
    };
    let cmap_len = {
        c.skip(2);
        let l = c.u16().unwrap_or(0);
        let bits = c.u8().unwrap_or(0);
        l as usize * (bits as usize).div_ceil(8)
    };
    c.skip(4);
    let (w, h, bpp, desc) = (
        c.u16().unwrap_or(0) as u32,
        c.u16().unwrap_or(0) as u32,
        c.u8().unwrap_or(0),
        c.u8().unwrap_or(0),
    );
    p.format = format!("TGA · {w}×{h} · {bpp} бит");
    p.fact("Размер в пикселях", format!("{w}×{h}"));
    p.fact("Бит на пиксель", bpp);
    p.fact("Сжатие", if ty >= 9 { "RLE" } else { "нет" });
    if cmap != 0
        || !matches!(ty, 2 | 3 | 10 | 11)
        || !matches!(bpp, 8 | 24 | 32)
        || w == 0
        || h == 0
        || w > 16384
        || h > 16384
    {
        p.note("этот вид TGA не показывается (палитра или редкий формат)");
        return p;
    }
    c.skip(id_len as usize + cmap_len);
    let px = (bpp / 8) as usize;
    let n = (w * h) as usize;
    let mut raw: Vec<u8> = Vec::with_capacity(n * px);
    if ty >= 9 {
        while raw.len() < n * px {
            let Some(hd) = c.u8() else { break };
            let count = (hd & 0x7f) as usize + 1;
            if hd & 0x80 != 0 {
                let Some(v) = c.take(px) else { break };
                for _ in 0..count {
                    raw.extend_from_slice(v);
                }
            } else {
                let Some(v) = c.take(px * count) else { break };
                raw.extend_from_slice(v);
            }
        }
        raw.truncate(n * px);
    } else if let Some(v) = c.take(n * px) {
        raw.extend_from_slice(v);
    }
    if raw.len() < n * px {
        p.note("данных меньше, чем обещает заголовок");
        return p;
    }
    let top = desc & 0x20 != 0;
    let mut rgba = vec![0u8; n * 4];
    for y in 0..h as usize {
        let sy = if top { y } else { h as usize - 1 - y };
        for x in 0..w as usize {
            let s = &raw[(sy * w as usize + x) * px..][..px];
            let v = match px {
                1 => [s[0], s[0], s[0], 255],
                3 => [s[2], s[1], s[0], 255],
                _ => [s[2], s[1], s[0], s[3]],
            };
            rgba[(y * w as usize + x) * 4..][..4].copy_from_slice(&v);
        }
    }
    picture(&mut p, w, h, rgba, String::new());
    p
}

/// PNG, JPEG, BMP, GIF, WebP: the browser shows them; read the size from the header.
pub(super) fn raster(e: &str, b: &[u8]) -> Preview {
    let (mime, dims) = match e {
        "png" => (
            "image/png",
            b.get(16..24).map(|x| {
                (
                    u32::from_be_bytes([x[0], x[1], x[2], x[3]]),
                    u32::from_be_bytes([x[4], x[5], x[6], x[7]]),
                )
            }),
        ),
        "jpg" | "jpeg" => ("image/jpeg", jpeg_size(b)),
        "bmp" => (
            "image/bmp",
            b.get(18..26).map(|x| {
                (
                    u32::from_le_bytes([x[0], x[1], x[2], x[3]]),
                    i32::from_le_bytes([x[4], x[5], x[6], x[7]]).unsigned_abs(),
                )
            }),
        ),
        "gif" => (
            "image/gif",
            b.get(6..10).map(|x| {
                (
                    u16::from_le_bytes([x[0], x[1]]) as u32,
                    u16::from_le_bytes([x[2], x[3]]) as u32,
                )
            }),
        ),
        _ => ("image/webp", None),
    };
    let mut p = Preview::new(Kind::Image, e.to_uppercase());
    if let Some((w, h)) = dims {
        p.format = format!("{} · {w}×{h}", e.to_uppercase());
        p.fact("Размер в пикселях", format!("{w}×{h}"));
    }
    if b.len() <= 16 << 20 {
        let (w, h) = dims.unwrap_or((0, 0));
        p.picture = Some(Picture {
            mime: mime.into(),
            data: base64(b),
            width: w,
            height: h,
            note: String::new(),
        });
    } else {
        p.note("файл больше 16 МБ — не показывается");
    }
    p
}

fn jpeg_size(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    while i + 9 < b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let m = b[i + 1];
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if (0xC0..=0xCF).contains(&m) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
            let h = u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32;
            let w = u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32;
            return Some((w, h));
        }
        i += 2 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::{Kind, inspect};

    /// A DDS with the given fourCC and data.
    pub(crate) fn dds(four: &[u8; 4], w: u32, h: u32, mips: u32, data: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[0..4].copy_from_slice(b"DDS ");
        b[4..8].copy_from_slice(&124u32.to_le_bytes());
        b[12..16].copy_from_slice(&h.to_le_bytes());
        b[16..20].copy_from_slice(&w.to_le_bytes());
        b[28..32].copy_from_slice(&mips.to_le_bytes());
        b[76..80].copy_from_slice(&32u32.to_le_bytes());
        b[80..84].copy_from_slice(&4u32.to_le_bytes());
        b[84..88].copy_from_slice(four);
        b.extend_from_slice(data);
        b
    }

    #[test]
    fn a_bc1_texture_decodes_to_a_png_thumbnail() {
        // one 4×4 block: color0 = red (565: 0xF800), color1 = 0, all indices 0 → red
        let block = [0x00, 0xF8, 0x00, 0x00, 0, 0, 0, 0];
        let p = inspect("t.dds", &dds(b"DXT1", 4, 4, 1, &block));
        assert_eq!(p.kind, Kind::Image);
        let pic = p.picture.expect("a picture");
        assert_eq!((pic.width, pic.height, pic.mime.as_str()), (4, 4, "image/png"));
        assert!(p.format.contains("BC1"));
    }

    #[test]
    fn a_big_texture_shows_the_first_small_enough_mip() {
        // 2048×2048 BC1 with 2 mips: level 0 is 1024×... skip it, decode 1024×1024
        let l0 = 512 * 512 * 8;
        let l1 = 256 * 256 * 8;
        let data = vec![0u8; l0 + l1];
        let p = inspect("t.dds", &dds(b"DXT1", 2048, 2048, 2, &data));
        let pic = p.picture.expect("a picture");
        assert_eq!((pic.width, pic.height), (1024, 1024));
        assert_eq!(pic.note, "мип 1 из 2, 1024×1024");
        // short data: a note, not a panic
        let p = inspect("t.dds", &dds(b"DXT1", 64, 64, 1, &[0; 10]));
        assert!(p.picture.is_none() && !p.notes.is_empty());
    }

    #[test]
    fn tga_rle_and_plain() {
        // 2×1 truecolor RLE: one packet repeating blue twice
        let mut b = vec![0u8, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 1, 0, 24, 0x20];
        b.extend_from_slice(&[0x81, 255, 0, 0]);
        let p = inspect("a.tga", &b);
        assert!(p.picture.is_some(), "{:?}", p.notes);
        assert!(p.format.contains("2×1"));
    }
}
