//! What a file is and what is in it, for the file explorer's preview: one model for every format. @sat:formats.preview
//!
//! Owns: the preview model (`Preview`: kind, format line, facts, sections, text, picture, sound, links to other
//! files, hex head, notes) and `inspect(name, bytes)` — dispatch by extension and by magic to the readers of
//! `preview::*`, with the text and binary fallbacks; the listing model of folders outside the build (`Place`,
//! `DiskEntry`: the game, the profile, the instance, the game's documents).
//! Not here: where the bytes come from (`@sat:scan.fetch`), which copy of a path (`@sat:app.preview`), drawing.
//!
//! Read-only: nothing here edits a file (editing waits for commit processing). Every reader is bounded: a corrupt
//! or hostile file gives a partial preview and a note, never a panic or a huge allocation. Large outputs are cut:
//! text at [`TEXT_LIMIT`], lists at [`LIST_LIMIT`], pictures to a thumbnail of at most [`THUMB`] pixels a side.

mod game;
mod hkx;
mod image;
mod media;
mod nif;
mod pex;
mod save;
mod text;

use serde::{Deserialize, Serialize};

/// Text shown at most (bytes of the decoded text).
pub const TEXT_LIMIT: usize = 512 * 1024;
/// Items in a list section at most.
pub const LIST_LIMIT: usize = 400;
/// Largest side of a picture preview.
pub const THUMB: u32 = 1024;

/// What kind of file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Text,
    Image,
    Mesh,
    Animation,
    Script,
    Plugin,
    Archive,
    Audio,
    Video,
    Interface,
    Strings,
    Library,
    Facegen,
    Save,
    Material,
    Document,
    #[default]
    Binary,
}

/// A titled list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub title: String,
    pub lines: Vec<String>,
    /// Items not listed.
    #[serde(default)]
    pub more: usize,
}

/// Text to show.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextBody {
    /// `ini`, `json`, `xml`, `papyrus`, `toml`, `yaml`, `text`, …
    pub lang: String,
    pub body: String,
    pub truncated: bool,
}

/// A picture to show: PNG (decoded and encoded here) or the file itself when the browser shows it as is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Picture {
    pub mime: String,
    /// Base64.
    pub data: String,
    pub width: u32,
    pub height: u32,
    /// What is shown, when not the whole image (`мип 2 из 12, 1024×1024`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// Bytes the browser plays as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Media {
    pub mime: String,
    /// Base64.
    pub data: String,
}

/// A folder outside the build the explorer shows: the game, the profile, the instance, the game's documents.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    /// `game`, `profile`, `instance`, `documents`.
    pub id: String,
    pub label: String,
    /// The folder on disk, for the user.
    pub root: String,
}

/// One child of a folder of a place; an archive (BSA, BA2) is a folder of its entries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskEntry {
    pub name: String,
    /// Relative to the place, forward slashes; inside an archive: `Data/A.bsa/textures/x.dds`.
    pub path: String,
    pub dir: bool,
    pub size: u64,
    /// An archive shown as a folder.
    #[serde(default)]
    pub archive: bool,
    /// Packed in an archive.
    #[serde(default)]
    pub inside: bool,
    /// Files under a folder of an archive.
    #[serde(default)]
    pub files: u32,
}

/// The preview of one file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preview {
    pub kind: Kind,
    /// One line: `DDS · BC7 · 2048×2048`.
    pub format: String,
    pub facts: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<Section>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<TextBody>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<Picture>,
    /// Sound the browser plays itself (WAV): MIME and base64 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<Media>,
    /// Paths of other game files this one refers to (textures of a mesh, a script's parent), data-relative,
    /// lower case, forward slashes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
    /// Hex of the head, for binaries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hex: Option<String>,
    /// What could not be read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl Preview {
    pub(crate) fn new(kind: Kind, format: impl Into<String>) -> Self {
        Self {
            kind,
            format: format.into(),
            ..Default::default()
        }
    }
    pub(crate) fn fact(&mut self, k: &str, v: impl ToString) {
        self.facts.push((k.to_string(), v.to_string()));
    }
    pub(crate) fn section(&mut self, title: &str, mut lines: Vec<String>) {
        if lines.is_empty() {
            return;
        }
        let more = lines.len().saturating_sub(LIST_LIMIT);
        lines.truncate(LIST_LIMIT);
        self.sections.push(Section {
            title: title.to_string(),
            lines,
            more,
        });
    }
    pub(crate) fn note(&mut self, n: impl Into<String>) {
        self.notes.push(n.into());
    }
    pub(crate) fn link(&mut self, path: &str) {
        let p = crate::normalize_path(path);
        if !p.is_empty() && !self.links.contains(&p) && self.links.len() < LIST_LIMIT {
            self.links.push(p);
        }
    }
}

/// The extension, lower case, without the dot.
pub(crate) fn ext(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match base.rsplit_once('.') {
        Some((_, e)) => e.to_ascii_lowercase(),
        None => String::new(),
    }
}

/// An archive read from its file (big archives are not read whole: the manifest is enough).
pub fn inspect_archive<R: std::io::Read + std::io::Seek>(r: R, size: u64) -> Preview {
    let mut p = game::archive_from(r);
    p.fact("Размер", human(size));
    p
}

/// The head of a file too big to read whole: what the head says, with a note.
pub fn inspect_head(name: &str, head: &[u8], size: u64) -> Preview {
    let mut p = inspect(name, head);
    if let Some(f) = p.facts.iter_mut().find(|f| f.0 == "Размер") {
        f.1 = human(size);
    }
    p.note(format!(
        "файл {} — прочитано только начало ({})",
        human(size),
        human(head.len() as u64)
    ));
    p
}

// @sat:formats.preview.readers
/// The reader an extension gets (`None`: by magic, else text when it looks like text, else hex) — for the census of
/// a build's extensions. Kept next to `inspect`'s dispatch: a new reader is added in both.
pub fn reader_of(e: &str) -> Option<&'static str> {
    Some(match e {
        "dds" | "tga" | "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp" => "картинка",
        "nif" | "kf" | "bto" | "btr" | "btt" | "nifcache" => "NIF",
        "hkx" | "hkt" => "Havok",
        "pex" => "Papyrus (скомпилированный)",
        "esp" | "esm" | "esl" => "плагин",
        "bsa" | "ba2" => "архив игры",
        "wav" | "xwm" | "fuz" | "ogg" | "mp3" | "flac" | "opus" | "m4a" => "звук",
        "lip" => "LIP",
        "bik" | "bk2" => "видео Bink",
        "swf" | "gfx" => "Scaleform",
        "seq" => "SEQ",
        "tri" => "TRI",
        "egm" | "egt" | "fg" => "FaceGen",
        "strings" | "dlstrings" | "ilstrings" => "строки",
        "dll" | "exe" => "библиотека",
        "ttf" | "otf" | "fnt" => "шрифт",
        "ess" | "fos" => "сохранение",
        "bgsm" | "bgem" => "материал",
        "zip" | "7z" | "rar" => "упакованный архив",
        "pdf" | "psd" => "документ",
        _ => return text::lang_of(e).map(|_| "текст"),
    })
}

/// What a file is: the reader for its extension, then by magic, then text or binary.
pub fn inspect(name: &str, bytes: &[u8]) -> Preview {
    let e = ext(name);
    let mut p = match e.as_str() {
        "dds" => image::dds(bytes),
        "tga" => image::tga(bytes),
        "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp" => image::raster(&e, bytes),
        "nif" | "kf" | "bto" | "btr" | "btt" | "nifcache" => nif::read(bytes),
        "hkx" | "hkt" => hkx::read(bytes),
        "pex" => pex::read(bytes),
        "esp" | "esm" | "esl" => game::plugin(name, bytes),
        "bsa" | "ba2" => game::archive(bytes),
        "wav" => media::wav(bytes),
        "xwm" => media::xwm(bytes),
        "fuz" => media::fuz(bytes),
        "lip" => media::lip(bytes),
        "bik" | "bk2" => media::bik(bytes),
        "swf" | "gfx" => media::swf(bytes),
        "seq" => game::seq(bytes),
        "tri" => game::tri(bytes),
        "egm" | "egt" | "fg" => game::facegen(&e, bytes),
        "strings" | "dlstrings" | "ilstrings" => game::strings(&e, bytes),
        "dll" | "exe" => game::pe(bytes),
        "ttf" | "otf" | "fnt" => game::font(&e, bytes),
        "ess" | "fos" => save::read(bytes),
        "ogg" | "mp3" | "flac" | "opus" | "m4a" => media::play(&e, bytes),
        "bgsm" | "bgem" => game::material(&e, bytes),
        "zip" | "7z" | "rar" => game::packed(&e, bytes),
        "pdf" | "psd" => game::document(&e, bytes),
        _ => by_magic(&e, bytes),
    };
    p.fact("Размер", human(bytes.len() as u64));
    if p.kind == Kind::Binary && p.hex.is_none() {
        binary_tail(&mut p, bytes);
    }
    p
}

fn by_magic(e: &str, b: &[u8]) -> Preview {
    match b.get(..4) {
        Some(b"DDS ") => image::dds(b),
        Some(b"BSA\0") | Some(b"BTDX") => game::archive(b),
        Some(b"TES4") => game::plugin("x.esp", b),
        Some(b"RIFF") => media::wav(b),
        Some(b"FUZE") => media::fuz(b),
        Some([0xFA, 0x57, 0xC0, 0xDE]) => pex::read(b),
        Some([0x57, 0xE0, 0xE0, 0x57]) => hkx::read(b),
        Some(b"Game") => nif::read(b),
        Some([0x89, b'P', b'N', b'G']) => image::raster("png", b),
        Some(b"TESV") | Some(b"FO4_") => save::read(b),
        Some(b"OggS") | Some(b"fLaC") => media::play(e, b),
        Some(b"BGSM") | Some(b"BGEM") => game::material(e, b),
        Some([b'P', b'K', 3, 4]) | Some([b'7', b'z', 0xBC, 0xAF]) | Some(b"Rar!") => game::packed(e, b),
        _ => match text::detect(e, b) {
            Some(p) => p,
            None => Preview::new(
                Kind::Binary,
                if e.is_empty() {
                    "файл".to_string()
                } else {
                    format!(".{e}")
                },
            ),
        },
    }
}

/// Hex of the head and printable strings, for binaries.
pub(crate) fn binary_tail(p: &mut Preview, b: &[u8]) {
    p.hex = Some(hex(&b[..b.len().min(512)]));
    let s = printable_strings(b, 6, 80);
    p.section("Строки в файле", s);
}

pub(crate) fn hex(b: &[u8]) -> String {
    let mut out = String::new();
    for (i, chunk) in b.chunks(16).enumerate() {
        out.push_str(&format!("{:08x}  ", i * 16));
        for (j, x) in chunk.iter().enumerate() {
            out.push_str(&format!("{x:02x} "));
            if j == 7 {
                out.push(' ');
            }
        }
        for _ in chunk.len()..16 {
            out.push_str("   ");
        }
        out.push_str(" |");
        out.extend(
            chunk
                .iter()
                .map(|&c| if (0x20..0x7f).contains(&c) { c as char } else { '.' }),
        );
        out.push_str("|\n");
    }
    out
}

/// Runs of printable ASCII at least `min` long, at most `max` of them.
pub(crate) fn printable_strings(b: &[u8], min: usize, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = Vec::new();
    for &c in b.iter().chain(std::iter::once(&0)) {
        if (0x20..0x7f).contains(&c) {
            cur.push(c);
        } else {
            if cur.len() >= min {
                out.push(String::from_utf8_lossy(&cur).into_owned());
                if out.len() >= max {
                    break;
                }
            }
            cur.clear();
        }
    }
    out
}

/// Game paths mentioned in the bytes (`textures\a\b.dds`), for links of binaries that name files.
pub(crate) fn mentioned_paths(b: &[u8], exts: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in printable_strings(b, 5, 100_000) {
        for tok in s.split(|c: char| c.is_whitespace() || "\"'=;,()<>|".contains(c)) {
            let l = tok.to_ascii_lowercase();
            if l.len() > 4 && exts.iter().any(|e| l.ends_with(e)) {
                let p = crate::normalize_path(tok);
                if !out.contains(&p) && out.len() < LIST_LIMIT {
                    out.push(p);
                }
            }
        }
    }
    out
}

pub(crate) fn human(n: u64) -> String {
    const U: [&str; 5] = ["Б", "КБ", "МБ", "ГБ", "ТБ"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} Б")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

pub(crate) fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// A bounds-checked little-endian cursor: every read is `None` past the end.
pub(crate) struct Cur<'a> {
    pub b: &'a [u8],
    pub at: usize,
}

impl<'a> Cur<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Self { b, at: 0 }
    }
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }
    pub fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }
    pub fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|s| u16::from_le_bytes([s[0], s[1]]))
    }
    pub fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    pub fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .map(|s| u64::from_le_bytes(s.try_into().unwrap_or([0; 8])))
    }
    pub fn u16be(&mut self) -> Option<u16> {
        self.take(2).map(|s| u16::from_be_bytes([s[0], s[1]]))
    }
    pub fn u32be(&mut self) -> Option<u32> {
        self.take(4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    pub fn u64be(&mut self) -> Option<u64> {
        self.take(8)
            .map(|s| u64::from_be_bytes(s.try_into().unwrap_or([0; 8])))
    }
    /// Bytes as text (UTF-8, else Windows-1252-ish).
    pub fn text(&mut self, n: usize) -> Option<String> {
        self.take(n).map(lossy)
    }
}

/// Text from bytes: UTF-8 when valid, else one byte one char (the game's files are Windows-1252 in practice).
pub(crate) fn lossy(b: &[u8]) -> String {
    let b = match b.iter().position(|&c| c == 0) {
        Some(i) => &b[..i],
        None => b,
    };
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| text::cp1252(c)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_binaries_get_hex_and_strings_and_text_is_found_by_content() {
        let mut b = vec![0u8, 1, 2, 3];
        b.extend_from_slice(b"hello textures\\a.dds world");
        let p = inspect("x.bin", &b);
        assert_eq!(p.kind, Kind::Binary);
        assert!(p.hex.as_deref().unwrap().starts_with("00000000  00 01 02 03"));
        assert_eq!(p.sections[0].lines[0], "hello textures\\a.dds world");
        let p = inspect("readme", b"plain words\nand more\n");
        assert_eq!(p.kind, Kind::Text);
        assert_eq!(mentioned_paths(&b, &[".dds"]), ["textures/a.dds"]);
    }

    #[test]
    fn base64_and_sizes() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(human(512), "512 Б");
        assert_eq!(human(1536), "1.5 КБ");
    }

    /// Real files from a folder (`SAT_PREVIEW_DIR`): every file previews without a panic; prints the format lines.
    #[test]
    #[ignore]
    fn real_files() {
        let Ok(dir) = std::env::var("SAT_PREVIEW_DIR") else {
            return;
        };
        fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() { walk(&p, out) } else { out.push(p) }
            }
        }
        let mut files = Vec::new();
        walk(std::path::Path::new(&dir), &mut files);
        for f in files {
            let b = std::fs::read(&f).unwrap();
            let t = std::time::Instant::now();
            let p = inspect(&f.to_string_lossy(), &b);
            println!(
                "{:?} {} [{:?}] {} ms pic={} notes={:?}",
                p.kind,
                p.format,
                f.file_name().unwrap(),
                t.elapsed().as_millis(),
                p.picture.as_ref().map_or(0, |x| x.data.len()),
                p.notes
            );
        }
    }

    #[test]
    fn the_cursor_never_reads_past_the_end() {
        let mut c = Cur::new(&[1, 0, 0]);
        assert_eq!(c.u16(), Some(1));
        assert_eq!(c.u32(), None);
        assert_eq!(c.u8(), Some(0));
        assert_eq!(c.u8(), None);
    }
}
