//! Installing a mod from an archive into its MO2 folder. @sat:mo2.install
//!
//! Owns: reading an archive's layout (where the game's `Data` content starts, whether it needs an installer),
//! the facts of a mod before it is installed (its files as the archive lists them), and extracting it into a
//! folder safely (no path leaves the folder).
//! Not here: when to install (`@sat:intent`, `@sat:sync.plan.installs`), the commit around it (`@sat:mo2.commit`).
//!
//! Formats: zip and 7z (told by their first bytes); rar — not yet. FOMOD installers: the choices are part of the intent
//! (`@sat:formats.fomod`); without choices — the installer's defaults.

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
/// What an install of an archive puts into the mod folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveLayout {
    /// The prefix inside the archive that becomes the mod folder (`""`, `Foo/`, `Foo/Data/`); for a FOMOD — the
    /// folder that holds `fomod/`.
    pub root: String,
    /// Files of the mod folder (`/`-separated, relative) and their sizes.
    pub files: Vec<(String, u64)>,
    /// Archive entry (normalized) → where it goes; one entry may go to several places.
    pub map: Vec<(String, String)>,
    /// The FOMOD installer and the choices in effect, if the archive has one.
    pub fomod: Option<FomodChoice>,
}

/// A FOMOD install as resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FomodChoice {
    pub module: String,
    /// `step/group/option` in effect (given or by the installer's defaults).
    pub chosen: Vec<String>,
    /// Sources the config names that the archive does not have (skipped, as MO2 does).
    pub missing: Vec<String>,
    pub config: sat_formats::fomod::Config,
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

/// Archive formats, told by their first bytes (the extension can lie).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Zip,
    SevenZ,
}

fn format_of(path: &Path) -> Result<Format, String> {
    let mut head = [0u8; 6];
    let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let n = std::io::Read::read(&mut f, &mut head).map_err(|e| format!("{}: {e}", path.display()))?;
    match &head[..n] {
        [b'P', b'K', ..] => Ok(Format::Zip),
        [b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C] => Ok(Format::SevenZ),
        [b'R', b'a', b'r', b'!', ..] => Err(format!("{}: rar is not supported yet", path.display())),
        _ => Err(format!("{}: not a zip or 7z archive", path.display())),
    }
}

fn open_zip(path: &Path) -> Result<zip::ZipArchive<BufReader<File>>, String> {
    let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    zip::ZipArchive::new(BufReader::new(f)).map_err(|e| format!("{}: {e}", path.display()))
}

fn open_7z(path: &Path) -> Result<sevenz_rust2::ArchiveReader<File>, String> {
    sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Files of an archive: raw name and size.
fn listing(path: &Path) -> Result<Vec<(String, u64)>, String> {
    let mut out = Vec::new();
    match format_of(path)? {
        Format::Zip => {
            let mut z = open_zip(path)?;
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
                out.push((raw, e.size()));
            }
        }
        Format::SevenZ => {
            let a = sevenz_rust2::Archive::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            out.extend(
                a.files
                    .iter()
                    .filter(|e| !e.is_directory())
                    .map(|e| (e.name().to_string(), e.size())),
            );
        }
    }
    Ok(out)
}

/// Reads an archive's layout without extracting it; a FOMOD archive takes the installer's defaults.
pub fn inspect(path: &Path) -> Result<ArchiveLayout, String> {
    inspect_with(path, None)
}

fn read_entry(path: &Path, raw: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    match format_of(path)? {
        Format::Zip => {
            let mut z = open_zip(path)?;
            let mut e = z
                .by_name(raw)
                .map_err(|e| format!("{}: {raw}: {e}", path.display()))?;
            std::io::Read::read_to_end(&mut e, &mut out).map_err(|e| e.to_string())?;
        }
        Format::SevenZ => {
            let mut found = false;
            open_7z(path)?
                .for_each_entries(|e, r| {
                    if e.name() == raw {
                        std::io::Read::read_to_end(r, &mut out)?;
                        found = true;
                        return Ok(false);
                    }
                    std::io::copy(r, &mut std::io::sink())?;
                    Ok(true)
                })
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if !found {
                return Err(format!("{}: {raw} not found", path.display()));
            }
        }
    }
    Ok(out)
}

// @sat:mo2.install.fomod
/// [`inspect`] with FOMOD choices (`step/group/option`); `None` takes the installer's defaults.
pub fn inspect_with(path: &Path, choices: Option<&[String]>) -> Result<ArchiveLayout, String> {
    let mut entries = Vec::new();
    let mut raws = Vec::new();
    for (raw, size) in listing(path)? {
        let name = norm(&raw).ok_or_else(|| format!("{}: unsafe path {raw:?}", path.display()))?;
        entries.push((name, size));
        raws.push(raw);
    }
    let config_at = entries.iter().position(|(p, _)| {
        let l = p.to_lowercase();
        (l == "fomod/moduleconfig.xml" || l.ends_with("/fomod/moduleconfig.xml"))
            && l.matches('/').count() <= 2
    });
    let size_of: std::collections::HashMap<String, u64> =
        entries.iter().map(|(n, s)| (n.to_lowercase(), *s)).collect();
    let mut map: Vec<(String, String)> = Vec::new();
    let (root, fomod) = match config_at {
        None => {
            let paths: Vec<String> = entries.iter().map(|(n, _)| n.clone()).collect();
            let root = find_root(&paths);
            for (n, _) in &entries {
                if let Some(rel) = n.strip_prefix(root.as_str())
                    && !rel.is_empty()
                {
                    map.push((n.clone(), rel.to_string()));
                }
            }
            (root, None)
        }
        Some(i) => {
            let cfg_path = &entries[i].0;
            let root = cfg_path[..cfg_path.len() - "fomod/moduleconfig.xml".len()].to_string();
            let config = sat_formats::fomod::parse(&read_entry(path, &raws[i])?)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let resolved = config.resolve(choices.unwrap_or(&[]))?;
            // destination → source, in install order: a later file wins its place
            let mut by_dest: Vec<(String, String)> = Vec::new();
            let mut at: std::collections::HashMap<String, usize> = Default::default();
            let mut missing = Vec::new();
            let mut put = |src: &str, dest: String| match at.get(&dest.to_lowercase()) {
                Some(&k) => by_dest[k] = (src.to_string(), dest),
                None => {
                    at.insert(dest.to_lowercase(), by_dest.len());
                    by_dest.push((src.to_string(), dest));
                }
            };
            let join = |a: &str, b: &str| match (a.is_empty(), b.is_empty()) {
                (true, _) => b.to_string(),
                (_, true) => a.to_string(),
                _ => format!("{a}/{b}"),
            };
            for f in &resolved.files {
                let src = join(root.trim_end_matches('/'), &f.source).to_lowercase();
                if f.folder {
                    let prefix = if src.is_empty() {
                        String::new()
                    } else {
                        format!("{src}/")
                    };
                    let mut any = false;
                    for (n, _) in &entries {
                        let under = n.len() > prefix.len()
                            && n.is_char_boundary(prefix.len())
                            && n[..prefix.len()].to_lowercase() == prefix;
                        if let Some(rest) = under.then(|| &n[prefix.len()..]) {
                            if rest.to_lowercase().starts_with("fomod/") && f.source.is_empty() {
                                continue;
                            }
                            put(n, join(&f.destination, rest));
                            any = true;
                        }
                    }
                    if !any {
                        missing.push(f.source.clone());
                    }
                } else if let Some((n, _)) = entries.iter().find(|(n, _)| n.to_lowercase() == src) {
                    let dest = if f.destination.is_empty() {
                        f.source.clone()
                    } else {
                        f.destination.clone()
                    };
                    put(n, dest);
                } else {
                    missing.push(f.source.clone());
                }
            }
            map = by_dest;
            (
                root,
                Some(FomodChoice {
                    module: config.name.clone(),
                    chosen: resolved.chosen,
                    missing,
                    config,
                }),
            )
        }
    };
    map.retain(|(_, d)| !d.is_empty() && !d.to_lowercase().starts_with("fomod/"));
    let files = map
        .iter()
        .map(|(src, dest)| {
            (
                dest.clone(),
                size_of.get(&src.to_lowercase()).copied().unwrap_or(0),
            )
        })
        .collect();
    Ok(ArchiveLayout {
        root,
        files,
        map,
        fomod,
    })
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
pub fn install(archive: &Path, target: &Path, choices: Option<&[String]>) -> Result<usize, String> {
    if target.exists() {
        return Err(format!("{} already exists", target.display()));
    }
    let layout = inspect_with(archive, choices)?;
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

/// Where archive entries go: normalized lowercase entry name → destinations.
type Targets = std::collections::HashMap<String, Vec<String>>;

fn targets(layout: &ArchiveLayout) -> Targets {
    let mut t: Targets = Default::default();
    for (src, dest) in &layout.map {
        t.entry(src.to_lowercase()).or_default().push(dest.clone());
    }
    t
}

/// The files an archive entry becomes in `into` (none: not installed).
fn destinations(archive: &Path, t: &Targets, into: &Path, raw: &str) -> Result<Vec<PathBuf>, String> {
    let Some(name) = norm(raw) else {
        return Err(format!("{}: unsafe path {raw:?}", archive.display()));
    };
    let mut out = Vec::new();
    for rel in t.get(&name.to_lowercase()).into_iter().flatten() {
        let p = into.join(rel);
        // `norm` refused `..` and roots in entries; destinations come from a config — check what they are made of
        if p.strip_prefix(into).map_or(true, |r| {
            r.components().any(|c| !matches!(c, Component::Normal(_)))
        }) {
            return Err(format!("{}: unsafe destination {rel:?}", archive.display()));
        }
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        out.push(p);
    }
    Ok(out)
}

fn write_all(r: &mut dyn std::io::Read, outs: &[PathBuf]) -> Result<(), String> {
    match outs {
        [] => std::io::copy(r, &mut std::io::sink())
            .map(|_| ())
            .map_err(|e| e.to_string()),
        [first, rest @ ..] => {
            let mut f = File::create(first).map_err(|e| format!("{}: {e}", first.display()))?;
            std::io::copy(r, &mut f).map_err(|e| format!("{}: {e}", first.display()))?;
            for o in rest {
                std::fs::copy(first, o).map_err(|e| format!("{}: {e}", o.display()))?;
            }
            Ok(())
        }
    }
}

fn extract(archive: &Path, layout: &ArchiveLayout, into: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    let t = targets(layout);
    let mut n = 0;
    match format_of(archive)? {
        Format::Zip => {
            let mut z = open_zip(archive)?;
            for i in 0..z.len() {
                let mut e = z.by_index(i).map_err(|e| format!("{}: {e}", archive.display()))?;
                if e.is_dir() {
                    continue;
                }
                let raw = e
                    .name()
                    .map_err(|x| format!("{}: {x}", archive.display()))?
                    .into_owned();
                let outs = destinations(archive, &t, into, &raw)?;
                if !outs.is_empty() {
                    write_all(&mut e, &outs)?;
                    n += outs.len();
                }
            }
        }
        Format::SevenZ => {
            // 7z streams are solid: entries are read in archive order, one pass
            let mut failure = None;
            open_7z(archive)?
                .for_each_entries(|e, reader| {
                    if e.is_directory() {
                        return Ok(true);
                    }
                    let res = destinations(archive, &t, into, e.name()).and_then(|outs| {
                        n += outs.len();
                        write_all(reader, &outs)
                    });
                    match res {
                        Ok(()) => Ok(true),
                        Err(m) => {
                            failure = Some(m);
                            Ok(false)
                        }
                    }
                })
                .map_err(|e| format!("{}: {e}", archive.display()))?;
            if let Some(m) = failure {
                return Err(m);
            }
        }
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
        assert!(l.fomod.is_none());
        let f = facts_of("Mod", &l);
        assert_eq!((f.metrics.files, f.metrics.plugins, f.metrics.dds), (2, 1, 1));
        let target = dir.path().join("mods").join("Mod");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        assert_eq!(install(&a, &target, None).unwrap(), 2);
        assert_eq!(std::fs::read(target.join("textures/a.dds")).unwrap(), b"aa");
        assert!(
            std::fs::read_to_string(target.join("meta.ini"))
                .unwrap()
                .contains("Mod-123.zip")
        );
        assert!(
            install(&a, &target, None).is_err(),
            "never over an existing folder"
        );
    }

    #[test]
    fn a_7z_archive_installs_like_a_zip() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("Seven-1.7z");
        let mut w = sevenz_rust2::ArchiveWriter::create(&a).unwrap();
        for (n, d) in [
            ("Seven/textures/a.dds", &b"aa"[..]),
            ("Seven/Seven.esp", &b"p"[..]),
        ] {
            w.push_archive_entry(sevenz_rust2::ArchiveEntry::new_file(n), Some(d))
                .unwrap();
        }
        w.finish().unwrap();
        let l = inspect(&a).unwrap();
        assert_eq!(l.root, "Seven/");
        let target = dir.path().join("Seven");
        assert_eq!(install(&a, &target, None).unwrap(), 2);
        assert_eq!(std::fs::read(target.join("textures/a.dds")).unwrap(), b"aa");
        let r = dir.path().join("x.rar");
        std::fs::write(&r, b"Rar!\x1a\x07\x00").unwrap();
        assert!(inspect(&r).unwrap_err().contains("rar"));
    }

    #[test]
    fn a_fomod_installs_the_chosen_options_and_unsafe_archives_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("f.zip");
        let xml = br#"<config><moduleName>F</moduleName>
            <requiredInstallFiles><folder source="00 Core" destination=""/></requiredInstallFiles>
            <installSteps order="Explicit"><installStep name="S"><optionalFileGroups><group name="G" type="SelectExactlyOne">
              <plugins order="Explicit">
                <plugin name="One"><files><file source="opt\one.esp" destination="F.esp"/></files>
                  <typeDescriptor><type name="Recommended"/></typeDescriptor></plugin>
                <plugin name="Two"><files><file source="opt\two.esp" destination="F.esp"/></files>
                  <typeDescriptor><type name="Optional"/></typeDescriptor></plugin>
              </plugins></group></optionalFileGroups></installStep></installSteps></config>"#;
        zip(
            &a,
            &[
                ("F 1.0/fomod/ModuleConfig.xml", xml),
                ("F 1.0/00 Core/textures/a.dds", b"a"),
                ("F 1.0/opt/one.esp", b"1"),
                ("F 1.0/opt/two.esp", b"2"),
            ],
        );
        let l = inspect(&a).unwrap();
        assert_eq!(l.fomod.as_ref().unwrap().chosen, ["S/G/One"]);
        assert_eq!(l.root, "F 1.0/");
        let target = dir.path().join("F");
        let two = vec!["S/G/Two".to_string()];
        assert_eq!(install(&a, &target, Some(&two)).unwrap(), 2);
        assert_eq!(std::fs::read(target.join("F.esp")).unwrap(), b"2");
        assert!(target.join("textures/a.dds").exists());
        assert!(!target.join("fomod").exists() && !target.join("opt").exists());
        let bad = vec!["S/G/Three".to_string()];
        assert!(install(&a, &dir.path().join("F2"), Some(&bad)).is_err());
        let b = dir.path().join("b.zip");
        zip(&b, &[("../evil.txt", b"x")]);
        assert!(inspect(&b).is_err());
        assert!(install(&b, &dir.path().join("B"), None).is_err());
        let mut left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["F", "b.zip", "f.zip"]);
    }
}
