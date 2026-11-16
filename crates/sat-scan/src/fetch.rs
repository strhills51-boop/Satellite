//! One copy of a file, read for viewing: loose in its mod's folder or an entry of its archive. @sat:scan.fetch
//!
//! Owns: finding a copy on disk (the mod folder, or the overwrite folder) and turning it into a preview
//! (`@sat:formats.preview`): small files are read whole, big ones by their head, archives by their manifest from
//! the file itself, archive entries unpacked one at a time (`@sat:formats.archive-data.one` — BA2 textures become
//! whole DDS files); places outside the build (`Disk`: the game, the profile, the instance, the game's documents)
//! listed folder by folder, their archives as folders of entries. Read-only; a path never leaves its place.
//! Not here: which copy (`@sat:app.preview`), the formats (`@sat:formats.preview`).

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sat_formats::preview::{self, DiskEntry, Place, Preview};

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

/// A file or folder on disk by a relative path, matching case-insensitively folder by folder (Windows paths in a
/// Linux test tree, or a path written in another case). `..` never resolves.
fn resolve(dir: &Path, rel: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = rel
        .split(['/', '\\'])
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    if parts.contains(&"..") {
        return None;
    }
    let direct = parts.iter().fold(dir.to_path_buf(), |p, s| p.join(s));
    if direct.exists() {
        return Some(direct);
    }
    let mut at = dir.to_path_buf();
    for part in parts {
        let next = std::fs::read_dir(&at)
            .ok()?
            .flatten()
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?;
        at = next.path();
    }
    Some(at)
}

// @sat:scan.fetch.find
/// A file under `dir` by a relative path, case-insensitively folder by folder; `..` never resolves.
pub fn find(dir: &Path, rel: &str) -> Option<PathBuf> {
    resolve(dir, rel).filter(|p| p.is_file())
}

fn is_archive_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".bsa") || n.ends_with(".ba2")
}

/// Preview of a file on disk: small files whole, big ones by their head, big archives by their manifest.
fn preview_file(path: &Path, name: &str) -> Option<Preview> {
    let size = std::fs::metadata(path).ok()?.len();
    if is_archive_name(name) && size > WHOLE {
        let file = std::fs::File::open(path).ok()?;
        return Some(preview::inspect_archive(std::io::BufReader::new(file), size));
    }
    if size > WHOLE {
        let mut head = Vec::with_capacity(HEAD);
        std::fs::File::open(path)
            .ok()?
            .take(HEAD as u64)
            .read_to_end(&mut head)
            .ok()?;
        return Some(preview::inspect_head(name, &head, size));
    }
    let bytes = std::fs::read(path).ok()?;
    Some(preview::inspect(name, &bytes))
}

/// An entry of archive `a` (a file on disk, `label` for notes).
fn preview_entry(a: &Path, label: &str, rel: &str) -> Preview {
    let Ok(file) = std::fs::File::open(a) else {
        return Preview {
            notes: vec![format!("архив {label} не открывается")],
            ..Default::default()
        };
    };
    match sat_formats::archive_data::read_one(std::io::BufReader::new(file), rel) {
        Ok(Some(bytes)) => {
            let mut p = preview::inspect(rel, &bytes);
            p.facts.push(("Из архива".into(), label.to_string()));
            p
        }
        Ok(None) => Preview {
            format: format!("в архиве {label}"),
            notes: vec!["запись не распаковывается (BA2 со сжатием LZ4 или запись больше 2 ГБ)".into()],
            ..Default::default()
        },
        Err(e) => Preview {
            notes: vec![format!("архив {label} не читается: {e}")],
            ..Default::default()
        },
    }
}

/// Preview of mod `m`'s copy of `rel`: loose, or from archive `archive` (a file in the mod's folder).
pub fn preview(f: &Folders, m: &str, rel: &str, archive: Option<&str>) -> Option<Preview> {
    let dir = f.mod_dir(m);
    match archive {
        None => preview_file(&find(&dir, rel)?, rel),
        Some(a) => Some(preview_entry(&find(&dir, a)?, a, rel)),
    }
}

/// An archive's entries: (path, size).
type Manifest = Arc<Vec<(String, u64)>>;

// @sat:scan.fetch.disk
/// Places outside the build, read folder by folder; archives are folders of their entries (manifests cached).
#[derive(Debug, Default)]
pub struct Disk {
    places: Vec<(Place, PathBuf)>,
    manifests: Mutex<HashMap<PathBuf, Manifest>>,
}

impl Disk {
    /// Places that exist on disk, in the order given: (id, label, folder).
    pub fn new(places: impl IntoIterator<Item = (&'static str, &'static str, PathBuf)>) -> Self {
        let mut seen: Vec<PathBuf> = Vec::new();
        let places = places
            .into_iter()
            .filter(|(_, _, p)| p.is_dir())
            .filter(|(_, _, p)| {
                let k = std::path::absolute(p).unwrap_or_else(|_| p.clone());
                let new = !seen.contains(&k);
                seen.push(k);
                new
            })
            .map(|(id, label, p)| {
                (
                    Place {
                        id: id.into(),
                        label: label.into(),
                        root: p.display().to_string(),
                    },
                    p,
                )
            })
            .collect();
        Self {
            places,
            manifests: Mutex::default(),
        }
    }

    pub fn places(&self) -> Vec<Place> {
        self.places.iter().map(|(p, _)| p.clone()).collect()
    }

    fn root(&self, id: &str) -> Option<&Path> {
        self.places
            .iter()
            .find(|(p, _)| p.id == id)
            .map(|(_, r)| r.as_path())
    }

    /// `rel` on disk, or split at an archive: (archive file, path inside, lower case).
    fn locate(&self, id: &str, rel: &str) -> Option<(PathBuf, Option<String>)> {
        let root = self.root(id)?;
        let parts: Vec<&str> = rel.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
        for k in 0..parts.len() {
            if is_archive_name(parts[k]) {
                let at = resolve(root, &parts[..=k].join("/"))?;
                if at.is_file() {
                    return Some((at, Some(parts[k + 1..].join("/").to_lowercase())));
                }
            }
        }
        Some((resolve(root, rel)?, None))
    }

    fn manifest(&self, a: &Path) -> Option<Manifest> {
        if let Some(m) = self.manifests.lock().ok()?.get(a) {
            return Some(m.clone());
        }
        let file = std::fs::File::open(a).ok()?;
        let m = sat_formats::archive::read_archive(std::io::BufReader::new(file)).ok()?;
        let v = Arc::new(
            m.entries
                .into_iter()
                .map(|e| (e.path, e.size))
                .collect::<Vec<_>>(),
        );
        self.manifests.lock().ok()?.insert(a.to_path_buf(), v.clone());
        Some(v)
    }

    /// The children of folder `rel` of place `id`: folders first, then files.
    pub fn list(&self, id: &str, rel: &str) -> Option<Vec<DiskEntry>> {
        let base = rel.trim_matches(['/', '\\']).replace('\\', "/");
        let join = |n: &str| {
            if base.is_empty() {
                n.to_string()
            } else {
                format!("{base}/{n}")
            }
        };
        let (at, inner) = self.locate(id, rel)?;
        let mut out = Vec::new();
        match inner {
            Some(inner) => {
                let m = self.manifest(&at)?;
                let prefix = if inner.is_empty() {
                    String::new()
                } else {
                    format!("{inner}/")
                };
                let mut dirs: Vec<(String, u32)> = Vec::new();
                for (p, size) in m.iter() {
                    let Some(rest) = p.strip_prefix(&prefix) else {
                        continue;
                    };
                    match rest.split_once('/') {
                        Some((d, _)) => match dirs.iter_mut().find(|x| x.0 == d) {
                            Some(x) => x.1 += 1,
                            None => dirs.push((d.to_string(), 1)),
                        },
                        None => out.push(DiskEntry {
                            name: rest.to_string(),
                            path: join(rest),
                            size: *size,
                            inside: true,
                            files: 1,
                            ..Default::default()
                        }),
                    }
                }
                out.extend(dirs.into_iter().map(|(d, n)| DiskEntry {
                    path: join(&d),
                    name: d,
                    dir: true,
                    inside: true,
                    files: n,
                    ..Default::default()
                }));
            }
            None => {
                for e in std::fs::read_dir(&at).ok()?.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let Ok(md) = e.metadata() else { continue };
                    let archive = md.is_file() && is_archive_name(&name);
                    out.push(DiskEntry {
                        path: join(&name),
                        dir: md.is_dir() || archive,
                        size: if md.is_file() { md.len() } else { 0 },
                        archive,
                        files: u32::from(md.is_file()),
                        name,
                        ..Default::default()
                    });
                }
            }
        }
        out.sort_by_key(|e| (!e.dir, e.name.to_lowercase()));
        Some(out)
    }

    // @sat:scan.fetch.game
    /// A data path from the game's `Data` (place `game`): loose, else the first of its archives that has it.
    pub fn game(&self, rel: &str) -> Option<Preview> {
        let root = self.root("game")?;
        let data = resolve(root, "Data").filter(|d| d.is_dir())?;
        let want = rel.trim_matches('/').replace('\\', "/").to_lowercase();
        if let Some(f) = find(&data, &want) {
            return preview_file(&f, &want);
        }
        let mut archives: Vec<PathBuf> = std::fs::read_dir(&data)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_archive_name(&p.to_string_lossy()))
            .collect();
        archives.sort();
        for a in archives {
            if self
                .manifest(&a)
                .is_some_and(|m| m.iter().any(|(p, _)| *p == want))
            {
                let label = a.file_name()?.to_string_lossy().into_owned();
                return Some(preview_entry(&a, &label, &want));
            }
        }
        None
    }

    /// Preview of file `rel` of place `id` (an archive's entry too).
    pub fn read(&self, id: &str, rel: &str) -> Option<Preview> {
        let (at, inner) = self.locate(id, rel)?;
        match inner {
            Some(inner) if !inner.is_empty() => {
                let label = at.file_name()?.to_string_lossy().into_owned();
                Some(preview_entry(&at, &label, &inner))
            }
            _ => at.is_file().then(|| preview_file(&at, rel)).flatten(),
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

    #[test]
    fn places_list_folders_and_archives_as_folders_and_never_leave_the_place() {
        let t = tempfile::tempdir().unwrap();
        let g = t.path().join("game");
        std::fs::create_dir_all(g.join("Data")).unwrap();
        std::fs::write(g.join("SkyrimSE.exe"), b"MZ").unwrap();
        std::fs::write(t.path().join("secret.txt"), b"no").unwrap();
        std::fs::write(
            g.join("Data/Skyrim - Misc.bsa"),
            sat_formats::archive::fixtures::bsa_v105(&[
                ("meshes\\a", &["b.nif", "c.nif"]),
                ("meshes", &["d.txt"]),
            ]),
        )
        .unwrap();
        let d = Disk::new([
            ("game", "Игра", g.clone()),
            ("none", "—", t.path().join("absent")),
        ]);
        assert_eq!(d.places().len(), 1);
        let root = d.list("game", "").unwrap();
        assert_eq!(
            root.iter().map(|e| (e.name.as_str(), e.dir)).collect::<Vec<_>>(),
            [("Data", true), ("SkyrimSE.exe", false)]
        );
        let data = d.list("game", "data").unwrap();
        assert!(data[0].archive && data[0].dir, "{data:?}");
        let inside = d.list("game", &data[0].path).unwrap();
        assert_eq!(inside[0].name, "meshes");
        let meshes = d.list("game", &inside[0].path).unwrap();
        assert_eq!(
            meshes
                .iter()
                .map(|e| (e.name.as_str(), e.dir, e.files))
                .collect::<Vec<_>>(),
            [("a", true, 2), ("d.txt", false, 1)]
        );
        assert!(d.read("game", "../secret.txt").is_none());
        assert!(d.read("game", "Data/../../secret.txt").is_none());
        assert!(d.read("game", "SkyrimSE.exe").is_some());
    }
}
