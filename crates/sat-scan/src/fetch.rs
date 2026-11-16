//! One copy of a file, read for viewing: loose in its mod's folder or an entry of its archive. @sat:scan.fetch
//!
//! Owns: finding a copy on disk (the mod folder, or the overwrite folder) and turning it into a preview
//! (`@sat:formats.preview`): small files are read whole, big ones by their head, archives by their manifest from
//! the file itself, archive entries unpacked one at a time (`@sat:formats.archive-data.one` — BA2 textures become
//! whole DDS files). Read-only.
//! Not here: which copy (`@sat:app.preview`), the formats (`@sat:formats.preview`).

use std::io::Read;
use std::path::{Path, PathBuf};

use sat_formats::preview::{self, Preview};

/// Files up to this size are read whole; bigger ones by their head.
pub const WHOLE: u64 = 96 << 20;
/// The head of a bigger file.
pub const HEAD: usize = 4 << 20;

/// Where mods live.
#[derive(Debug, Clone)]
pub struct Folders {
    pub mods: PathBuf,
    pub overwrite: PathBuf,
}

impl Folders {
    fn mod_dir(&self, m: &str) -> PathBuf {
        if m == sat_core::session::OVERWRITE {
            self.overwrite.clone()
        } else {
            self.mods.join(m)
        }
    }
}

/// A file on disk by its game path, matching case-insensitively folder by folder (Windows paths in a Linux test
/// tree, or a path written in another case).
fn find(dir: &Path, rel: &str) -> Option<PathBuf> {
    let direct = dir.join(rel);
    if direct.is_file() {
        return Some(direct);
    }
    let mut at = dir.to_path_buf();
    for part in rel.split(['/', '\\']).filter(|s| !s.is_empty()) {
        let next = std::fs::read_dir(&at)
            .ok()?
            .flatten()
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?;
        at = next.path();
    }
    at.is_file().then_some(at)
}

/// Preview of mod `m`'s copy of `rel`: loose, or from archive `archive` (a file in the mod's folder).
pub fn preview(f: &Folders, m: &str, rel: &str, archive: Option<&str>) -> Option<Preview> {
    let dir = f.mod_dir(m);
    match archive {
        None => {
            let path = find(&dir, rel)?;
            let size = std::fs::metadata(&path).ok()?.len();
            let ext = rel.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            if (ext == "bsa" || ext == "ba2") && size > WHOLE {
                let file = std::fs::File::open(&path).ok()?;
                return Some(preview::inspect_archive(std::io::BufReader::new(file), size));
            }
            if size > WHOLE {
                let mut head = Vec::with_capacity(HEAD);
                std::fs::File::open(&path)
                    .ok()?
                    .take(HEAD as u64)
                    .read_to_end(&mut head)
                    .ok()?;
                return Some(preview::inspect_head(rel, &head, size));
            }
            let bytes = std::fs::read(&path).ok()?;
            Some(preview::inspect(rel, &bytes))
        }
        Some(a) => {
            let path = find(&dir, a)?;
            let file = std::fs::File::open(&path).ok()?;
            match sat_formats::archive_data::read_one(std::io::BufReader::new(file), rel) {
                Ok(Some(bytes)) => {
                    let mut p = preview::inspect(rel, &bytes);
                    p.facts.push(("Из архива".into(), a.to_string()));
                    Some(p)
                }
                Ok(None) => Some(Preview {
                    format: format!("в архиве {a}"),
                    notes: vec![
                        "запись не распаковывается (BA2 со сжатием LZ4 или запись больше 2 ГБ)".into(),
                    ],
                    ..Default::default()
                }),
                Err(e) => Some(Preview {
                    notes: vec![format!("архив {a} не читается: {e}")],
                    ..Default::default()
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loose_copy_is_found_in_any_case_and_an_archive_entry_is_unpacked() {
        let t = tempfile::tempdir().unwrap();
        let m = t.path().join("mods/A/Textures");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(m.join("Note.TXT"), b"hello\n").unwrap();
        let f = Folders {
            mods: t.path().join("mods"),
            overwrite: t.path().join("overwrite"),
        };
        let p = preview(&f, "A", "textures/note.txt", None).expect("found");
        assert_eq!(p.text.unwrap().body, "hello\n");
        assert!(preview(&f, "A", "textures/none.txt", None).is_none());
        assert!(preview(&f, "B", "x", None).is_none());
    }
}
