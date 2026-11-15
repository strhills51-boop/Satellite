//! Lineage of chains: who an exact chain was made of, so the user's work follows a chain whose members changed. @sat:lineage
//!
//! Owns: the members (stable keys) of every exact chain the user touched (settled, merged, split, named), the
//! mapping of a vanished chain to the chain that took its place, and orphans — touched chains that are gone.
//! Not here: the signature itself (`@sat:identity`), what the user did (`@sat:decisions`, `@sat:layout`,
//! `@sat:naming.manual`), applying the mapping (`@sat:app.chains.heads`).
//!
//! An exact chain's signature is the set of its members' stable keys. Update a mod so it joins or leaves a stack
//! and the chain gets a new signature: without lineage the user's merge would silently lose that chain and a name
//! would drop. Here a touched chain that vanished is matched to a new chain (one not touched before) with the most
//! similar members — Jaccard of member sets ≥ [`FOLLOW_AT`], best pairs first, one to one. The user's layout and
//! names follow the match; a settled chain does not stay settled (its members changed — the user looks again) but
//! is shown as "was settled".
//!
//! On disk: `{signature hex: [member keys]}`.

use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

/// Member sets at least this similar are the same chain.
pub const FOLLOW_AT: f64 = 0.5;

/// Members of touched exact chains.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "Stored", try_from = "Stored")]
pub struct ChainLineage {
    members: FxHashMap<u64, Vec<String>>,
}

#[derive(Serialize, Deserialize)]
struct Stored(std::collections::BTreeMap<String, Vec<String>>);

impl From<ChainLineage> for Stored {
    fn from(l: ChainLineage) -> Self {
        Self(
            l.members
                .into_iter()
                .map(|(k, v)| (format!("{k:016x}"), v))
                .collect(),
        )
    }
}

impl TryFrom<Stored> for ChainLineage {
    type Error = String;
    fn try_from(s: Stored) -> Result<Self, String> {
        let members =
            s.0.into_iter()
                .map(|(h, v)| {
                    u64::from_str_radix(&h, 16)
                        .map(|k| (k, v))
                        .map_err(|_| format!("bad chain signature {h:?}"))
                })
                .collect::<Result<_, _>>()?;
        Ok(Self { members })
    }
}

fn jaccard(a: &[String], b: &[String]) -> f64 {
    let sa: FxHashSet<&String> = a.iter().collect();
    let common = b.iter().filter(|x| sa.contains(x)).count();
    let all = a.len() + b.len() - common;
    if all == 0 { 0.0 } else { common as f64 / all as f64 }
}

impl ChainLineage {
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
    pub fn len(&self) -> usize {
        self.members.len()
    }
    pub fn knows(&self, sig: u64) -> bool {
        self.members.contains_key(&sig)
    }

    /// Remembers the members of a touched chain; returns whether it was new.
    pub fn record(&mut self, sig: u64, mut members: Vec<String>) -> bool {
        members.sort_unstable();
        members.dedup();
        self.members.insert(sig, members).is_none()
    }

    /// Where vanished touched chains went: `new signature → old signature`. `current` — every exact chain now,
    /// with its members.
    pub fn follow(&self, current: &[(u64, Vec<String>)]) -> FxHashMap<u64, u64> {
        let now: FxHashSet<u64> = current.iter().map(|c| c.0).collect();
        let gone: Vec<(&u64, &Vec<String>)> = self.members.iter().filter(|(s, _)| !now.contains(s)).collect();
        if gone.is_empty() {
            return FxHashMap::default();
        }
        // candidates: chains nobody touched (a touched one keeps its own history); an index by member cuts pairs
        let mut by_member: FxHashMap<&str, Vec<usize>> = FxHashMap::default();
        for (i, (s, ms)) in current.iter().enumerate() {
            if self.members.contains_key(s) {
                continue;
            }
            for m in ms {
                by_member.entry(m.as_str()).or_default().push(i);
            }
        }
        let mut pairs: Vec<(f64, u64, usize)> = Vec::new();
        for &(&old, ms) in &gone {
            let mut seen: FxHashSet<usize> = FxHashSet::default();
            for m in ms.iter() {
                for &i in by_member.get(m.as_str()).into_iter().flatten() {
                    if seen.insert(i) {
                        let j = jaccard(ms, &current[i].1);
                        if j >= FOLLOW_AT {
                            pairs.push((j, old, i));
                        }
                    }
                }
            }
        }
        pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut out = FxHashMap::default();
        let mut used_old: FxHashSet<u64> = FxHashSet::default();
        for (_, old, i) in pairs {
            let new = current[i].0;
            if !used_old.contains(&old) && !out.contains_key(&new) {
                used_old.insert(old);
                out.insert(new, old);
            }
        }
        out
    }

    /// Forgets one chain (it was taken over by its successor).
    pub fn forget(&mut self, sig: u64) {
        self.members.remove(&sig);
    }

    /// Forgets chains `keep` refuses; returns how many.
    pub fn retain(&mut self, keep: impl Fn(u64) -> bool) -> usize {
        let before = self.members.len();
        self.members.retain(|&s, _| keep(s));
        before - self.members.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_chain_whose_members_changed_is_followed_by_the_most_similar_new_one() {
        let mut l = ChainLineage::default();
        l.record(1, v(&["a", "b", "c"]));
        l.record(2, v(&["x", "y"]));
        // 1 gained d (3/4 similar); 2 is still there; 5 is an unrelated new chain; 6 is too far from 1
        let now = [
            (4, v(&["a", "b", "c", "d"])),
            (2, v(&["x", "y"])),
            (5, v(&["p", "q"])),
            (6, v(&["a", "z", "w", "u"])),
        ];
        let f = l.follow(&now);
        assert_eq!(f.len(), 1);
        assert_eq!(f.get(&4), Some(&1));
        // a touched chain is never taken over by another vanished one
        l.record(4, v(&["a", "b", "c", "d"]));
        assert!(l.follow(&now).is_empty());
        // stored and read back
        let j = serde_json::to_string(&l).unwrap();
        assert_eq!(serde_json::from_str::<ChainLineage>(&j).unwrap(), l);
        assert_eq!(l.retain(|s| s != 1), 1);
    }
}
