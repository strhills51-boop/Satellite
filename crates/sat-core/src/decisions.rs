//! Decisions: what the user settled about conflicts — chains marked "done". @sat:decisions
//!
//! Owns: the set of settled chains, keyed by the chain's content signature (`@sat:identity.of`), and the change
//! set a decision produces (`Truth::Decisions`).
//! Not here: chains (`@sat:overwrite.projection`), the project file that carries them (`@sat:project`), SYNC
//! record decisions (`@sat:sync.records`).
//!
//! On disk the set is a sorted list of 16-digit hex signatures: stable diffs, and no precision loss in readers whose
//! numbers are doubles (JavaScript).
//!
//! A decision survives reorders, re-scans and renames because the signature depends only on the chain's members,
//! and a member is its entity id (`@sat:session.member-keys`), not its folder name. When a
//! chain changes members (a mod added or removed from the stack), it is a new chain with no decision — the user
//! looks at it again. That is the monolith's reconcile rule (matched / new / removed) without its defect: the old
//! Satellite's signature depended on load order and lost every decision after a reorder.
//!
//! A decision also keeps a **stamp** of what the chain was when it was settled (`@sat:decisions.stamp`): its paths
//! (count and an order-free hash) and its winner. Reorders, updates of a mod's files and renames do not change the
//! signature, so the decision stays — but if the paths or the winner differ from the stamp, the chain is shown as
//! "changed since settled" for the user to confirm (the monolith's truth fingerprint held owners and files, and a
//! change reset the decision; here it is kept and flagged, nothing is lost silently). Marking it done again renews
//! the stamp. Decisions from before stamps have none and are never flagged.

use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use crate::engine::{Change, Keys, Truth};

// @sat:decisions.chains
/// Settled chains by content signature.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "Stored", try_from = "Stored")]
pub struct ChainDecisions {
    done: FxHashSet<u64>,
    stamps: FxHashMap<u64, Stamp>,
}

// @sat:decisions.stamp
/// What a chain was when it was settled.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    /// Its paths.
    pub paths: u32,
    /// Order-free hash of its paths (`paths_hash`).
    #[serde(with = "hex")]
    pub hash: u64,
    /// The winner's stable key.
    pub winner: String,
}

/// What differs from the stamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Drift {
    /// Paths then and now (equal counts with a different hash: the same number, other paths).
    pub paths: (u32, u32),
    pub paths_changed: bool,
    /// The winner then, when it changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winner_was: Option<String>,
}

/// Order-free hash of a chain's paths (lower-cased): XOR of per-path FNV — a path set, not a list.
pub fn paths_hash<'a>(paths: impl IntoIterator<Item = &'a str>) -> u64 {
    let mut acc = 0u64;
    for p in paths {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in p.bytes() {
            h = (h ^ u64::from(b.to_ascii_lowercase())).wrapping_mul(0x0100_0000_01b3);
        }
        acc ^= h.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
    acc
}

mod hex {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{v:016x}"))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        let t = String::deserialize(d)?;
        u64::from_str_radix(&t, 16).map_err(serde::de::Error::custom)
    }
}

#[derive(Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    done: Vec<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    stamps: std::collections::BTreeMap<String, Stamp>,
}

impl From<ChainDecisions> for Stored {
    fn from(d: ChainDecisions) -> Self {
        let mut v: Vec<u64> = d.done.into_iter().collect();
        v.sort_unstable();
        Self {
            done: v.into_iter().map(|s| format!("{s:016x}")).collect(),
            stamps: d
                .stamps
                .into_iter()
                .map(|(k, v)| (format!("{k:016x}"), v))
                .collect(),
        }
    }
}

impl TryFrom<Stored> for ChainDecisions {
    type Error = String;
    fn try_from(s: Stored) -> Result<Self, String> {
        let done = s
            .done
            .iter()
            .map(|h| u64::from_str_radix(h, 16).map_err(|_| format!("bad chain signature {h:?}")))
            .collect::<Result<_, _>>()?;
        let stamps = s
            .stamps
            .into_iter()
            .map(|(h, v)| {
                u64::from_str_radix(&h, 16)
                    .map(|k| (k, v))
                    .map_err(|_| format!("bad chain signature {h:?}"))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { done, stamps })
    }
}

impl ChainDecisions {
    pub fn is_done(&self, signature: u64) -> bool {
        self.done.contains(&signature)
    }

    /// Marks or unmarks a chain; `None` when nothing changed.
    pub fn set(&mut self, signature: u64, done: bool) -> Option<Change> {
        let changed = if done {
            self.done.insert(signature)
        } else {
            self.stamps.remove(&signature);
            self.done.remove(&signature)
        };
        changed.then_some(Change {
            truth: Truth::Decisions,
            keys: Keys::All,
        })
    }

    /// Marks a chain done with what it is now (renews the stamp of a chain already done).
    pub fn set_stamped(&mut self, signature: u64, stamp: Stamp) -> Option<Change> {
        let a = self.done.insert(signature);
        let b = self.stamps.insert(signature, stamp.clone()).as_ref() != Some(&stamp);
        (a || b).then_some(Change {
            truth: Truth::Decisions,
            keys: Keys::All,
        })
    }

    /// How a settled chain differs from its stamp; `None` when it is not settled, has no stamp, or is the same.
    pub fn drift(&self, signature: u64, now: &Stamp) -> Option<Drift> {
        if !self.done.contains(&signature) {
            return None;
        }
        let was = self.stamps.get(&signature)?;
        let paths_changed = was.paths != now.paths || was.hash != now.hash;
        let winner_was = (was.winner != now.winner).then(|| was.winner.clone());
        (paths_changed || winner_was.is_some()).then_some(Drift {
            paths: (was.paths, now.paths),
            paths_changed,
            winner_was,
        })
    }

    /// Settled signatures.
    pub fn signatures(&self) -> impl Iterator<Item = u64> + '_ {
        self.done.iter().copied()
    }

    /// Drops decisions whose signature `keep` refuses (orphans); returns how many.
    pub fn retain(&mut self, keep: impl Fn(u64) -> bool) -> usize {
        let before = self.done.len();
        self.done.retain(|&s| keep(s));
        let done = &self.done;
        self.stamps.retain(|s, _| done.contains(s));
        before - self.done.len()
    }

    pub fn len(&self) -> usize {
        self.done.len()
    }

    pub fn is_empty(&self) -> bool {
        self.done.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::identity;

    #[test]
    fn a_decision_follows_the_chain_not_the_order() {
        let mut d = ChainDecisions::default();
        let sig = identity(["B", "A", "C"]).signature;
        assert!(d.set(sig, true).is_some());
        assert!(d.set(sig, true).is_none(), "no change, no change set");
        // the same members in another order (after a reorder) are the same chain
        assert!(d.is_done(identity(["C", "B", "A"]).signature));
        // a member more is a new chain: not settled
        assert!(!d.is_done(identity(["A", "B", "C", "D"]).signature));
        assert!(d.set(sig, false).is_some());
        assert!(d.is_empty());
    }

    #[test]
    fn stored_as_sorted_hex() {
        let mut d = ChainDecisions::default();
        d.set(u64::MAX - 1, true);
        d.set(5, true);
        let j = serde_json::to_string(&d).unwrap();
        assert_eq!(j, r#"{"done":["0000000000000005","fffffffffffffffe"]}"#);
        assert_eq!(serde_json::from_str::<ChainDecisions>(&j).unwrap(), d);
        assert!(serde_json::from_str::<ChainDecisions>(r#"{"done":["xyz"]}"#).is_err());
    }

    #[test]
    fn a_stamp_flags_what_changed_since_settled() {
        let mut d = ChainDecisions::default();
        let st = |n: u32, ps: &[&str], w: &str| Stamp {
            paths: n,
            hash: paths_hash(ps.iter().copied()),
            winner: w.into(),
        };
        d.set_stamped(7, st(2, &["a.dds", "b.dds"], "e1"));
        assert_eq!(
            d.drift(7, &st(2, &["B.dds", "a.dds"], "e1")),
            None,
            "order and case of paths do not matter"
        );
        let dr = d.drift(7, &st(3, &["a.dds", "b.dds", "c.dds"], "e2")).unwrap();
        assert_eq!(
            (dr.paths, dr.paths_changed, dr.winner_was.as_deref()),
            ((2, 3), true, Some("e1"))
        );
        assert!(
            d.drift(7, &st(2, &["a.dds", "x.dds"], "e1"))
                .unwrap()
                .paths_changed
        );
        // renewing the stamp settles it again
        d.set_stamped(7, st(3, &["a.dds", "b.dds", "c.dds"], "e2"));
        assert_eq!(d.drift(7, &st(3, &["a.dds", "b.dds", "c.dds"], "e2")), None);
        // stored and read back
        let j = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<ChainDecisions>(&j).unwrap(), d);
        // old decisions have no stamp: never flagged
        d.set(9, true);
        assert_eq!(d.drift(9, &st(1, &["z"], "x")), None);
        assert_eq!(d.retain(|s| s != 7), 1);
        assert!(!d.is_done(7));
    }
}
