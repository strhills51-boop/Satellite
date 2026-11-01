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
//! A decision survives reorders and re-scans because the signature depends only on the chain's members. When a
//! chain changes members (a mod added or removed from the stack), it is a new chain with no decision — the user
//! looks at it again. That is the monolith's reconcile rule (matched / new / removed) without its defect: the old
//! Satellite's signature depended on load order and lost every decision after a reorder.

use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};

use crate::engine::{Change, Keys, Truth};

// @sat:decisions.chains
/// Settled chains by content signature.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "Stored", try_from = "Stored")]
pub struct ChainDecisions {
    done: FxHashSet<u64>,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    done: Vec<String>,
}

impl From<ChainDecisions> for Stored {
    fn from(d: ChainDecisions) -> Self {
        let mut v: Vec<u64> = d.done.into_iter().collect();
        v.sort_unstable();
        Self {
            done: v.into_iter().map(|s| format!("{s:016x}")).collect(),
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
        Ok(Self { done })
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
            self.done.remove(&signature)
        };
        changed.then_some(Change {
            truth: Truth::Decisions,
            keys: Keys::All,
        })
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
}
