//! Mod facts inside the project: what a scan saw, kept so the project works without the build. @sat:stll.facts
//!
//! Owns: the per-mod facts file (`facts/<entity id>.facts`): format, content signature, write-if-changed, read,
//! and dropping facts of entities that left the project.
//! Not here: producing facts (`@sat:scan`), the scan cache of one machine (`@sat:scan.cache`), entity ids
//! (`@sat:entity`).
//!
//! The scan cache belongs to a machine (keyed by folder name, holds analyzer stamps); these files belong to the
//! project (keyed by entity id, portable, no machine paths). A mod whose content signature did not change is not
//! rewritten, so saving after a scan of 7 500 mods writes only the mods that changed.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use sat_core::entity::EntityId;
use sat_core::facts::ModFacts;

// @sat:stll.facts.format
/// Layout of a facts file; bump with a reader for the previous one.
pub const FACTS_FORMAT: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Header {
    format: u32,
    signature: u64,
}

#[derive(Serialize)]
struct StoredRef<'a> {
    format: u32,
    signature: u64,
    facts: &'a ModFacts,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    format: u32,
    signature: u64,
    facts: ModFacts,
}

fn config() -> impl bincode::config::Config {
    bincode::config::standard()
}

/// The facts of a project (a folder of per-mod files).
#[derive(Debug, Clone)]
pub struct FactsStore {
    dir: PathBuf,
}

/// What a write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Put {
    Written,
    /// Same content signature already stored.
    Unchanged,
}

impl FactsStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: &EntityId) -> PathBuf {
        self.dir.join(format!("{}.facts", id.0))
    }

    /// The content signature stored for an entity (reads only the header).
    pub fn signature(&self, id: &EntityId) -> Option<u64> {
        let data = std::fs::read(self.path(id)).ok()?;
        let (h, _): (Header, usize) = bincode::serde::decode_from_slice(&data, config()).ok()?;
        (h.format == FACTS_FORMAT).then_some(h.signature)
    }

    // @sat:stll.facts.put
    /// Stores the facts of an entity unless the same content signature is already there. Written through a temp
    /// file and a rename: a crash never leaves half a file.
    pub fn put(&self, id: &EntityId, signature: u64, facts: &ModFacts) -> std::io::Result<Put> {
        if self.signature(id) == Some(signature) {
            return Ok(Put::Unchanged);
        }
        std::fs::create_dir_all(&self.dir)?;
        let bytes = bincode::serde::encode_to_vec(
            StoredRef {
                format: FACTS_FORMAT,
                signature,
                facts,
            },
            config(),
        )
        .map_err(std::io::Error::other)?;
        let path = self.path(id);
        let tmp = path.with_extension("facts-tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)?;
        Ok(Put::Written)
    }

    // @sat:stll.facts.get
    /// The stored facts of an entity and their content signature; `None` if absent, of another format or damaged
    /// (facts are rebuildable: a bad file is a cache miss, not an error).
    pub fn get(&self, id: &EntityId) -> Option<(u64, ModFacts)> {
        let data = std::fs::read(self.path(id)).ok()?;
        let (s, _): (Stored, usize) = bincode::serde::decode_from_slice(&data, config()).ok()?;
        (s.format == FACTS_FORMAT).then_some((s.signature, s.facts))
    }

    /// Removes facts of entities not in `keep`. Returns how many were removed.
    pub fn retain(&self, keep: &std::collections::HashSet<&str>) -> std::io::Result<usize> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return Ok(0);
        };
        let mut n = 0;
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(id) = name.strip_suffix(".facts")
                && !keep.contains(id)
            {
                std::fs::remove_file(e.path())?;
                n += 1;
            }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sat_core::facts::{LooseDir, LooseFile};

    #[test]
    fn facts_round_trip_and_unchanged_mods_are_not_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FactsStore::new(tmp.path().join("facts"));
        let id = EntityId::mint();
        let mut f = ModFacts {
            name: "A".into(),
            dirs: vec![LooseDir {
                rel: "textures".into(),
                files: vec![LooseFile {
                    name: "a.dds".into(),
                    size: 10,
                    vram: 4,
                }],
            }],
            ..ModFacts::default()
        };
        assert_eq!(store.put(&id, 7, &f).unwrap(), Put::Written);
        assert_eq!(store.put(&id, 7, &f).unwrap(), Put::Unchanged);
        f.metrics.files = 1;
        assert_eq!(store.put(&id, 8, &f).unwrap(), Put::Written);
        assert_eq!(store.get(&id), Some((8, f)));
        // damaged file = miss
        std::fs::write(store.dir().join(format!("{}.facts", id.0)), b"junk").unwrap();
        assert!(store.get(&id).is_none());
        // facts of entities that left the project go away
        let other = EntityId::mint();
        store.put(&other, 1, &ModFacts::default()).unwrap();
        let keep: std::collections::HashSet<&str> = [other.0.as_str()].into();
        assert_eq!(store.retain(&keep).unwrap(), 1);
        assert!(store.get(&other).is_some());
    }
}
