//! Installing a mod from an archive into its MO2 folder. @sat:mo2.install
//!
//! Owns: reading an archive's layout (where the game's `Data` content starts, whether it needs an installer),
//! the facts of a mod before it is installed (its files as the archive lists them), and extracting it into a
//! folder safely (no path leaves the folder).
//! Not here: when to install (`@sat:intent`, `@sat:sync.plan.installs`), the commit around it (`@sat:mo2.commit`).
//!
//! Formats: zip. 7z and rar — next (most Nexus archives are 7z). FOMOD installers: recognised and refused for now
//! (the installer's choices will be part of the intent).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Component, Path, PathBuf};

use sat_core::facts::{LooseDir, LooseFile, ModFacts};

/// Top folders of a game's `Data` (Bethesda games; lowercase).
const DATA_DIRS: &[&str] = &[
    "meshes",
    "textures",
    "scripts",
    "interface",
    "sound",
    "music",
    "seq",
    "skse",
    "strings",
    "shadersfx",
    "lodsettings",
    "grass",
    "video",
    "materials",
    "calientetools",
    "nemesis_engine",
    "fonts",
    "dyndolod",
    "netscriptframework",
    "source",
    "tools",
    "facegen",
    "distantlod",
    "lod",
    "trees",
    "dialogueviews",
    "f4se",
    "mcm",
    "platform",
    "shaders",
    "vis",
];
/// Files that sit in `Data` itself.
const DATA_EXTS: &[&str] = &["esp", "esm", "esl", "bsa", "ba2", "ini"];

// @sat:mo2.install.layout
/// What an archive holds, seen from the mod folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveLayout {
    /// The prefix inside the archive that becomes the mod folder (`""`, or `Foo/` when everything is in one
    /// folder, or `Foo/Data/`).
    pub root: String,
    /// Files under the root: path relative to the mod folder (`/`-separated) and size.
    pub files: Vec<(String, u64)>,
    /// A FOMOD installer (`fomod/ModuleConfig.xml`): choices are needed.
    pub fomod: bool,
}

fn norm(name: &str) -> Option<String> {
    let n = name.replace('\\', "/");
    if n.starts_with('/') || n.split('/').any(|c| c == ".." || c.contains(':')) {
        return None;
    }
    Some(n)
}

fn is_data_level(paths: &[&str]) -> bool {
    paths.iter().any(|p| match p.split_once('/') {
        Some((top, _)) => DATA_DIRS.contains(&top.to_lowercase().as_str()),
        None => p
            .rsplit_once('.')
            .is_some_and(|(_, e)| DATA_EXTS.contains(&e.to_lowercase().as_str())),
    })
}

/// Picks the root: the shallowest level that looks like `Data`; else a single top folder is stripped.
fn find_root(paths: &[String]) -> String {
    let mut prefix = String::new();
    for _ in 0..3 {
        let under: Vec<&str> = paths
            .iter()
            .filter_map(|p| p.strip_prefix(prefix.as_str()))
            .collect();
        if is_data_level(&under) {
            return prefix;
        }
        let tops: std::collections::BTreeSet<&str> = under
            .iter()
            .filter_map(|p| p.split_once('/').map(|x| x.0))
            .collect();
        // a `Data` folder is the root; else one folder (beside readmes) is the mod's wrapper
        if let Some(d) = tops.iter().find(|t| t.eq_ignore_ascii_case("data")) {
            return format!("{prefix}{d}/");
        }
        match tops.len() {
            1 => prefix = format!("{prefix}{}/", tops.into_iter().next().unwrap()),
            _ => return prefix,
        }
    }
    prefix
}

fn open(path: &Path) -> Result<zip::ZipArchive<BufReader<File>>, String> {
    let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    zip::ZipArchive::new(BufReader::new(f)).map_err(|e| {
        format!(
            "{}: not a zip archive ({e}); 7z and rar come next",
            path.display()
        )
    })
}

/// Reads an archive's layout without extracting it.
pub fn inspect(path: &Path) -> Result<ArchiveLayout, String> {
    let mut z = open(path)?;
    let mut entries = Vec::with_capacity(z.len());
    for i in 0..z.len() {
        let e = z
            .by_index_raw(i)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if e.is_dir() {
            continue;
        }
        let raw = e
            .name()
            .map_err(|x| format!("{}: {x}", path.display()))?
            .into_owned();
        let name = norm(&raw).ok_or_else(|| format!("{}: unsafe path {raw:?}", path.display()))?;
        entries.push((name, e.size()));
    }
    let paths: Vec<String> = entries.iter().map(|(n, _)| n.clone()).collect();
    let fomod = paths.iter().any(|p| {
        let l = p.to_lowercase();
        l == "fomod/moduleconfig.xml" || l.ends_with("/fomod/moduleconfig.xml") && l.matches('/').count() <= 2
    });
    let root = find_root(&paths);
    let files = entries
        .into_iter()
        .filter_map(|(n, s)| n.strip_prefix(root.as_str()).map(|r| (r.to_string(), s)))
        .filter(|(r, _)| !r.is_empty() && !r.to_lowercase().starts_with("fomod/"))
        .collect();
    Ok(ArchiveLayout { root, files, fomod })
}

// @sat:mo2.install.facts
/// The facts of a mod as its archive lists them: conflicts and metrics are known before it is installed.
pub fn facts_of(name: &str, layout: &ArchiveLayout) -> ModFacts {
    let mut dirs: BTreeMap<String, Vec<LooseFile>> = BTreeMap::new();
    let mut f = ModFacts {
        name: name.to_string(),
        ..ModFacts::default()
    };
    for (path, size) in &layout.files {
        let (dir, file) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
        dirs.entry(dir.to_lowercase()).or_default().push(LooseFile {
            name: file.to_lowercase(),
            size: *size,
            vram: 0,
        });
        f.metrics.files += 1;
        f.metrics.bytes += size;
        let ext = file
            .rsplit_once('.')
            .map(|(_, e)| e.to_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "esp" | "esm" | "esl" if !dir.contains('/') && dir.is_empty() => f.metrics.plugins += 1,
            "bsa" | "ba2" if dir.is_empty() => f.metrics.archives += 1,
            "dds" => f.metrics.dds += 1,
            "nif" => f.metrics.nif += 1,
            "pex" => f.metrics.pex += 1,
            "hkx" => f.metrics.hkx += 1,
            "dll" => f.metrics.dll += 1,
            _ => {}
        }
        *f.metrics.by_extension.entry(ext).or_default() += 1;
    }
    f.dirs = dirs
        .into_iter()
        .map(|(rel, files)| LooseDir { rel, files })
        .collect();
    f
}

// @sat:mo2.install.extract
/// Extracts the archive's root into `target` (which must not exist), through a temp folder beside it and a
/// rename, and writes `meta.ini` with the installation file (MO2 shows where the mod came from). On any error
/// nothing is left behind. Returns the number of files.
pub fn install(archive: &Path, target: &Path) -> Result<usize, String> {
    if target.exists() {
        return Err(format!("{} already exists", target.display()));
    }
    let layout = inspect(archive)?;
    if layout.fomod {
        return Err(format!(
            "{}: a FOMOD installer — its choices are not supported yet",
            archive.display()
        ));
    }
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp: PathBuf = target.with_file_name(format!(".{name}.installing"));
    let _ = std::fs::remove_dir_all(&tmp);
    let r = extract(archive, &layout, &tmp).and_then(|n| {
        let file = archive
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        std::fs::write(
            tmp.join("meta.ini"),
            format!("[General]\ninstallationFile={file}\n"),
        )
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, target).map_err(|e| format!("{}: {e}", target.display()))?;
        Ok(n)
    });
    if r.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    r
}

fn extract(archive: &Path, layout: &ArchiveLayout, into: &Path) -> Result<usize, String> {
    let mut z = open(archive)?;
    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    let mut n = 0;
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| format!("{}: {e}", archive.display()))?;
        if e.is_dir() {
            continue;
        }
        let raw = e
            .name()
            .map_err(|x| format!("{}: {x}", archive.display()))?
            .into_owned();
        let Some(name) = norm(&raw) else {
            return Err(format!("{}: unsafe path {raw:?}", archive.display()));
        };
        let Some(rel) = name.strip_prefix(layout.root.as_str()) else {
            continue;
        };
        if rel.is_empty() || rel.to_lowercase().starts_with("fomod/") {
            continue;
        }
        let out = into.join(rel);
        // `norm` refused `..` and roots; check what the joined path is made of anyway
        if out.strip_prefix(into).map_or(true, |r| {
            r.components().any(|c| !matches!(c, Component::Normal(_)))
        }) {
            return Err(format!("{}: unsafe path {rel:?}", archive.display()));
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let mut f = File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        std::io::copy(&mut e, &mut f).map_err(|e| format!("{}: {e}", out.display()))?;
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(File::create(path).unwrap());
        for (n, d) in entries {
            w.start_file(*n, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(d).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn the_data_root_is_found_and_the_mod_installs_whole() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("Mod-123.zip");
        zip(
            &a,
            &[
                ("Mod 1.0/Data/textures/a.dds", b"aa"),
                ("Mod 1.0/Data/Mod.esp", b"p"),
                ("Mod 1.0/readme.txt", b"r"),
            ],
        );
        let l = inspect(&a).unwrap();
        assert_eq!(l.root, "Mod 1.0/Data/");
        assert!(!l.fomod);
        let f = facts_of("Mod", &l);
        assert_eq!((f.metrics.files, f.metrics.plugins, f.metrics.dds), (2, 1, 1));
        let target = dir.path().join("mods").join("Mod");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        assert_eq!(install(&a, &target).unwrap(), 2);
        assert_eq!(std::fs::read(target.join("textures/a.dds")).unwrap(), b"aa");
        assert!(
            std::fs::read_to_string(target.join("meta.ini"))
                .unwrap()
                .contains("Mod-123.zip")
        );
        assert!(install(&a, &target).is_err(), "never over an existing folder");
    }

    #[test]
    fn fomod_and_unsafe_archives_are_refused_and_leave_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("f.zip");
        zip(
            &a,
            &[
                ("fomod/ModuleConfig.xml", b"<x/>"),
                ("00 Core/textures/a.dds", b"a"),
            ],
        );
        assert!(inspect(&a).unwrap().fomod);
        let target = dir.path().join("F");
        assert!(install(&a, &target).unwrap_err().contains("FOMOD"));
        let b = dir.path().join("b.zip");
        zip(&b, &[("../evil.txt", b"x")]);
        assert!(inspect(&b).is_err());
        assert!(install(&b, &dir.path().join("B")).is_err());
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left.len(), 2, "{left:?}");
    }
}
