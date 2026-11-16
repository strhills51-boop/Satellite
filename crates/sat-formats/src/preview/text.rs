//! Text files for the preview: encodings, languages, what a config or a script source says. @sat:formats.preview.text
//!
//! Owns: text detection, decoding (UTF-8, UTF-16, Windows-1252), ini sections, JSON layout, XML root, Papyrus source outline.
//! Not here: the model and dispatch (`@sat:formats.preview`), ini editing (`@sat:formats.ini`).

use super::{Kind, Preview, TEXT_LIMIT, TextBody};

/// Extensions that are text whatever their bytes.
const TEXT: &[(&str, &str)] = &[
    ("ini", "ini"),
    ("json", "json"),
    ("jsonc", "json"),
    ("jslot", "json"),
    ("ckm", "text"),
    ("cgid", "text"),
    ("pas", "text"),
    ("xml", "xml"),
    ("psc", "papyrus"),
    ("toml", "toml"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("txt", "text"),
    ("log", "text"),
    ("md", "markdown"),
    ("csv", "csv"),
    ("cfg", "ini"),
    ("lst", "text"),
    ("html", "xml"),
    ("htm", "xml"),
    ("xsd", "xml"),
    ("bat", "text"),
    ("cmd", "text"),
    ("ps1", "text"),
    ("lua", "text"),
    ("py", "text"),
    ("js", "text"),
    ("hlsl", "text"),
    ("fx", "text"),
    ("fxh", "text"),
    ("glsl", "text"),
    ("ssf", "text"),
    ("modgroups", "text"),
    ("ws", "text"),
    ("swl", "text"),
];

/// The language of a text extension.
pub(super) fn lang_of(e: &str) -> Option<&'static str> {
    TEXT.iter().find(|(x, _)| *x == e).map(|(_, l)| *l)
}

/// Windows-1252 → char for bytes that are not ASCII.
pub(crate) fn cp1252(c: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}',
        '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    match c {
        0x80..=0x9f => HIGH[(c - 0x80) as usize],
        _ => c as char,
    }
}

/// Bytes → text and the encoding's name.
fn decode(b: &[u8]) -> (String, &'static str) {
    if let Some(r) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (String::from_utf8_lossy(r).into_owned(), "UTF-8 (BOM)");
    }
    if b.starts_with(&[0xFF, 0xFE]) || b.starts_with(&[0xFE, 0xFF]) {
        let le = b[0] == 0xFF;
        let u: Vec<u16> = b[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                if le {
                    u16::from_le_bytes(*c)
                } else {
                    u16::from_be_bytes(*c)
                }
            })
            .collect();
        return (
            String::from_utf16_lossy(&u),
            if le { "UTF-16 LE" } else { "UTF-16 BE" },
        );
    }
    match std::str::from_utf8(b) {
        Ok(s) => (s.to_string(), "UTF-8"),
        Err(_) => (b.iter().map(|&c| cp1252(c)).collect(), "Windows-1252"),
    }
}

/// Looks like text: no NUL in the head (UTF-16 has a BOM), few control bytes.
fn looks_text(b: &[u8]) -> bool {
    if b.starts_with(&[0xFF, 0xFE]) || b.starts_with(&[0xFE, 0xFF]) {
        return true;
    }
    let head = &b[..b.len().min(4096)];
    if head.is_empty() || head.contains(&0) {
        return false;
    }
    let ctl = head
        .iter()
        .filter(|&&c| c < 0x09 || (0x0e..0x20).contains(&c))
        .count();
    ctl * 100 < head.len()
}

/// A text preview when the extension says text, or the bytes look like it.
pub(super) fn detect(e: &str, b: &[u8]) -> Option<Preview> {
    let lang = lang_of(e);
    if lang.is_none() && !looks_text(b) {
        return None;
    }
    Some(read(lang.unwrap_or("text"), e, b))
}

fn read(lang: &str, e: &str, b: &[u8]) -> Preview {
    let (mut s, enc) = decode(b);
    let mut p = Preview::new(
        Kind::Text,
        if e.is_empty() {
            "текст".to_string()
        } else {
            format!(".{e} · {lang}")
        },
    );
    p.fact("Кодировка", enc);
    p.fact("Строк", s.lines().count());
    match lang {
        "ini" => ini(&mut p, &s),
        "json" => {
            match serde_json::from_str::<serde_json::Value>(&s) {
                Ok(v) => {
                    p.fact("JSON", "корректный");
                    // minified files read better laid out
                    if s.lines().count() <= 2 && s.len() > 160 {
                        s = serde_json::to_string_pretty(&v).unwrap_or(s);
                    }
                    if let serde_json::Value::Object(o) = &v {
                        p.section("Ключи верхнего уровня", o.keys().cloned().collect());
                    }
                }
                Err(err) => p.note(format!("JSON не разбирается: {err}")),
            }
        }
        "xml" => match roxmltree::Document::parse(&s) {
            Ok(d) => {
                let root = d.root_element();
                p.fact("Корень", root.tag_name().name());
                p.fact("Элементов", d.descendants().filter(|n| n.is_element()).count());
            }
            Err(err) => p.note(format!("XML не разбирается: {err}")),
        },
        "papyrus" => papyrus(&mut p, &s),
        _ => {}
    }
    for l in super::mentioned_paths(
        s.as_bytes(),
        &[
            ".nif", ".dds", ".hkx", ".pex", ".esp", ".esm", ".esl", ".wav", ".xwm",
        ],
    ) {
        p.link(&l);
    }
    let truncated = s.len() > TEXT_LIMIT;
    if truncated {
        let mut cut = TEXT_LIMIT;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    p.text = Some(TextBody {
        lang: lang.to_string(),
        body: s,
        truncated,
    });
    p
}

fn ini(p: &mut Preview, s: &str) {
    let mut sections: Vec<String> = Vec::new();
    let mut keys = 0usize;
    let mut cur = String::from("(без раздела)");
    let mut n = 0usize;
    let flush = |sections: &mut Vec<String>, cur: &str, n: usize| {
        if n > 0 {
            sections.push(format!("[{cur}] — {n}"));
        }
    };
    for line in s.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            flush(&mut sections, &cur, n);
            cur = t[1..t.len() - 1].to_string();
            n = 0;
        } else if !t.is_empty() && !t.starts_with(';') && !t.starts_with('#') && t.contains('=') {
            n += 1;
            keys += 1;
        }
    }
    flush(&mut sections, &cur, n);
    p.fact("Ключей", keys);
    p.section("Разделы", sections);
}

fn papyrus(p: &mut Preview, s: &str) {
    let mut funcs = Vec::new();
    let mut props = Vec::new();
    for line in s.lines() {
        let t = line.trim();
        let l = t.to_ascii_lowercase();
        if l.starts_with("scriptname ") {
            let mut it = t.split_whitespace().skip(1);
            if let Some(n) = it.next() {
                p.fact("Скрипт", n);
            }
            let rest: Vec<&str> = it.collect();
            if let Some(i) = rest.iter().position(|w| w.eq_ignore_ascii_case("extends"))
                && let Some(parent) = rest.get(i + 1)
            {
                p.fact("Наследует", parent);
                p.link(&format!("scripts/{parent}.pex"));
            }
        } else if l.contains(" property ") && !l.starts_with(';') {
            props.push(t.to_string());
        } else if (l.starts_with("function ") || l.starts_with("event ") || l.contains(" function "))
            && !l.starts_with(';')
        {
            funcs.push(t.to_string());
        }
    }
    p.section("Свойства", props);
    p.section("Функции и события", funcs);
}

#[cfg(test)]
mod tests {
    use super::super::{Kind, inspect};

    #[test]
    fn configs_and_sources_say_what_is_in_them() {
        let p = inspect("a.ini", b"; c\n[General]\nbX=1\nfY=2\n[Display]\nz=3\n");
        assert_eq!(p.kind, Kind::Text);
        assert_eq!(p.sections[0].lines, ["[General] — 2", "[Display] — 1"]);
        let p = inspect("config.json", br#"{"priority":10,"conditions":[{"condition":"IsFemale","negated":false}],"name":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#);
        assert!(
            p.text.as_ref().unwrap().body.contains("\n  \"priority\": 10"),
            "minified JSON laid out"
        );
        assert_eq!(p.sections[0].lines, ["conditions", "name", "priority"]);
        let p = inspect("s.psc", b"Scriptname MyQuest extends Quest Conditional\nInt Property Count Auto\nFunction Go()\nEndFunction\n");
        assert!(p.facts.contains(&("Наследует".into(), "Quest".into())));
        assert_eq!(p.links, ["scripts/quest.pex"]);
        let p = inspect("t.txt", &[0xFF, 0xFE, b'h', 0, b'i', 0]);
        assert_eq!(p.text.unwrap().body, "hi");
        let p = inspect("w.txt", &[b'c', b'a', b'f', 0xE9]);
        assert_eq!(p.text.unwrap().body, "café");
    }
}
