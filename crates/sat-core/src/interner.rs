//! Path interning: every directory and every name segment is stored once.
//!
//! A file path is encoded as a [`FileKey`] = `(DirId << 32) | NameId`, so a virtual file system with
//! millions of paths needs one `u64` per path plus the shared segment strings.
//! Matching is case-insensitive (Windows semantics); the first-seen spelling is kept for display.

use rustc_hash::FxHashMap;

use crate::ids::{DirId, NameId};

/// Packed `(dir, name)` key of a file path. Sorting by key groups files by directory.
pub type FileKey = u64;

#[inline]
pub fn file_key(dir: DirId, name: NameId) -> FileKey {
    (u64::from(dir.0) << 32) | u64::from(name.0)
}

#[inline]
pub fn key_dir(k: FileKey) -> DirId {
    DirId((k >> 32) as u32)
}

#[inline]
pub fn key_name(k: FileKey) -> NameId {
    NameId(k as u32)
}

/// Lowercases for case-insensitive matching; ASCII fast path avoids allocation when already lowercase.
fn fold(s: &str) -> std::borrow::Cow<'_, str> {
    if s.bytes().all(|b| !b.is_ascii_uppercase() && b.is_ascii()) {
        std::borrow::Cow::Borrowed(s)
    } else if s.is_ascii() {
        std::borrow::Cow::Owned(s.to_ascii_lowercase())
    } else {
        std::borrow::Cow::Owned(s.to_lowercase())
    }
}

/// Interns path segments into a directory tree rooted at [`DirId`]`(0)` (the virtual `Data` root).
///
/// Memory layout (millions of names): display strings live in one arena; lookup is a map from the hash of
/// the case-folded segment to the first id with that hash, collisions chained through `next_same_hash`.
/// No second copy of each string is kept for the case-insensitive key.
#[derive(Debug, Clone)]
pub struct PathInterner {
    arena: String,
    spans: Vec<(u32, u32)>,
    by_hash: FxHashMap<u64, u32>,
    next_same_hash: Vec<u32>,
    dir_index: FxHashMap<u64, DirId>,
    dir_parent: Vec<DirId>,
    dir_name: Vec<NameId>,
}

const END: u32 = u32::MAX;

fn hash_folded(folded: &str) -> u64 {
    use std::hash::Hasher;
    let mut h = rustc_hash::FxHasher::default();
    h.write(folded.as_bytes());
    h.finish()
}

fn folded_eq(display: &str, folded: &str) -> bool {
    if display.is_ascii() && folded.is_ascii() {
        display.len() == folded.len() && display.eq_ignore_ascii_case(folded)
    } else {
        display.to_lowercase() == folded
    }
}

impl Default for PathInterner {
    fn default() -> Self {
        Self::new()
    }
}

impl PathInterner {
    pub const ROOT: DirId = DirId(0);

    pub fn new() -> Self {
        let mut s = Self {
            arena: String::new(),
            spans: Vec::new(),
            by_hash: FxHashMap::default(),
            next_same_hash: Vec::new(),
            dir_index: FxHashMap::default(),
            dir_parent: vec![DirId(0)],
            dir_name: vec![NameId(0)],
        };
        s.name("");
        s
    }

    pub fn name_count(&self) -> usize {
        self.spans.len()
    }

    pub fn dir_count(&self) -> usize {
        self.dir_parent.len()
    }

    /// Approximate heap bytes held by the interner (for budgets).
    pub fn heap_bytes(&self) -> usize {
        self.arena.capacity()
            + self.spans.capacity() * 8
            + self.next_same_hash.capacity() * 4
            + self.by_hash.capacity() * 16
            + self.dir_index.capacity() * 16
            + self.dir_parent.capacity() * 8
    }

    fn lookup(&self, folded: &str, h: u64) -> Option<NameId> {
        let mut cur = *self.by_hash.get(&h)?;
        while cur != END {
            if folded_eq(self.segment(NameId(cur)), folded) {
                return Some(NameId(cur));
            }
            cur = self.next_same_hash[cur as usize];
        }
        None
    }

    /// Interns one segment (case-insensitive).
    pub fn name(&mut self, segment: &str) -> NameId {
        let folded = fold(segment);
        let h = hash_folded(&folded);
        if let Some(id) = self.lookup(&folded, h) {
            return id;
        }
        let id = self.spans.len() as u32;
        let start = self.arena.len() as u32;
        self.arena.push_str(segment);
        self.spans.push((start, segment.len() as u32));
        let prev = self.by_hash.insert(h, id).unwrap_or(END);
        self.next_same_hash.push(prev);
        NameId(id)
    }

    /// Looks a segment up without inserting.
    pub fn find_name(&self, segment: &str) -> Option<NameId> {
        let folded = fold(segment);
        self.lookup(&folded, hash_folded(&folded))
    }

    /// Interns a relative directory path (`/` or `\` separated; empty = root).
    pub fn dir(&mut self, rel: &str) -> DirId {
        let mut cur = Self::ROOT;
        for seg in rel.split(['/', '\\']).filter(|s| !s.is_empty()) {
            let n = self.name(seg);
            let k = (u64::from(cur.0) << 32) | u64::from(n.0);
            cur = match self.dir_index.get(&k) {
                Some(&d) => d,
                None => {
                    let d = DirId::from_idx(self.dir_parent.len());
                    self.dir_index.insert(k, d);
                    self.dir_parent.push(cur);
                    self.dir_name.push(n);
                    d
                }
            };
        }
        cur
    }

    /// Finds a directory without inserting.
    pub fn find_dir(&self, rel: &str) -> Option<DirId> {
        let mut cur = Self::ROOT;
        for seg in rel.split(['/', '\\']).filter(|s| !s.is_empty()) {
            let n = self.find_name(seg)?;
            cur = *self.dir_index.get(&((u64::from(cur.0) << 32) | u64::from(n.0)))?;
        }
        Some(cur)
    }

    pub fn segment(&self, n: NameId) -> &str {
        let (start, len) = self.spans[n.idx()];
        &self.arena[start as usize..(start + len) as usize]
    }

    pub fn parent(&self, d: DirId) -> DirId {
        self.dir_parent[d.idx()]
    }

    pub fn dir_segment(&self, d: DirId) -> NameId {
        self.dir_name[d.idx()]
    }

    /// Display path of a directory (`""` for root).
    pub fn dir_path(&self, d: DirId) -> String {
        let mut parts = Vec::new();
        let mut cur = d;
        while cur != Self::ROOT {
            parts.push(self.segment(self.dir_name[cur.idx()]));
            cur = self.dir_parent[cur.idx()];
        }
        parts.reverse();
        parts.join("/")
    }

    /// Display path of a file key.
    pub fn key_path(&self, k: FileKey) -> String {
        let dir = self.dir_path(key_dir(k));
        let name = self.segment(key_name(k));
        if dir.is_empty() {
            name.to_string()
        } else {
            format!("{dir}/{name}")
        }
    }

    /// True if `d` is `ancestor` or below it.
    pub fn is_within(&self, mut d: DirId, ancestor: DirId) -> bool {
        loop {
            if d == ancestor {
                return true;
            }
            if d == Self::ROOT {
                return false;
            }
            d = self.dir_parent[d.idx()];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_insensitive_and_display_preserving() {
        let mut i = PathInterner::new();
        let a = i.dir("Textures/Armor");
        let b = i.dir("textures\\ARMOR\\");
        assert_eq!(a, b);
        assert_eq!(i.dir_path(a), "Textures/Armor");
        let n1 = i.name("Cuirass.DDS");
        let n2 = i.name("cuirass.dds");
        assert_eq!(n1, n2);
        assert_eq!(i.key_path(file_key(a, n1)), "Textures/Armor/Cuirass.DDS");
        assert_eq!(i.find_dir("TEXTURES/armor"), Some(a));
        assert_eq!(i.find_dir("textures/weapons"), None);
        assert!(i.is_within(a, i.find_dir("textures").unwrap()));
        assert_eq!(i.dir(""), PathInterner::ROOT);
    }

    #[test]
    fn unicode_folding() {
        let mut i = PathInterner::new();
        assert_eq!(i.name("Ёлка.dds"), i.name("ёлка.DDS"));
    }
}
