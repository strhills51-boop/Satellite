//! Stable entity ids: a mod or a separator is an id; its name is an attribute (ADR 0007 §Модель). @sat:entity
//!
//! Owns: entity ids, minting them, and the registry id ↔ current name (case-folded lookup) of a project.
//! Not here: the order (`@sat:canon`), what the project keys by id (`@sat:project`).
//!
//! Why: an MO2 folder name was the identity, so a rename lost everything keyed by it (colour, style, tags, notes,
//! the content signature that finds renames); a planned mod has no folder yet; Vortex has no mod folders at all.
//! With ids, a rename is an attribute change and every section of the project follows the entity.
//!
//! Ids are `e` + 16 base-32 chars: 48 bits of time (ms) and 32 bits of a per-process counter mixed with the
//! process start, so ids minted on two machines do not collide in practice and sort by creation time.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::canon::{EntryKind, Modlist};

/// A stable entity id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub String);

impl std::fmt::Display for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

const B32: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

fn seed() -> u64 {
    static SEED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *SEED.get_or_init(|| {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        let mut h = t ^ u64::from(std::process::id()).rotate_left(32) ^ 0x9e37_79b9_7f4a_7c15;
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^ (h >> 33)
    })
}

// @sat:entity.mint
impl EntityId {
    /// A new id.
    pub fn mint() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let low = (seed().wrapping_add(n.wrapping_mul(0x9e37_79b9))) & 0xffff_ffff;
        let v: u128 = (u128::from(ms & 0xffff_ffff_ffff) << 32) | u128::from(low);
        let mut s = String::with_capacity(17);
        s.push('e');
        for i in (0..16).rev() {
            s.push(B32[((v >> (i * 5)) & 31) as usize] as char);
        }
        Self(s)
    }
}

/// One entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityRec {
    pub id: EntityId,
    pub kind: EntryKind,
    /// Current name (MO2: the folder name; separators keep `_separator`).
    pub name: String,
}

// @sat:entity.registry
/// The entities of a project: id ↔ current name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Vec<EntityRec>", into = "Vec<EntityRec>")]
pub struct Entities {
    list: Vec<EntityRec>,
    by_name: HashMap<String, usize>,
    by_id: HashMap<EntityId, usize>,
}

impl From<Vec<EntityRec>> for Entities {
    fn from(list: Vec<EntityRec>) -> Self {
        let mut e = Self {
            list,
            ..Self::default()
        };
        e.reindex();
        e
    }
}

impl From<Entities> for Vec<EntityRec> {
    fn from(e: Entities) -> Self {
        e.list
    }
}

fn fold(s: &str) -> String {
    s.to_lowercase()
}

impl Entities {
    fn reindex(&mut self) {
        self.by_name = self
            .list
            .iter()
            .enumerate()
            .map(|(i, r)| (fold(&r.name), i))
            .collect();
        self.by_id = self
            .list
            .iter()
            .enumerate()
            .map(|(i, r)| (r.id.clone(), i))
            .collect();
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = &EntityRec> {
        self.list.iter()
    }

    pub fn id_of(&self, name: &str) -> Option<&EntityId> {
        self.by_name.get(&fold(name)).map(|&i| &self.list[i].id)
    }
    pub fn name_of(&self, id: &EntityId) -> Option<&str> {
        self.by_id.get(id).map(|&i| self.list[i].name.as_str())
    }

    /// The id of `name`, minting one if the entity is new.
    pub fn ensure(&mut self, name: &str, kind: EntryKind) -> EntityId {
        if let Some(id) = self.id_of(name) {
            return id.clone();
        }
        let id = EntityId::mint();
        self.by_name.insert(fold(name), self.list.len());
        self.by_id.insert(id.clone(), self.list.len());
        self.list.push(EntityRec {
            id: id.clone(),
            kind,
            name: name.to_string(),
        });
        id
    }

    // @sat:entity.rename
    /// Renames an entity: the id and everything keyed by it stay. `false` if `from` is unknown or `to` is taken
    /// by another entity.
    pub fn rename(&mut self, from: &str, to: &str) -> bool {
        let Some(&i) = self.by_name.get(&fold(from)) else {
            return false;
        };
        if self.by_name.get(&fold(to)).is_some_and(|&j| j != i) {
            return false;
        }
        self.by_name.remove(&fold(from));
        self.list[i].name = to.to_string();
        self.by_name.insert(fold(to), i);
        true
    }

    pub fn remove(&mut self, name: &str) -> Option<EntityId> {
        let i = *self.by_name.get(&fold(name))?;
        let r = self.list.remove(i);
        self.reindex();
        Some(r.id)
    }

    /// Makes the registry follow a canon: every entry gets an id (kept if known by name), entities no longer in
    /// the canon are dropped, the order follows the canon.
    pub fn follow(&mut self, canon: &Modlist) {
        let list = canon
            .entries
            .iter()
            .map(|e| EntityRec {
                id: self.id_of(&e.name).cloned().unwrap_or_else(EntityId::mint),
                kind: e.kind,
                name: e.name.clone(),
            })
            .collect();
        self.list = list;
        self.reindex();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canon::Entry;

    #[test]
    fn ids_are_unique_ordered_and_readable() {
        let ids: Vec<EntityId> = (0..10_000).map(|_| EntityId::mint()).collect();
        let set: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(set.len(), ids.len());
        assert!(ids.iter().all(|i| i.0.len() == 17 && i.0.starts_with('e')));
        assert!(
            ids.first().unwrap().0[..10] <= ids.last().unwrap().0[..10],
            "time comes first"
        );
    }

    #[test]
    fn a_rename_keeps_the_id_and_the_registry_follows_the_canon() {
        let canon = Modlist::new(
            ["A", "B"]
                .iter()
                .map(|n| Entry {
                    name: n.to_string(),
                    kind: EntryKind::Mod,
                    enabled: true,
                })
                .collect(),
        );
        let mut e = Entities::default();
        e.follow(&canon);
        let a = e.id_of("a").unwrap().clone();
        assert!(e.rename("A", "A v2"));
        assert_eq!(e.id_of("A V2"), Some(&a));
        assert_eq!(e.name_of(&a), Some("A v2"));
        assert!(!e.rename("A v2", "B"), "the name belongs to another entity");
        // round trip through JSON keeps ids and lookups
        let json = serde_json::to_string(&e).unwrap();
        let mut back: Entities = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id_of("a v2"), Some(&a));
        assert!(back.remove("B").is_some());
    }
}
