//! The user's layout of chains: which exact chains he merged into one card and which he split apart. @sat:layout
//!
//! Owns: the list of user groups of exact chains (by content signature, `@sat:app.chains` exact signatures), and
//! the three edits — merge, split, back to the system's grouping.
//! Not here: the system's grouping (`@sat:routes` — the monolith's normalization, the default), applying the
//! layout to a projection (`RouteGroups::with_layout`), the project file that carries it (`@sat:project`).
//!
//! Owner, 2026-11-15: a chain is the smallest stable view, a context the largest; anything between is not
//! predictable, so the user sets it while he works. The system's normalization stays the default (it detected
//! chains that belong together and merged them); the user can split what it merged and merge what it did not.
//!
//! A group is a set of exact signatures; groups do not overlap. A group of one is "stands alone" (split). An exact
//! chain in no group goes where the system puts it. A signature that no longer exists (members changed) simply
//! matches nothing — the rest of the group still holds, like the monolith's composites (`PARTIAL` keeps the found
//! part). On disk: lists of 16-digit hex signatures, sorted.

use serde::{Deserialize, Serialize};

/// User groups of exact chains.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "Stored", try_from = "Stored")]
pub struct ChainLayout {
    groups: Vec<Vec<u64>>,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    groups: Vec<Vec<String>>,
}

impl From<ChainLayout> for Stored {
    fn from(l: ChainLayout) -> Self {
        Self {
            groups: l
                .groups
                .iter()
                .map(|g| g.iter().map(|s| format!("{s:016x}")).collect())
                .collect(),
        }
    }
}

impl TryFrom<Stored> for ChainLayout {
    type Error = String;
    fn try_from(s: Stored) -> Result<Self, String> {
        let mut l = Self::default();
        for g in s.groups {
            let g: Vec<u64> = g
                .iter()
                .map(|h| u64::from_str_radix(h, 16).map_err(|_| format!("bad chain signature {h:?}")))
                .collect::<Result<_, _>>()?;
            l.push(g);
        }
        Ok(l)
    }
}

impl ChainLayout {
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
    pub fn groups(&self) -> &[Vec<u64>] {
        &self.groups
    }

    /// The group that holds exact signature `s`.
    pub fn group_of(&self, s: u64) -> Option<usize> {
        self.groups.iter().position(|g| g.binary_search(&s).is_ok())
    }

    /// Takes out every group that touches `set`; returns whether any did.
    fn take(&mut self, set: &[u64]) -> bool {
        let before = self.groups.len();
        self.groups.retain(|g| !g.iter().any(|s| set.contains(s)));
        before != self.groups.len()
    }

    fn push(&mut self, mut g: Vec<u64>) {
        g.sort_unstable();
        g.dedup();
        if g.is_empty() {
            return;
        }
        self.take(&g.clone());
        self.groups.push(g);
    }

    /// Merges cards (each a set of exact signatures) into one; returns whether the layout changed.
    pub fn merge(&mut self, cards: &[&[u64]]) -> bool {
        let all: Vec<u64> = cards.iter().flat_map(|c| c.iter().copied()).collect();
        let before = self.clone();
        self.take(&all);
        self.push(all);
        *self != before
    }

    /// Splits a card into its exact chains, each standing alone.
    pub fn split(&mut self, card: &[u64]) -> bool {
        let before = self.clone();
        self.take(card);
        for &s in card {
            self.groups.push(vec![s]);
        }
        *self != before
    }

    /// A chain the user's group knew as `old` is now `new` (its members changed and it was followed,
    /// `@sat:lineage`).
    pub fn rekey(&mut self, old: u64, new: u64) {
        for g in &mut self.groups {
            if let Ok(i) = g.binary_search(&old) {
                g.remove(i);
                if let Err(j) = g.binary_search(&new) {
                    g.insert(j, new);
                }
            }
        }
    }

    /// Forgets chains `keep` refuses (orphans), and groups left with nothing; how many chains went.
    pub fn retain(&mut self, keep: impl Fn(u64) -> bool) -> usize {
        let before: usize = self.groups.iter().map(Vec::len).sum();
        // a merge that lost all but one chain is no merge: the chain goes back to the system (a split chain, alone
        // from the start, stays alone)
        self.groups.retain_mut(|g| {
            let was = g.len();
            g.retain(|&s| keep(s));
            !(g.is_empty() || (was > 1 && g.len() == 1))
        });
        before - self.groups.iter().map(Vec::len).sum::<usize>()
    }

    /// Gives a card's exact chains back to the system's grouping.
    pub fn reset(&mut self, card: &[u64]) -> bool {
        self.take(card)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_split_and_reset_keep_groups_apart() {
        let mut l = ChainLayout::default();
        assert!(l.merge(&[&[3, 1], &[2]]));
        assert_eq!(l.groups(), [vec![1, 2, 3]]);
        assert!(!l.merge(&[&[1, 2, 3]]), "the same group again changes nothing");
        // merging a part with something else takes the old group out whole
        assert!(l.merge(&[&[1], &[9]]));
        assert_eq!(l.groups(), [vec![1, 9]]);
        assert!(l.split(&[1, 9]));
        assert_eq!(l.groups(), [vec![1], vec![9]]);
        assert_eq!(l.group_of(9), Some(1));
        assert!(l.reset(&[1, 9]));
        assert!(l.is_empty());
        assert!(!l.reset(&[1]));
    }

    #[test]
    fn stored_as_sorted_hex() {
        let mut l = ChainLayout::default();
        l.merge(&[&[0xff, 0x1]]);
        let j = serde_json::to_string(&l).unwrap();
        assert_eq!(j, r#"{"groups":[["0000000000000001","00000000000000ff"]]}"#);
        assert_eq!(serde_json::from_str::<ChainLayout>(&j).unwrap(), l);
    }
}
