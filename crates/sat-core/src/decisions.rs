//! Decisions: what the user settled about conflicts — chains marked "done". @sat:decisions
//!
//! Owns: the set of settled chains, keyed by the chain's content signature (`@sat:identity.of`), and the change
//! set a decision produces (`Truth::Decisions`).
//! Not here: chains (`@sat:overwrite.projection`), storing decisions in the project file (next: the project
//! container, ADR 0007), SYNC record decisions (`@sat:sync.records`).
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
pub struct ChainDecisions {
    done: FxHashSet<u64>,
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
}
