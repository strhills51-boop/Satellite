//! Sound, video and Scaleform for the preview: WAV, XWM, FUZ, LIP, BIK, SWF/GFX, Ogg/MP3/FLAC. @sat:formats.preview.media
//!
//! Owns: RIFF chunks and the fmt block, FUZ parts, Bink header, SWF stage, frame rate and frames; WAV, Ogg, MP3, FLAC bytes to play.

use super::{Cur, Kind, Preview};

/// RIFF chunks: (id, data).
fn chunks(b: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut i = 12;
    while i + 8 <= b.len() && out.len() < 64 {
        let id: [u8; 4] = b[i..i + 4].try_into().unwrap_or([0; 4]);
        let n = u32::from_le_bytes(b[i + 4..i + 8].try_into().unwrap_or([0; 4])) as usize;
        let end = (i + 8).saturating_add(n).min(b.len());
        out.push((id, &b[i + 8..end]));
        i = end + (n & 1);
    }
    out
}

fn fmt(p: &mut Preview, f: &[u8], data: usize) -> Option<f64> {
    let mut c = Cur::new(f);
    let tag = c.u16()?;
    let ch = c.u16()?;
    let rate = c.u32()?;
    let bytes_sec = c.u32()?;
    let _align = c.u16()?;
    let bits = c.u16()?;
    let codec = match tag {
        1 => "PCM",
        2 => "MS ADPCM",
        0x11 => "IMA ADPCM",
        0x161 | 0x162 => "xWMA",
        0xFFFE => "PCM (extensible)",
        _ => "другой",
    };
    p.fact("Кодек", codec);
    p.fact("Каналы", ch);
    p.fact("Частота", format!("{rate} Гц"));
    if bits > 0 {
        p.fact("Бит", bits);
    }
    let secs = if bytes_sec > 0 {
        data as f64 / bytes_sec as f64
    } else {
        0.0
    };
    if secs > 0.0 {
        p.fact("Длительность", format!("{secs:.1} с"));
    }
    p.format = format!(
        "{} · {codec} · {ch} кан · {rate} Гц{}",
        p.format,
        if secs > 0.0 {
            format!(" · {secs:.1} с")
        } else {
            String::new()
        }
    );
    Some(secs)
}

pub(super) fn wav(b: &[u8]) -> Preview {
    if b.get(8..12) == Some(b"XWMA") {
        return xwm(b);
    }
    let mut p = Preview::new(Kind::Audio, "WAV");
    if b.get(..4) != Some(b"RIFF") || b.get(8..12) != Some(b"WAVE") {
        p.note("это не RIFF WAVE");
        return p;
    }
    let ch = chunks(b);
    let data = ch.iter().find(|c| &c.0 == b"data").map_or(0, |c| c.1.len());
    match ch.iter().find(|c| &c.0 == b"fmt ") {
        Some((_, f)) => {
            fmt(&mut p, f, data);
            // the browser plays PCM wav itself
            if b.len() <= 32 << 20 {
                p.audio = Some(super::Media {
                    mime: "audio/wav".into(),
                    data: super::base64(b),
                });
            }
        }
        None => p.note("нет блока fmt"),
    }
    p
}

pub(super) fn xwm(b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Audio, "XWM");
    if b.get(8..12) != Some(b"XWMA") {
        p.note("это не XWMA");
        return p;
    }
    let ch = chunks(b);
    let data = ch.iter().find(|c| &c.0 == b"data").map_or(0, |c| c.1.len());
    if let Some((_, f)) = ch.iter().find(|c| &c.0 == b"fmt ") {
        fmt(&mut p, f, data);
    }
    p.note("xWMA браузер не проигрывает — только сведения");
    p
}

pub(super) fn fuz(b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Audio, "FUZ");
    let mut c = Cur::new(b);
    if c.take(4) != Some(b"FUZE") {
        p.note("это не FUZ");
        return p;
    }
    let _ver = c.u32();
    let lip = c.u32().unwrap_or(0) as usize;
    p.fact("Липсинк (LIP)", super::human(lip as u64));
    let audio_at = 12 + lip;
    if let Some(a) = b.get(audio_at..) {
        p.fact("Звук", super::human(a.len() as u64));
        let x = xwm(a);
        p.format = format!("FUZ · {}", x.format);
        p.facts.extend(x.facts.into_iter().filter(|(k, _)| k != "Размер"));
    }
    p
}

pub(super) fn lip(b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Audio, "LIP — липсинк");
    p.fact("Что это", "движения губ к реплике (FaceFX)");
    super::binary_tail(&mut p, b);
    p
}

pub(super) fn bik(b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Video, "Bink");
    let mut c = Cur::new(b);
    let sig = c.take(3).unwrap_or(b"???");
    let ver = c.u8().unwrap_or(0) as char;
    if sig != b"BIK" && sig != b"KB2" {
        p.note("это не Bink");
        return p;
    }
    let _size = c.u32();
    let frames = c.u32().unwrap_or(0);
    let _largest = c.u32();
    let _frames2 = c.u32();
    let w = c.u32().unwrap_or(0);
    let h = c.u32().unwrap_or(0);
    let num = c.u32().unwrap_or(0);
    let den = c.u32().unwrap_or(1).max(1);
    let fps = num as f64 / den as f64;
    p.format = format!(
        "Bink {} · {w}×{h} · {fps:.0} к/с",
        if sig == b"KB2" { "2" } else { "1" }
    );
    p.fact("Версия", format!("{}{ver}", String::from_utf8_lossy(sig)));
    p.fact("Размер кадра", format!("{w}×{h}"));
    p.fact("Кадров", frames);
    if fps > 0.0 {
        p.fact("Длительность", format!("{:.1} с", frames as f64 / fps));
    }
    p
}

/// Scaleform: SWF/GFX header (FWS plain, CWS zlib, GFX/CFX Scaleform), stage size, frame rate, frames.
pub(super) fn swf(b: &[u8]) -> Preview {
    let mut p = Preview::new(Kind::Interface, "SWF");
    let Some(sig) = b.get(..3) else {
        p.note("пустой файл");
        return p;
    };
    let ver = b.get(3).copied().unwrap_or(0);
    let body: Vec<u8> = match sig {
        b"FWS" | b"GFX" => b.get(8..).unwrap_or(&[]).iter().take(64).copied().collect(),
        b"CWS" | b"CFX" => {
            use std::io::Read;
            let mut out = Vec::new();
            let _ = flate2::read::ZlibDecoder::new(b.get(8..).unwrap_or(&[]))
                .take(64)
                .read_to_end(&mut out);
            out
        }
        b"ZWS" => {
            p.note("сжато LZMA — размеры не читаются");
            Vec::new()
        }
        _ => {
            p.note("это не SWF");
            return p;
        }
    };
    p.fact("Подпись", String::from_utf8_lossy(sig));
    p.fact("Версия Flash", ver);
    if let Some(len) = b.get(4..8) {
        p.fact(
            "Размер распакованного",
            super::human(u32::from_le_bytes(len.try_into().unwrap_or([0; 4])) as u64),
        );
    }
    if !body.is_empty() {
        // RECT: 5 bits n, then 4 × n bits (twips)
        let bit = |i: usize| (body[i / 8] >> (7 - i % 8)) & 1;
        let read = |from: usize, n: usize| (0..n).fold(0i64, |a, k| (a << 1) | bit(from + k) as i64);
        let n = read(0, 5) as usize;
        if 5 + 4 * n <= body.len() * 8 {
            let (xmin, xmax, ymin, ymax) =
                (read(5, n), read(5 + n, n), read(5 + 2 * n, n), read(5 + 3 * n, n));
            let (w, h) = ((xmax - xmin) / 20, (ymax - ymin) / 20);
            let at = (5 + 4 * n).div_ceil(8);
            if let Some(r) = body.get(at..at + 4) {
                let fps = r[1] as f32 + r[0] as f32 / 256.0;
                let frames = u16::from_le_bytes([r[2], r[3]]);
                p.fact("Сцена", format!("{w}×{h}"));
                p.fact("Кадров в секунду", fps);
                p.fact("Кадров", frames);
                p.format = format!("{} {ver} · {w}×{h}", String::from_utf8_lossy(sig));
            }
        }
    }
    // the interface names other files (fonts, images, other movies)
    for l in super::mentioned_paths(b, &[".swf", ".gfx", ".dds", ".png"]) {
        p.link(&l);
    }
    p
}

/// Sound the browser plays as is (Ogg, MP3, FLAC, Opus, M4A): the format, and the bytes when small enough.
pub(super) fn play(e: &str, b: &[u8]) -> Preview {
    let (fmt, mime) = match b.get(..4) {
        Some(b"OggS") => (
            "Ogg",
            if e == "opus" {
                "audio/ogg; codecs=opus"
            } else {
                "audio/ogg"
            },
        ),
        Some(b"fLaC") => ("FLAC", "audio/flac"),
        Some([b'I', b'D', b'3', _]) | Some([0xFF, 0xFB | 0xF3 | 0xF2, _, _]) => ("MP3", "audio/mpeg"),
        _ if b.get(4..8) == Some(b"ftyp") => ("M4A", "audio/mp4"),
        _ => (e, "application/octet-stream"),
    };
    let mut p = Preview::new(Kind::Audio, fmt.to_uppercase());
    if mime.starts_with("audio/") && b.len() <= 32 << 20 {
        p.audio = Some(super::Media {
            mime: mime.into(),
            data: super::base64(b),
        });
    } else if !mime.starts_with("audio/") {
        p.note("формат не распознан");
    }
    p
}

#[cfg(test)]
mod tests {
    use super::super::{Kind, inspect};

    #[test]
    fn a_wav_says_codec_channels_rate_and_length_and_plays() {
        let mut b = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&44100u32.to_le_bytes());
        b.extend_from_slice(&88200u32.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&88200u32.to_le_bytes());
        b.extend_from_slice(&vec![0u8; 88200]);
        let p = inspect("a.wav", &b);
        assert_eq!(p.kind, Kind::Audio);
        assert_eq!(p.format, "WAV · PCM · 1 кан · 44100 Гц · 1.0 с");
        assert_eq!(p.audio.map(|a| a.mime).as_deref(), Some("audio/wav"));
    }

    #[test]
    fn a_swf_says_its_stage() {
        // FWS v10, RECT n=15: 0..12800 twips × 0..9600 (640×480), 30 fps, 1 frame
        let mut bits = String::from("01111");
        for v in [0u32, 12800, 0, 9600] {
            bits.push_str(&format!("{v:015b}"));
        }
        while bits.len() % 8 != 0 {
            bits.push('0');
        }
        let mut b = b"FWS\x0a\0\0\0\0".to_vec();
        b.extend((0..bits.len() / 8).map(|i| u8::from_str_radix(&bits[i * 8..i * 8 + 8], 2).unwrap()));
        b.extend_from_slice(&[0, 30, 1, 0]);
        let p = inspect("menu.swf", &b);
        assert_eq!(p.format, "FWS 10 · 640×480");
    }
}
