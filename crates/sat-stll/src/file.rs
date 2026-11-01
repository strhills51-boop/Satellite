//! The `.stll` file: the whole project in one zip — manifest, sections, facts — saved atomically. @sat:stll.file
//!
//! Owns: the file layout (ADR 0007 §Файл), opening it (sections joined into a [`Project`], facts read lazily by
//! entity id), and saving: a new file is written next to the old one and renamed over it, so a crash or a full
//! disk leaves the old project whole. Facts whose content signature did not change are copied as stored bytes
//! (no decompress, no recompress). Entries this build does not know (a journal of a newer build, extensions) are
//! copied unchanged.
//! Not here: how the tree is cut into sections (`@sat:stll.sections`), the facts format (`@sat:stll.facts`),
//! where the file of a build lives (callers: `<profile>/satellite/project.stll` — the location is the machine
//! binding for now; a binding file appears when a project is kept elsewhere).
//!
//! Layout:
//! - `manifest.json` — format, versions, sections (path, shape, lines, public), facts (signature per entity);
//!   written last;
//! - `project/…` — the user's truth as NDJSON sections;
//! - `facts/<entity id>.facts` — what a scan saw of one mod (deflate; rebuildable — may be dropped and rescanned).
//!
//! Integrity: every zip entry carries a CRC-32 that the reader checks; the manifest names what must be there.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use sat_core::facts::ModFacts;
use sat_core::project::{PROJECT_SCHEMA, Project};

use crate::facts::{FACTS_FORMAT, Put, decode, encode};
use crate::sections::{self, Shape};

/// The project file of a build, in its satellite folder.
pub const FILE_NAME: &str = "project.stll";
/// `manifest.json` → `format`.
pub const FORMAT: &str = "satellite-project";
// @sat:stll.file.version
/// Layout of the file itself (not of the project inside: that is `project_schema`).
pub const CONTAINER_VERSION: u32 = 1;

const MANIFEST: &str = "manifest.json";
const FACTS_DIR: &str = "facts/";

/// What the manifest says about one section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionInfo {
    pub name: String,
    pub path: Vec<String>,
    pub shape: Shape,
    pub lines: usize,
    /// Whether the section may be published with the project (the site shows public sections only).
    pub public: bool,
}

/// What the manifest says about facts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactsInfo {
    pub format: u32,
    /// Content signature of each stored mod (16-digit hex, by entity id): what is stored is known without
    /// reading it, so a save or a site loads only what it needs.
    pub mods: BTreeMap<String, String>,
}

/// `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub project_schema: u32,
    /// Program that wrote the file.
    pub writer: String,
    pub sections: Vec<SectionInfo>,
    #[serde(default)]
    pub facts: FactsInfo,
    /// Fields of a newer writer, kept.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Sections kept private by default: sync records may name folders of the user's machine.
fn public(name: &str) -> bool {
    !name.starts_with("project/sync")
}

#[derive(Debug, thiserror::Error)]
pub enum StllError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: not a readable project file: {message}")]
    Zip { path: PathBuf, message: String },
    #[error("{path}: {message}")]
    Project { path: PathBuf, message: String },
    #[error(
        "{path}: written by a newer Satellite (file layout {found}, this build reads {CONTAINER_VERSION})"
    )]
    Newer { path: PathBuf, found: u32 },
}

/// An opened project file (or one not written yet).
pub struct Stll {
    path: PathBuf,
    archive: Option<ZipArchive<BufReader<File>>>,
    manifest: Option<Manifest>,
}

impl std::fmt::Debug for Stll {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stll")
            .field("path", &self.path)
            .field("on_disk", &self.archive.is_some())
            .finish()
    }
}

fn zip_err(path: &Path, e: impl std::fmt::Display) -> StllError {
    StllError::Zip {
        path: path.to_path_buf(),
        message: e.to_string(),
    }
}

fn read_entry(archive: &mut ZipArchive<BufReader<File>>, name: &str) -> Option<Vec<u8>> {
    let mut f = archive.by_name(name).ok()?;
    let mut data = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut data).ok()?;
    Some(data)
}

fn facts_entry(id: &str) -> String {
    format!("{FACTS_DIR}{id}.facts")
}

impl Stll {
    /// A project file that does not exist yet; [`Stll::begin`] writes it.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            archive: None,
            manifest: None,
        }
    }

    // @sat:stll.file.open
    /// Opens a project file: the sections become a [`Project`] (migrated if older); facts stay in the file until
    /// asked for.
    pub fn open(path: impl Into<PathBuf>) -> Result<(Self, Project), StllError> {
        let path = path.into();
        let file = File::open(&path).map_err(|source| StllError::Io {
            path: path.clone(),
            source,
        })?;
        let mut archive = ZipArchive::new(BufReader::new(file)).map_err(|e| zip_err(&path, e))?;
        let manifest: Manifest = read_entry(&mut archive, MANIFEST)
            .ok_or_else(|| zip_err(&path, "no manifest.json"))
            .and_then(|d| {
                serde_json::from_slice(&d).map_err(|e| zip_err(&path, format!("manifest.json: {e}")))
            })?;
        if manifest.format != FORMAT {
            return Err(zip_err(&path, format!("format {:?}", manifest.format)));
        }
        if manifest.version > CONTAINER_VERSION {
            return Err(StllError::Newer {
                path,
                found: manifest.version,
            });
        }
        let mut bodies = Vec::with_capacity(manifest.sections.len());
        for s in &manifest.sections {
            let body = read_entry(&mut archive, &s.name)
                .ok_or_else(|| zip_err(&path, format!("{} is missing", s.name)))?;
            bodies.push(body);
        }
        let tree = sections::join(
            manifest
                .sections
                .iter()
                .zip(&bodies)
                .map(|(s, b)| (s.name.as_str(), s.path.as_slice(), s.shape, b.as_slice())),
        )
        .map_err(|e| StllError::Project {
            path: path.clone(),
            message: e.to_string(),
        })?;
        let project = Project::from_value(Value::Object(tree)).map_err(|message| StllError::Project {
            path: path.clone(),
            message,
        })?;
        Ok((
            Self {
                path,
                archive: Some(archive),
                manifest: Some(manifest),
            },
            project,
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn manifest(&self) -> Option<&Manifest> {
        self.manifest.as_ref()
    }

    /// The content signature of an entity's stored facts (from the manifest; nothing is read).
    pub fn facts_signature(&self, id: &str) -> Option<u64> {
        let m = self.manifest.as_ref()?;
        if m.facts.format != FACTS_FORMAT {
            return None;
        }
        u64::from_str_radix(m.facts.mods.get(id)?, 16).ok()
    }

    /// How many mods have stored facts.
    pub fn facts_count(&self) -> usize {
        self.manifest.as_ref().map_or(0, |m| m.facts.mods.len())
    }

    // @sat:stll.file.facts
    /// The stored facts of an entity and their content signature; `None` if absent or damaged (facts are
    /// rebuildable: a bad entry is a miss, not an error).
    pub fn facts(&mut self, id: &str) -> Option<(u64, ModFacts)> {
        self.facts_signature(id)?;
        decode(&read_entry(self.archive.as_mut()?, &facts_entry(id))?)
    }

    // @sat:stll.file.save
    /// Starts writing a new version of the file (next to it, as a temp file): put facts as a scan produces them,
    /// then [`Writer::finish`] with the project.
    pub fn begin(&mut self) -> Result<Writer<'_>, StllError> {
        let dir = self
            .path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        std::fs::create_dir_all(&dir).map_err(|source| StllError::Io {
            path: dir.clone(),
            source,
        })?;
        let tmp = tempfile::Builder::new()
            .prefix(".project.")
            .suffix(".stll-tmp")
            .tempfile_in(&dir)
            .map_err(|source| StllError::Io {
                path: dir.clone(),
                source,
            })?;
        let (file, tmp_path) = tmp.into_parts();
        Ok(Writer {
            src: self,
            zip: ZipWriter::new(file),
            tmp: tmp_path,
            facts: BTreeMap::new(),
            report: Saved::default(),
        })
    }

    /// Writes the project, keeping the stored facts of every entity still in it.
    pub fn save(&mut self, project: &Project) -> Result<Saved, StllError> {
        let w = self.begin()?;
        w.finish(project, keep_entities(project))
    }
}

/// The facts to keep on save: those of the project's entities (and the overwrite folder).
pub fn keep_entities(project: &Project) -> impl Fn(&str) -> bool + '_ {
    let ids: BTreeSet<&str> = project.entities.iter().map(|e| e.id.0.as_str()).collect();
    move |id: &str| id == OVERWRITE_ID || ids.contains(id)
}

/// Facts id of MO2's overwrite folder (no entity: it is not in the canon).
pub const OVERWRITE_ID: &str = "overwrite";

/// What a save did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Saved {
    /// Facts compressed and written (new or changed).
    pub facts_written: usize,
    /// Facts copied as stored (same signature).
    pub facts_copied: usize,
    /// Stored facts of entities that left the project.
    pub facts_dropped: usize,
    /// Entries of other programs kept.
    pub foreign_kept: usize,
    pub sections: usize,
}

/// A new version of a project file being written.
pub struct Writer<'a> {
    src: &'a mut Stll,
    zip: ZipWriter<File>,
    tmp: tempfile::TempPath,
    facts: BTreeMap<String, u64>,
    report: Saved,
}

impl std::fmt::Debug for Writer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Writer")
            .field("path", &self.src.path)
            .field("facts", &self.facts.len())
            .finish()
    }
}

fn opts(method: CompressionMethod) -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(method)
        .compression_level(if method == CompressionMethod::Deflated {
            Some(3)
        } else {
            None
        })
        .large_file(false)
}

impl Writer<'_> {
    fn io(&self, e: impl std::fmt::Display) -> std::io::Error {
        std::io::Error::other(format!("{}: {e}", self.tmp.display()))
    }

    fn copy(&mut self, name: &str) -> Result<bool, std::io::Error> {
        let Some(archive) = self.src.archive.as_mut() else {
            return Ok(false);
        };
        let Ok(entry) = archive.by_name(name) else {
            return Ok(false);
        };
        self.zip
            .raw_copy_file(entry)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(true)
    }

    // @sat:stll.file.put
    /// Puts one mod's facts: unchanged ones (same content signature as stored) are copied as they are.
    pub fn put_facts(&mut self, id: &str, signature: u64, facts: &ModFacts) -> std::io::Result<Put> {
        if self.facts.contains_key(id) {
            return Ok(Put::Unchanged);
        }
        let name = facts_entry(id);
        if self.src.facts_signature(id) == Some(signature) && self.copy(&name)? {
            self.facts.insert(id.to_string(), signature);
            self.report.facts_copied += 1;
            return Ok(Put::Unchanged);
        }
        let bytes = encode(signature, facts)?;
        self.zip
            .start_file(name, opts(CompressionMethod::Deflated))
            .map_err(|e| self.io(e))?;
        self.zip.write_all(&bytes)?;
        self.facts.insert(id.to_string(), signature);
        self.report.facts_written += 1;
        Ok(Put::Written)
    }

    /// Finishes the file and puts it in place of the old one (a rename: the old file is whole until then).
    /// Stored facts not put again are kept if `keep` says so; entries of other programs are kept.
    pub fn finish(self, project: &Project, keep: impl Fn(&str) -> bool) -> Result<Saved, StllError> {
        let target = self.src.path.clone();
        self.finish_to(project, keep, &target)
    }

    /// [`Writer::finish`] into another file (a commit prepares the new project beside the old one and swaps
    /// them inside its transaction). The opened file then still reads the old version.
    pub fn finish_to(
        mut self,
        project: &Project,
        keep: impl Fn(&str) -> bool,
        target: &Path,
    ) -> Result<Saved, StllError> {
        let path = target.to_path_buf();
        let io = |source: std::io::Error| StllError::Io {
            path: path.clone(),
            source,
        };
        // stored facts not met in this run
        let old: Vec<(String, String)> = self
            .src
            .manifest
            .as_ref()
            .map(|m| m.facts.mods.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        for (id, sig) in old {
            if self.facts.contains_key(&id) {
                continue;
            }
            if keep(&id) && self.copy(&facts_entry(&id)).map_err(io)? {
                self.facts
                    .insert(id, u64::from_str_radix(&sig, 16).unwrap_or_default());
                self.report.facts_copied += 1;
            } else {
                self.report.facts_dropped += 1;
            }
        }
        // entries of other programs
        let known: BTreeSet<String> = self
            .src
            .manifest
            .as_ref()
            .map(|m| m.sections.iter().map(|s| s.name.clone()).collect())
            .unwrap_or_default();
        let foreign: Vec<String> = self.src.archive.as_ref().map_or_else(Vec::new, |a| {
            a.file_names()
                .filter_map(Result::ok)
                .filter(|n| n != MANIFEST && !n.starts_with(FACTS_DIR) && !known.contains(n.as_ref()))
                .filter(|n| !n.starts_with("project/"))
                .map(|n| n.into_owned())
                .collect()
        });
        for name in foreign {
            if self.copy(&name).map_err(io)? {
                self.report.foreign_kept += 1;
            }
        }
        // the user's truth
        let tree = match project.to_value() {
            Ok(Value::Object(o)) => o,
            Ok(_) => unreachable!("a project serializes to an object"),
            Err(message) => {
                return Err(StllError::Project {
                    path: path.clone(),
                    message,
                });
            }
        };
        let secs = sections::split(&tree);
        let mut infos = Vec::with_capacity(secs.len());
        for s in secs {
            self.zip
                .start_file(&s.name, opts(CompressionMethod::Deflated))
                .map_err(|e| zip_err(&path, e))?;
            self.zip.write_all(&s.body).map_err(io)?;
            infos.push(SectionInfo {
                public: public(&s.name),
                name: s.name,
                path: s.path,
                shape: s.shape,
                lines: s.lines,
            });
        }
        self.report.sections = infos.len();
        let manifest = Manifest {
            format: FORMAT.into(),
            version: CONTAINER_VERSION,
            project_schema: PROJECT_SCHEMA,
            writer: format!("sat-stll {}", env!("CARGO_PKG_VERSION")),
            sections: infos,
            facts: FactsInfo {
                format: FACTS_FORMAT,
                mods: self
                    .facts
                    .iter()
                    .map(|(k, v)| (k.clone(), format!("{v:016x}")))
                    .collect(),
            },
            extra: self
                .src
                .manifest
                .as_ref()
                .map(|m| m.extra.clone())
                .unwrap_or_default(),
        };
        let mut body = serde_json::to_vec_pretty(&manifest).expect("manifest serializes");
        body.push(b'\n');
        self.zip
            .start_file(MANIFEST, opts(CompressionMethod::Stored))
            .map_err(|e| zip_err(&path, e))?;
        self.zip.write_all(&body).map_err(io)?;
        let file = self.zip.finish().map_err(|e| zip_err(&path, e))?;
        file.sync_all().map_err(io)?;
        drop(file);
        let replacing = target == self.src.path;
        if replacing {
            // the old file is closed before the rename (Windows keeps open files in place)
            self.src.archive = None;
        }
        // a replaced file keeps its permissions (the temp file is created private)
        if let Ok(md) = std::fs::metadata(target) {
            let _ = std::fs::set_permissions(&self.tmp, md.permissions());
        }
        self.tmp.persist(target).map_err(|e| io(e.error))?;
        if replacing {
            let reopened = File::open(target).map_err(io)?;
            self.src.archive =
                Some(ZipArchive::new(BufReader::new(reopened)).map_err(|e| zip_err(&path, e))?);
            self.src.manifest = Some(manifest);
        }
        Ok(self.report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sat_core::canon::{Entry, EntryKind, Modlist};
    use sat_core::facts::{LooseDir, LooseFile};

    fn project() -> Project {
        let mut p = Project::import(Modlist::new(
            ["A", "B"]
                .iter()
                .map(|n| Entry {
                    name: n.to_string(),
                    kind: EntryKind::Mod,
                    enabled: true,
                })
                .collect(),
        ));
        p.signatures.insert(p.entities.id_of("A").unwrap().0.clone(), 7);
        p
    }

    fn facts(name: &str, size: u64) -> ModFacts {
        ModFacts {
            name: name.into(),
            dirs: vec![LooseDir {
                rel: "textures".into(),
                files: vec![LooseFile {
                    name: "a.dds".into(),
                    size,
                    vram: 0,
                }],
            }],
            ..ModFacts::default()
        }
    }

    fn ids(p: &Project) -> (String, String) {
        (
            p.entities.id_of("A").unwrap().0.clone(),
            p.entities.id_of("B").unwrap().0.clone(),
        )
    }

    #[test]
    fn a_project_round_trips_with_its_facts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let p = project();
        let (a, b) = ids(&p);
        let mut f = Stll::new(&path);
        let mut w = f.begin().unwrap();
        assert_eq!(w.put_facts(&a, 1, &facts("A", 10)).unwrap(), Put::Written);
        assert_eq!(w.put_facts(&b, 2, &facts("B", 20)).unwrap(), Put::Written);
        let saved = w.finish(&p, keep_entities(&p)).unwrap();
        assert_eq!((saved.facts_written, saved.facts_copied), (2, 0));
        // only the temp file's rename made it: no leftovers
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [FILE_NAME]);

        let (mut g, back) = Stll::open(&path).unwrap();
        assert_eq!(back, p);
        assert_eq!(g.facts_count(), 2);
        assert_eq!(g.facts(&a), Some((1, facts("A", 10))));
        assert_eq!(g.facts("nobody"), None);
        // sections are plain NDJSON a person can read
        let mut zip = ZipArchive::new(File::open(&path).unwrap()).unwrap();
        let mut ent = String::new();
        zip.by_name("project/entities.ndjson")
            .unwrap()
            .read_to_string(&mut ent)
            .unwrap();
        assert_eq!(ent.lines().count(), 2);
        assert!(ent.contains("\"name\":\"A\""));
    }

    #[test]
    fn a_save_copies_unchanged_facts_drops_gone_ones_and_keeps_foreign_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut p = project();
        let (a, b) = ids(&p);
        let mut f = Stll::new(&path);
        let mut w = f.begin().unwrap();
        w.put_facts(&a, 1, &facts("A", 10)).unwrap();
        w.put_facts(&b, 2, &facts("B", 20)).unwrap();
        w.finish(&p, keep_entities(&p)).unwrap();
        // another program adds an entry and a manifest field (rewrite the zip with them)
        {
            let mut src = ZipArchive::new(File::open(&path).unwrap()).unwrap();
            let out_path = dir.path().join("x.zip");
            let mut out = ZipWriter::new(File::create(&out_path).unwrap());
            for i in 0..src.len() {
                let e = src.by_index(i).unwrap();
                if e.name().unwrap() == MANIFEST {
                    continue;
                }
                out.raw_copy_file(e).unwrap();
            }
            let mut m: Value = {
                let mut s = String::new();
                src.by_name(MANIFEST).unwrap().read_to_string(&mut s).unwrap();
                serde_json::from_str(&s).unwrap()
            };
            m["game"] = Value::String("skyrimse".into());
            out.start_file("extensions/note.txt", SimpleFileOptions::default())
                .unwrap();
            out.write_all(b"hello").unwrap();
            out.start_file(MANIFEST, SimpleFileOptions::default()).unwrap();
            out.write_all(&serde_json::to_vec(&m).unwrap()).unwrap();
            out.finish().unwrap();
            std::fs::rename(&out_path, &path).unwrap();
        }
        // and a newer field in the project tree survives too
        p.extra.insert("future".into(), serde_json::json!({"k": 1}));
        let (mut g, _) = Stll::open(&path).unwrap();
        // B leaves the project; A is rescanned with the same signature
        p.canon.entries.retain(|e| e.name != "B");
        p.entities.follow(&p.canon);
        let mut w = g.begin().unwrap();
        assert_eq!(w.put_facts(&a, 1, &facts("A", 10)).unwrap(), Put::Unchanged);
        let saved = w.finish(&p, keep_entities(&p)).unwrap();
        assert_eq!(
            (
                saved.facts_written,
                saved.facts_copied,
                saved.facts_dropped,
                saved.foreign_kept
            ),
            (0, 1, 1, 1)
        );
        assert_eq!(g.facts(&a), Some((1, facts("A", 10))), "the reopened file reads");
        let (mut h, back) = Stll::open(&path).unwrap();
        assert_eq!(back.extra.get("future"), Some(&serde_json::json!({"k": 1})));
        assert_eq!(
            h.manifest().unwrap().extra.get("game"),
            Some(&Value::String("skyrimse".into()))
        );
        assert!(h.facts(&b).is_none());
        let mut zip = ZipArchive::new(File::open(&path).unwrap()).unwrap();
        let mut s = String::new();
        zip.by_name("extensions/note.txt")
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        assert_eq!(s, "hello");
    }

    #[test]
    fn a_failed_save_leaves_the_old_file_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let p = project();
        let mut f = Stll::new(&path);
        f.save(&p).unwrap();
        let before = std::fs::read(&path).unwrap();
        {
            let (mut g, _) = Stll::open(&path).unwrap();
            let mut w = g.begin().unwrap();
            w.put_facts("x", 1, &facts("X", 1)).unwrap();
            // dropped without finish: a crash mid-save
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no temp file left"
        );
        // garbage is refused with the file name, not a panic
        std::fs::write(&path, b"junk").unwrap();
        let e = Stll::open(&path).unwrap_err().to_string();
        assert!(e.contains(FILE_NAME), "{e}");
    }
}
