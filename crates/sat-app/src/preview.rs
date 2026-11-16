//! The file explorer's reads: copies and preview of a file, the build by folders, search, pins. @sat:app.preview
//!
//! Owns: the copies of a path (every mod that has it, loose or in which archive, size, which one the game takes,
//! disabled owners too), choosing the copy to show (the game's by default), asking the shell's preview source for
//! it (`@sat:scan.fetch` behind `set_preview_source`); the build's folders listed lazily (a folder → its folders
//! and files with counts, from the table's path interner, an index built once per table); search over paths;
//! pinned files (the project keeps them).
//! Not here: formats (`@sat:formats.preview`), trees of mods / chains (`@sat:app.tree`), editing (waits for
//! commit processing).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use sat_core::ids::{ModId, PathId};
use sat_core::order::Key;
use sat_core::table::{ProviderTable, Source};
use sat_formats::preview::Preview;

use crate::App;

/// Reads a copy: (mod folder name, game path, archive file name if packed) → preview.
pub type PreviewSource = Arc<dyn Fn(&str, &str, Option<&str>) -> Option<Preview> + Send + Sync>;

/// The shell's preview source, if any (no `Debug` on closures).
#[derive(Default, Clone)]
pub(crate) struct SourceSlot(pub Option<PreviewSource>);

impl std::fmt::Debug for SourceSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "SourceSlot(set)"
        } else {
            "SourceSlot(none)"
        })
    }
}

/// One copy of a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileCopy {
    pub name: String,
    pub key: Option<Key>,
    pub enabled: bool,
    /// The archive it is packed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<String>,
    pub size: u64,
    /// The copy the game takes.
    pub wins: bool,
}

/// A file with its copies and the preview of one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilePreview {
    pub path: String,
    /// The game's copy first, then by priority; disabled owners last.
    pub copies: Vec<FileCopy>,
    /// Index of the copy shown.
    pub shown: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<Preview>,
    pub pinned: bool,
}

/// One child of a build folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VfsEntry {
    pub name: String,
    /// Full game path.
    pub path: String,
    pub dir: bool,
    /// Files under it (a file: 1).
    pub files: u32,
    /// Of them, provided by two or more mods.
    pub contested: u32,
    /// A file: the mod the game takes it from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winner: Option<String>,
    /// A file: copies.
    #[serde(default)]
    pub copies: u32,
    /// A file: packed in an archive (the game's copy).
    #[serde(default)]
    pub archived: bool,
}

/// A pinned file and whether the build still has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedFile {
    pub path: String,
    pub found: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winner: Option<String>,
}

/// The build's folders: per folder its sub-folders and files, files under it (with contested), built once per
/// table.
#[derive(Debug, Default)]
pub(crate) struct VfsIndex {
    table: usize,
    /// dir id → child dirs
    dirs: Vec<Vec<u32>>,
    /// dir id → files
    files: Vec<Vec<PathId>>,
    /// dir id → (files under, contested under)
    under: Vec<(u32, u32)>,
}

fn distinct_owners(t: &ProviderTable, p: PathId) -> usize {
    let mut v: Vec<ModId> = t.path_providers(p).map(|i| t.owner(i)).collect();
    v.sort_unstable();
    v.dedup();
    v.len()
}

impl VfsIndex {
    fn build(t: &ProviderTable) -> Self {
        let it = t.interner();
        let n = it.dir_count();
        let mut ix = VfsIndex {
            table: t as *const _ as usize,
            dirs: vec![Vec::new(); n],
            files: vec![Vec::new(); n],
            under: vec![(0, 0); n],
        };
        for d in 1..n as u32 {
            let parent = it.parent(sat_core::ids::DirId(d));
            if let Some(v) = ix.dirs.get_mut(parent.0 as usize) {
                v.push(d);
            }
        }
        for p in 0..t.path_count() {
            let p = PathId::from_idx(p);
            let d = sat_core::interner::key_dir(t.path_key(p));
            ix.files[d.0 as usize].push(p);
            let contested = u32::from(distinct_owners(t, p) > 1);
            // sum up the folders above
            let mut at = d;
            loop {
                let u = &mut ix.under[at.0 as usize];
                u.0 += 1;
                u.1 += contested;
                if at.0 == 0 {
                    break;
                }
                at = it.parent(at);
            }
        }
        ix
    }
}

impl App {
    /// Where the shell reads copies from (`@sat:scan.fetch`).
    pub fn set_preview_source(&mut self, s: PreviewSource) {
        self.preview_source = SourceSlot(Some(s));
    }

    /// The project's pinned files.
    pub fn set_pinned_files(&mut self, files: Vec<String>) {
        self.pinned_files = files;
    }

    /// Pinned files as edited (the shell keeps them in the project).
    pub fn pinned_files(&self) -> &[String] {
        &self.pinned_files
    }

    // @sat:app.preview.copies
    /// The copies of `path`: the game's first, then the rest of the stack, then disabled owners.
    fn copies(&mut self, p: PathId) -> Vec<FileCopy> {
        let ow = self.session.overwrite();
        let table = self.session.table().clone();
        let book = self.session.book();
        let prio = ow.priorities();
        let mut provs: Vec<usize> = table.path_providers(p).collect();
        // higher priority first; loose before archive within a mod (loose wins)
        provs.sort_by_key(|&i| {
            let m = table.owner(i);
            (std::cmp::Reverse(prio.get(m)), table.is_archive(i), i)
        });
        let winner = provs.iter().copied().find(|&i| prio.is_active(table.owner(i)));
        provs
            .into_iter()
            .map(|i| {
                let m = table.owner(i);
                let name = table.mod_name(m).to_string();
                let key = book.key_of(&name);
                FileCopy {
                    enabled: prio.is_active(m),
                    key,
                    archive: match table.source(i) {
                        Source::Loose => None,
                        Source::Archive(a) => table.archives().get(a.idx()).map(|a| a.file_name.clone()),
                    },
                    size: table.size(i),
                    wins: Some(i) == winner,
                    name,
                }
            })
            .collect()
    }

    // @sat:app.preview.file
    /// A file's copies and the preview of copy `copy` (the game's when `None`).
    pub(crate) fn file_preview(&mut self, path: &str, copy: Option<u32>) -> Option<FilePreview> {
        let table = self.session.table().clone();
        let p = table.find_path(path)?;
        let copies = self.copies(p);
        let shown = copy
            .map(|c| c as usize)
            .filter(|&c| c < copies.len())
            .unwrap_or(0);
        let game = table.path_string(p);
        let preview = copies.get(shown).and_then(|c| {
            let src = self.preview_source.0.as_ref()?;
            src(&c.name, &game, c.archive.as_deref())
        });
        Some(FilePreview {
            pinned: self.pinned_files.iter().any(|f| f.eq_ignore_ascii_case(&game)),
            path: game,
            copies,
            shown: shown as u32,
            preview,
        })
    }

    // @sat:app.preview.vfs
    /// The children of build folder `dir` (`""` — the root): folders first, then files.
    pub(crate) fn vfs_dir(&mut self, dir: &str) -> Option<Vec<VfsEntry>> {
        let table = self.session.table().clone();
        let fresh = self
            .vfs
            .as_ref()
            .is_some_and(|v| v.table == Arc::as_ptr(&table) as usize);
        if !fresh {
            self.vfs = Some(Arc::new(VfsIndex::build(&table)));
        }
        let ix = self.vfs.clone()?;
        let it = table.interner();
        let d = if dir.trim_matches('/').is_empty() {
            sat_core::ids::DirId(0)
        } else {
            it.find_dir(&dir.trim_matches('/').to_lowercase())?
        };
        let ow = self.session.overwrite();
        let mut out: Vec<VfsEntry> = ix.dirs[d.0 as usize]
            .iter()
            .map(|&c| {
                let cd = sat_core::ids::DirId(c);
                let (files, contested) = ix.under[c as usize];
                VfsEntry {
                    name: it.segment(it.dir_segment(cd)).to_string(),
                    path: it.dir_path(cd),
                    dir: true,
                    files,
                    contested,
                    winner: None,
                    copies: 0,
                    archived: false,
                }
            })
            .collect();
        out.sort_by_key(|e| e.name.to_lowercase());
        let mut files: Vec<VfsEntry> = ix.files[d.0 as usize]
            .iter()
            .map(|&p| {
                let copies = table.path_providers(p).len() as u32;
                let owners = distinct_owners(&table, p);
                let (winner, archived) = match ow.path_chain(p) {
                    Some(c) => (
                        Some(self.session.unit_label(ow.chain_mods(c)[0]).to_string()),
                        false,
                    ),
                    None => {
                        let i = table
                            .path_providers(p)
                            .find(|&i| ow.priorities().is_active(table.owner(i)));
                        (
                            i.map(|i| {
                                self.session
                                    .unit_label(ow.units().rep(table.owner(i)))
                                    .to_string()
                            }),
                            i.is_some_and(|i| table.is_archive(i)),
                        )
                    }
                };
                let path = table.path_string(p);
                VfsEntry {
                    name: path.rsplit('/').next().unwrap_or(&path).to_string(),
                    path,
                    dir: false,
                    files: 1,
                    contested: u32::from(owners > 1),
                    winner,
                    copies,
                    archived,
                }
            })
            .collect();
        files.sort_by_key(|e| e.name.to_lowercase());
        out.extend(files);
        Some(out)
    }

    // @sat:app.preview.find
    /// Paths containing `text` (case-insensitive), at most `limit`; the total found.
    pub(crate) fn find_paths(&self, text: &str, limit: usize) -> (Vec<String>, usize) {
        let needle = text.trim().to_lowercase().replace('\\', "/");
        if needle.len() < 2 {
            return (Vec::new(), 0);
        }
        let table = self.session.table();
        let mut out = Vec::new();
        let mut total = 0usize;
        for p in 0..table.path_count() {
            let s = table.path_string(PathId::from_idx(p));
            if s.to_lowercase().contains(&needle) {
                total += 1;
                if out.len() < limit {
                    out.push(s);
                }
            }
        }
        out.sort_unstable();
        (out, total)
    }

    /// Pinned files with their state.
    pub(crate) fn pinned(&mut self) -> Vec<PinnedFile> {
        let table = self.session.table().clone();
        let ow = self.session.overwrite();
        self.pinned_files
            .iter()
            .map(|f| {
                let p = table.find_path(f);
                PinnedFile {
                    path: p.map_or_else(|| f.clone(), |p| table.path_string(p)),
                    found: p.is_some(),
                    winner: p.and_then(|p| {
                        let i = table
                            .path_providers(p)
                            .find(|&i| ow.priorities().is_active(table.owner(i)))?;
                        let m = match ow.path_chain(p) {
                            Some(c) => ow.chain_mods(c)[0],
                            None => ow.units().rep(table.owner(i)),
                        };
                        Some(self.session.unit_label(m).to_string())
                    }),
                }
            })
            .collect()
    }

    /// Pins or unpins a file; true when the list changed.
    pub(crate) fn pin_file(&mut self, path: &str, pinned: bool) -> bool {
        let table = self.session.table();
        let path = table
            .find_path(path)
            .map_or_else(|| path.replace('\\', "/"), |p| table.path_string(p));
        let at = self
            .pinned_files
            .iter()
            .position(|f| f.eq_ignore_ascii_case(&path));
        match (at, pinned) {
            (None, true) => {
                self.pinned_files.push(path);
                true
            }
            (Some(i), false) => {
                self.pinned_files.remove(i);
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{Command, Query, Reply};

    #[test]
    fn copies_show_who_has_a_file_and_the_source_previews_the_chosen_one() {
        let mut a = crate::tests::app_of(["A", "B"]);
        a.set_preview_source(Arc::new(|m: &str, p: &str, arch: Option<&str>| {
            Some(sat_formats::preview::Preview {
                format: format!("{m}:{p}:{arch:?}"),
                ..Default::default()
            })
        }));
        let Reply::FilePreview { file: Some(f) } = a
            .query(Query::FilePreview {
                path: "Textures/A.dds".into(),
                copy: None,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            f.copies
                .iter()
                .map(|c| (c.name.as_str(), c.wins))
                .collect::<Vec<_>>(),
            [("B", true), ("A", false)]
        );
        assert_eq!(f.preview.unwrap().format, "B:textures/a.dds:None");
        let Reply::FilePreview { file: Some(f) } = a
            .query(Query::FilePreview {
                path: "textures/a.dds".into(),
                copy: Some(1),
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(f.preview.unwrap().format, "A:textures/a.dds:None");
        // the build by folders, search, pins
        let Reply::VfsDir { entries } = a.query(Query::VfsDir { dir: String::new() }).unwrap() else {
            panic!()
        };
        assert_eq!(
            (
                entries[0].name.as_str(),
                entries[0].dir,
                entries[0].files,
                entries[0].contested
            ),
            ("textures", true, 1, 1)
        );
        let Reply::VfsDir { entries } = a
            .query(Query::VfsDir {
                dir: "textures".into(),
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            (
                entries[0].name.as_str(),
                entries[0].winner.as_deref(),
                entries[0].copies
            ),
            ("a.dds", Some("B"), 2)
        );
        let Reply::FoundPaths { paths, total } = a
            .query(Query::FindPaths {
                text: "A.D".into(),
                limit: 10,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!((paths, total), (vec!["textures/a.dds".to_string()], 1));
        a.command(Command::PinFile {
            path: "TEXTURES/A.DDS".into(),
            pinned: true,
        })
        .unwrap();
        let Reply::PinnedFiles { files } = a.query(Query::PinnedFiles).unwrap() else {
            panic!()
        };
        assert_eq!(
            (files[0].path.as_str(), files[0].found, files[0].winner.as_deref()),
            ("textures/a.dds", true, Some("B"))
        );
        assert_eq!(a.pinned_files(), ["textures/a.dds"]);
    }
}
