//! Canon: the mod manager's own state — order, enabled flags, separators. @sat:canon
//!
//! Manager-agnostic: adapters (`sat-mo2`, later Vortex…) parse into and serialize from these types.
//! Display order is **lowest priority first** (as MO2 shows it).
//!
//! Owns: the serialized snapshot of the canon (`Modlist`, `Entry`), the separator suffix, priorities from order.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::ids::ModId;
use crate::overwrite::Priorities;

// @sat:canon.separator-suffix
/// Suffix that marks a separator: MO2 stores separators as folders and modlist entries named `<name>_separator`,
/// and canon keeps it in [`Entry::name`]. Single owner of this convention (docs/semantic/MAP.md).
pub const SEPARATOR_SUFFIX: &str = "_separator";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntryKind {
    /// A managed mod folder.
    Mod,
    /// A separator: groups the mods below it (in display order) until the next separator.
    Separator,
    /// Something the manager lists but does not manage (MO2 `*` lines: DLC, game files).
    Foreign,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Entry {
    /// Unique key inside the list (MO2: folder name, separators keep their `_separator` suffix).
    pub name: String,
    pub kind: EntryKind,
    pub enabled: bool,
}

impl Entry {
    pub fn is_mod(&self) -> bool {
        self.kind == EntryKind::Mod
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CanonError {
    #[error("no entry named {0:?}")]
    UnknownEntry(String),
    #[error("entry {0:?} cannot be moved")]
    Immovable(String),
}

/// Ordered list of entries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modlist {
    pub entries: Vec<Entry>,
}

impl Modlist {
    pub fn new(entries: Vec<Entry>) -> Self {
        Self { entries }
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.name == name)
    }

    /// Index of the separator owning entry `i` (`None` if above the first separator or a separator itself).
    pub fn separator_of(&self, i: usize) -> Option<usize> {
        if self.entries[i].kind == EntryKind::Separator {
            return None;
        }
        self.entries[..i]
            .iter()
            .rposition(|e| e.kind == EntryKind::Separator)
    }

    /// Indices of a separator's block (the separator and the entries until the next separator).
    pub fn block(&self, sep: usize) -> std::ops::Range<usize> {
        let end = self.entries[sep + 1..]
            .iter()
            .position(|e| e.kind == EntryKind::Separator)
            .map_or(self.entries.len(), |p| sep + 1 + p);
        sep..end
    }

    // @sat:canon.move-entries
    /// Moves entries so they land right before `before` (`None` = end of list), keeping their relative order.
    /// Moving a separator moves its whole block. Returns `false` when nothing changed.
    pub fn move_entries(&mut self, names: &[&str], before: Option<&str>) -> Result<bool, CanonError> {
        let mut pick = vec![false; self.entries.len()];
        for &n in names {
            let i = self
                .index_of(n)
                .ok_or_else(|| CanonError::UnknownEntry(n.to_string()))?;
            if self.entries[i].kind == EntryKind::Foreign {
                return Err(CanonError::Immovable(n.to_string()));
            }
            if self.entries[i].kind == EntryKind::Separator {
                for j in self.block(i) {
                    pick[j] = true;
                }
            } else {
                pick[i] = true;
            }
        }
        if let Some(b) = before {
            let bi = self
                .index_of(b)
                .ok_or_else(|| CanonError::UnknownEntry(b.to_string()))?;
            if pick[bi] {
                return Ok(false);
            }
        }
        let old = std::mem::take(&mut self.entries);
        let (moved, mut rest): (Vec<_>, Vec<_>) =
            old.iter().cloned().enumerate().partition(|(i, _)| pick[*i]);
        let at = match before {
            Some(b) => rest
                .iter()
                .position(|(_, e)| e.name == b)
                .expect("target not picked"),
            None => rest.len(),
        };
        let tail = rest.split_off(at);
        self.entries = rest
            .into_iter()
            .chain(moved)
            .chain(tail)
            .map(|(_, e)| e)
            .collect();
        Ok(self.entries != old)
    }

    /// Sets (or toggles with `None`) the enabled flag of mods. Separators and foreign entries are ignored.
    pub fn set_enabled(&mut self, names: &[&str], enabled: Option<bool>) -> Result<usize, CanonError> {
        let want: HashSet<&str> = names.iter().copied().collect();
        let mut changed = 0;
        for e in self
            .entries
            .iter_mut()
            .filter(|e| e.is_mod() && want.contains(e.name.as_str()))
        {
            let v = enabled.unwrap_or(!e.enabled);
            if e.enabled != v {
                e.enabled = v;
                changed += 1;
            }
        }
        if let Some(missing) = names.iter().find(|n| self.entries.iter().all(|e| e.name != **n)) {
            return Err(CanonError::UnknownEntry(missing.to_string()));
        }
        Ok(changed)
    }

    /// Priorities for a provider table (see [`priorities`]).
    pub fn priorities(
        &self,
        mod_count: usize,
        resolve: impl Fn(&str) -> Option<ModId>,
        top: Option<ModId>,
    ) -> Priorities {
        priorities(
            self.entries.iter().map(|e| (e.name.as_str(), e.kind, e.enabled)),
            mod_count,
            resolve,
            top,
        )
    }
}

// @sat:canon.priorities
/// Priorities for a provider table from entries in display order `(name, kind, enabled)`: mods get their
/// display position (1-based among managed mods); disabled or unresolvable mods are inactive; `top` (e.g.
/// Overwrite) gets the highest priority. The single rule for both the snapshot ([`Modlist`]) and the live book.
pub fn priorities<'a>(
    entries: impl Iterator<Item = (&'a str, EntryKind, bool)>,
    mod_count: usize,
    resolve: impl Fn(&str) -> Option<ModId>,
    top: Option<ModId>,
) -> Priorities {
    let mut p = Priorities::all_inactive(mod_count);
    let mut rank = 0i32;
    for (name, _, enabled) in entries.filter(|e| e.1 == EntryKind::Mod) {
        rank += 1;
        if enabled && let Some(m) = resolve(name) {
            p.0[m.idx()] = rank;
        }
    }
    if let Some(t) = top {
        p.0[t.idx()] = rank + 1;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(spec: &[&str]) -> Modlist {
        // "#Name" separator, "-Name" disabled mod, "*Name" foreign, "Name" enabled mod
        Modlist::new(
            spec.iter()
                .map(|s| {
                    let (kind, enabled, name) = match s.chars().next() {
                        Some('#') => (EntryKind::Separator, false, &s[1..]),
                        Some('-') => (EntryKind::Mod, false, &s[1..]),
                        Some('*') => (EntryKind::Foreign, true, &s[1..]),
                        _ => (EntryKind::Mod, true, *s),
                    };
                    Entry {
                        name: name.into(),
                        kind,
                        enabled,
                    }
                })
                .collect(),
        )
    }
    fn names(l: &Modlist) -> Vec<&str> {
        l.entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn move_mods_and_separator_blocks() {
        let mut l = list(&["*DLC", "#S1", "a", "b", "#S2", "c", "d"]);
        assert!(l.move_entries(&["c"], Some("a")).unwrap());
        assert_eq!(names(&l), ["DLC", "S1", "c", "a", "b", "S2", "d"]);
        assert!(l.move_entries(&["S2"], Some("S1")).unwrap());
        assert_eq!(names(&l), ["DLC", "S2", "d", "S1", "c", "a", "b"]);
        assert!(l.move_entries(&["a", "d"], None).unwrap());
        assert_eq!(names(&l), ["DLC", "S2", "S1", "c", "b", "d", "a"]);
        assert!(!l.move_entries(&["c"], Some("c")).unwrap());
        assert_eq!(
            l.move_entries(&["DLC"], None),
            Err(CanonError::Immovable("DLC".into()))
        );
        assert!(matches!(
            l.move_entries(&["zzz"], None),
            Err(CanonError::UnknownEntry(_))
        ));
    }

    #[test]
    fn separators_and_priorities() {
        let l = list(&["*DLC", "#S1", "a", "-b", "#S2", "c"]);
        assert_eq!(l.separator_of(3), Some(1));
        assert_eq!(l.separator_of(0), None);
        assert_eq!(l.block(1), 1..4);
        let ids = ["a", "b", "c", "OW"];
        let p = l.priorities(
            4,
            |n| ids.iter().position(|x| *x == n).map(ModId::from_idx),
            Some(ModId(3)),
        );
        assert_eq!(p.0, vec![1, -1, 3, 4]);
    }

    #[test]
    fn toggle() {
        let mut l = list(&["#S", "a", "-b"]);
        assert_eq!(l.set_enabled(&["a", "b"], None).unwrap(), 2);
        assert_eq!(
            l.entries.iter().map(|e| e.enabled).collect::<Vec<_>>(),
            [false, false, true]
        );
        assert_eq!(l.set_enabled(&["S"], Some(true)).unwrap(), 0);
    }
}
