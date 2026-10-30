//! Warm tier: the provider table.
//!
//! A *provider* is one `(path, mod, source)` triple: a loose file of a mod, or an entry of one of its archives.
//! The table is column-oriented, sorted by [`FileKey`], immutable after build, and addressed only by integers:
//! - path groups: providers `[path_offsets[p], path_offsets[p+1])` share one virtual path ([`PathId`] `p`);
//! - mod index (CSR): providers of each mod, in path order.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::facts::ModFacts;
use crate::ids::{ArchiveId, ModId, PathId};
use crate::interner::{FileKey, PathInterner, key_dir};

/// An archive that contributes virtual paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveRef {
    pub owner: ModId,
    pub file_name: String,
    pub entries: u32,
}

/// Source of one provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Loose,
    Archive(ArchiveId),
}

const LOOSE: u32 = u32::MAX;

/// Immutable, column-oriented provider table.
#[derive(Debug)]
pub struct ProviderTable {
    interner: PathInterner,
    mod_names: Vec<String>,
    archives: Vec<ArchiveRef>,

    // provider columns, sorted by (key, insertion order)
    key: Vec<FileKey>,
    owner: Vec<ModId>,
    size: Vec<u64>,
    archive: Vec<u32>,
    vram_kib: Vec<u32>,

    // path groups
    path_key: Vec<FileKey>,
    path_offsets: Vec<u32>,

    // mod -> providers (CSR)
    mod_offsets: Vec<u32>,
    mod_providers: Vec<u32>,
}

/// Build statistics (for logs and budgets).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableStats {
    pub mods: usize,
    pub providers: usize,
    pub paths: usize,
    pub dirs: usize,
    pub names: usize,
    pub archives: usize,
}

impl ProviderTable {
    /// Builds the table. `ModId(i)` is `facts[i]`.
    ///
    /// Interning is sequential (one hash map); sorting and column gathering are parallel.
    pub fn build(facts: &[ModFacts]) -> Self {
        let total: usize = facts.iter().map(ModFacts::provider_count).sum();
        let mut interner = PathInterner::new();
        let mut archives = Vec::new();
        let mut key = Vec::with_capacity(total);
        let mut owner = Vec::with_capacity(total);
        let mut size = Vec::with_capacity(total);
        let mut archive = Vec::with_capacity(total);
        let mut vram = Vec::with_capacity(total);

        for (mi, m) in facts.iter().enumerate() {
            let mid = ModId::from_idx(mi);
            for d in &m.dirs {
                let dir = interner.dir(&d.rel);
                for f in &d.files {
                    key.push(crate::interner::file_key(dir, interner.name(&f.name)));
                    owner.push(mid);
                    size.push(f.size);
                    archive.push(LOOSE);
                    vram.push(to_kib(f.vram));
                }
            }
            for a in &m.archives {
                let Some(manifest) = &a.manifest else { continue };
                let aid = archives.len() as u32;
                archives.push(ArchiveRef {
                    owner: mid,
                    file_name: a.file_name.clone(),
                    entries: manifest.entries.len() as u32,
                });
                for e in &manifest.entries {
                    let (dir, file) = e.path.rsplit_once('/').unwrap_or(("", e.path.as_str()));
                    let dir = interner.dir(dir);
                    key.push(crate::interner::file_key(dir, interner.name(file)));
                    owner.push(mid);
                    size.push(e.size);
                    archive.push(aid);
                    vram.push(to_kib(e.vram));
                }
            }
        }

        let n = key.len();
        let mut order: Vec<(FileKey, u32)> = key.iter().enumerate().map(|(i, &k)| (k, i as u32)).collect();
        order.par_sort_unstable();
        fn gather<T: Copy + Send + Sync>(order: &[(FileKey, u32)], src: &[T]) -> Vec<T> {
            order.par_iter().map(|&(_, i)| src[i as usize]).collect()
        }
        let key_s: Vec<FileKey> = order.par_iter().map(|&(k, _)| k).collect();
        let owner_s = gather(&order, &owner);
        let size_s = gather(&order, &size);
        let archive_s = gather(&order, &archive);
        let vram_s = gather(&order, &vram);
        drop((key, owner, size, archive, vram, order));

        let mut path_key = Vec::new();
        let mut path_offsets = Vec::new();
        for i in 0..n {
            if i == 0 || key_s[i] != key_s[i - 1] {
                path_key.push(key_s[i]);
                path_offsets.push(i as u32);
            }
        }
        path_offsets.push(n as u32);

        let mods = facts.len();
        let mut mod_offsets = vec![0u32; mods + 1];
        for o in &owner_s {
            mod_offsets[o.idx() + 1] += 1;
        }
        for i in 1..=mods {
            mod_offsets[i] += mod_offsets[i - 1];
        }
        let mut fill = mod_offsets[..mods].to_vec();
        let mut mod_providers = vec![0u32; n];
        for (i, o) in owner_s.iter().enumerate() {
            mod_providers[fill[o.idx()] as usize] = i as u32;
            fill[o.idx()] += 1;
        }

        Self {
            interner,
            mod_names: facts.iter().map(|m| m.name.clone()).collect(),
            archives,
            key: key_s,
            owner: owner_s,
            size: size_s,
            archive: archive_s,
            vram_kib: vram_s,
            path_key,
            path_offsets,
            mod_offsets,
            mod_providers,
        }
    }

    pub fn stats(&self) -> TableStats {
        TableStats {
            mods: self.mod_names.len(),
            providers: self.key.len(),
            paths: self.path_key.len(),
            dirs: self.interner.dir_count(),
            names: self.interner.name_count(),
            archives: self.archives.len(),
        }
    }

    /// Approximate live heap bytes of the table (columns + indexes + interner), for memory budgets.
    pub fn heap_bytes(&self) -> usize {
        self.interner.heap_bytes()
            + self.key.capacity() * 8
            + self.owner.capacity() * 4
            + self.size.capacity() * 8
            + self.archive.capacity() * 4
            + self.vram_kib.capacity() * 4
            + self.path_key.capacity() * 8
            + self.path_offsets.capacity() * 4
            + self.mod_offsets.capacity() * 4
            + self.mod_providers.capacity() * 4
            + self.mod_names.iter().map(|n| n.capacity() + 24).sum::<usize>()
    }

    pub fn interner(&self) -> &PathInterner {
        &self.interner
    }
    pub fn mod_count(&self) -> usize {
        self.mod_names.len()
    }
    pub fn mod_name(&self, m: ModId) -> &str {
        &self.mod_names[m.idx()]
    }
    pub fn mod_names(&self) -> &[String] {
        &self.mod_names
    }
    pub fn archives(&self) -> &[ArchiveRef] {
        &self.archives
    }
    pub fn path_count(&self) -> usize {
        self.path_key.len()
    }
    pub fn provider_count(&self) -> usize {
        self.key.len()
    }

    pub fn path_key(&self, p: PathId) -> FileKey {
        self.path_key[p.idx()]
    }
    pub fn path_string(&self, p: PathId) -> String {
        self.interner.key_path(self.path_key[p.idx()])
    }
    /// Provider index range of a path group.
    pub fn path_providers(&self, p: PathId) -> std::ops::Range<usize> {
        self.path_offsets[p.idx()] as usize..self.path_offsets[p.idx() + 1] as usize
    }
    /// Path group containing provider `i` (binary search).
    pub fn path_of_provider(&self, i: usize) -> PathId {
        let p = self.path_offsets.partition_point(|&o| o as usize <= i) - 1;
        PathId::from_idx(p)
    }
    /// Finds a path by display/relative string (case-insensitive).
    pub fn find_path(&self, rel: &str) -> Option<PathId> {
        let (dir, file) = rel
            .trim_matches('/')
            .rsplit_once(['/', '\\'])
            .unwrap_or(("", rel));
        let k = crate::interner::file_key(self.interner.find_dir(dir)?, self.interner.find_name(file)?);
        self.path_key.binary_search(&k).ok().map(PathId::from_idx)
    }

    /// Provider indices of a mod, in path order.
    pub fn mod_providers(&self, m: ModId) -> &[u32] {
        &self.mod_providers[self.mod_offsets[m.idx()] as usize..self.mod_offsets[m.idx() + 1] as usize]
    }

    pub fn owner(&self, i: usize) -> ModId {
        self.owner[i]
    }
    pub fn size(&self, i: usize) -> u64 {
        self.size[i]
    }
    pub fn vram_kib(&self, i: usize) -> u32 {
        self.vram_kib[i]
    }
    pub fn provider_key(&self, i: usize) -> FileKey {
        self.key[i]
    }
    pub fn source(&self, i: usize) -> Source {
        match self.archive[i] {
            LOOSE => Source::Loose,
            a => Source::Archive(ArchiveId(a)),
        }
    }
    pub fn is_archive(&self, i: usize) -> bool {
        self.archive[i] != LOOSE
    }
    /// Directory of a provider's path.
    pub fn provider_dir(&self, i: usize) -> crate::ids::DirId {
        key_dir(self.key[i])
    }
}

fn to_kib(bytes: u64) -> u32 {
    if bytes == 0 {
        0
    } else {
        bytes.div_ceil(1024).min(u64::from(u32::MAX)) as u32
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::facts::{ArchiveFacts, LooseDir, LooseFile};
    use sat_formats::{ArchiveEntry, ArchiveKind, ArchiveManifest};

    /// `mk("A", &["textures/a.dds", …])`
    pub(crate) fn mk(name: &str, files: &[&str]) -> ModFacts {
        let mut m = ModFacts {
            name: name.into(),
            ..Default::default()
        };
        for f in files {
            let (dir, file) = f.rsplit_once('/').unwrap_or(("", f));
            m.dirs.push(LooseDir {
                rel: dir.into(),
                files: vec![LooseFile {
                    name: file.into(),
                    size: 10,
                    vram: 0,
                }],
            });
        }
        m
    }

    pub(crate) fn with_archive(mut m: ModFacts, archive: &str, paths: &[&str]) -> ModFacts {
        m.archives.push(ArchiveFacts {
            file_name: archive.into(),
            size: 0,
            manifest: Some(ArchiveManifest {
                kind: ArchiveKind::Bsa { version: 105 },
                entries: paths
                    .iter()
                    .map(|p| ArchiveEntry {
                        path: p.to_string(),
                        size: 7,
                        vram: 2048,
                    })
                    .collect(),
            }),
            error: None,
        });
        m
    }

    #[test]
    fn groups_paths_case_insensitively() {
        let t = ProviderTable::build(&[
            mk("A", &["textures/a.dds", "meshes/x.nif"]),
            mk("B", &["Textures/A.DDS", "textures/c.dds"]),
            with_archive(mk("C", &[]), "C.bsa", &["textures/a.dds"]),
        ]);
        let s = t.stats();
        assert_eq!((s.mods, s.providers, s.paths, s.archives), (3, 5, 3, 1));
        let p = t.find_path("TEXTURES/a.dds").unwrap();
        let owners: Vec<_> = t.path_providers(p).map(|i| t.mod_name(t.owner(i))).collect();
        assert_eq!(owners, ["A", "B", "C"]);
        assert_eq!(t.path_string(p), "textures/a.dds");
        assert_eq!(t.mod_providers(ModId(1)).len(), 2);
        let arch = t.path_providers(p).find(|&i| t.is_archive(i)).unwrap();
        assert_eq!(t.vram_kib(arch), 2);
        assert_eq!(t.path_of_provider(arch), p);
    }
}
