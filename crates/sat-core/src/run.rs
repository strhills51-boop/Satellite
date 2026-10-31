//! Run lifecycle and result generations (NG-D002 p.1c; monolith Run Contract CP2 174165 and generations CP4 168499).
//!
//! - [`RunStatus`] and [`RunStatus::can_go`] — the monolith's transition table (12 statuses).
//! - [`Authority`] holds the authoritative result; a run builds a [`Staging`] generation and either **promotes** it
//!   (seals pass, the authority did not move meanwhile) or drops it (aborted). Readers never see a half-built result.
//! - [`seal`] — invariants checked before promotion (monolith seals v507/v513/v517/v518 and conflict invariants),
//!   done **before** publication; the monolith sealed the metric summary after promoting (05-run §5 p.9).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Status of a run (monolith `modlisttool.authoritative-run-contract.v1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunStatus {
    Created,
    Planning,
    Running,
    Blocked,
    Staging,
    AwaitingCommit,
    Committing,
    Verifying,
    Promoting,
    Completed,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Allowed transitions (`AUTH_ALLOWED` 174188); staying in place is always allowed.
    pub fn can_go(self, to: Self) -> bool {
        use RunStatus::*;
        if self == to {
            return true;
        }
        let allowed: &[RunStatus] = match self {
            Created => &[Planning, Running, Blocked, Cancelled, Failed],
            Planning => &[Running, Blocked, Staging, Cancelled, Failed],
            Running => &[Planning, Blocked, Staging, Verifying, Cancelled, Failed],
            Blocked => &[Planning, Running, Staging, AwaitingCommit, Cancelled, Failed],
            Staging => &[Blocked, AwaitingCommit, Committing, Cancelled, Failed],
            AwaitingCommit => &[Blocked, Committing, Cancelled, Failed],
            Committing => &[Verifying, Failed],
            Verifying => &[Promoting, Failed],
            Promoting => &[Completed, Failed],
            Completed | Failed | Cancelled => &[],
        };
        allowed.contains(&to)
    }
}

/// A run's status with its history; refuses illegal transitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunState {
    pub status: RunStatus,
    /// `(from, to, reason)`.
    pub history: Vec<(RunStatus, RunStatus, String)>,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            status: RunStatus::Created,
            history: Vec::new(),
        }
    }
}

impl RunState {
    pub fn go(&mut self, to: RunStatus, reason: impl Into<String>) -> Result<(), RunError> {
        if !self.status.can_go(to) {
            return Err(RunError::Transition {
                from: self.status,
                to,
            });
        }
        if self.status != to {
            self.history.push((self.status, to, reason.into()));
            self.status = to;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunError {
    #[error("run cannot go from {from:?} to {to:?}")]
    Transition { from: RunStatus, to: RunStatus },
    #[error(
        "the authoritative result changed during the run (expected generation {expected:?}, found {found:?})"
    )]
    ParentChanged {
        expected: Option<u64>,
        found: Option<u64>,
    },
    #[error("seals failed: {0:?}")]
    Seals(Vec<String>),
}

/// A published result.
#[derive(Debug)]
pub struct Generation<T> {
    pub id: u64,
    pub parent: Option<u64>,
    pub data: T,
}

/// A result being built; dropping it aborts the generation.
#[derive(Debug)]
pub struct Staging<T> {
    id: u64,
    parent: Option<u64>,
    pub data: T,
}

impl<T> Staging<T> {
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn parent(&self) -> Option<u64> {
        self.parent
    }
}

/// The authoritative result of some kind (e.g. an analysis snapshot), replaced atomically.
#[derive(Debug)]
pub struct Authority<T> {
    current: Option<Arc<Generation<T>>>,
    next_id: u64,
}

impl<T> Default for Authority<T> {
    fn default() -> Self {
        Self {
            current: None,
            next_id: 1,
        }
    }
}

impl<T> Authority<T> {
    pub fn current(&self) -> Option<Arc<Generation<T>>> {
        self.current.clone()
    }

    /// Starts a generation on top of the current one.
    pub fn begin(&mut self, data: T) -> Staging<T> {
        let id = self.next_id;
        self.next_id += 1;
        Staging {
            id,
            parent: self.current.as_ref().map(|g| g.id),
            data,
        }
    }

    /// Publishes a staging generation if its seals pass and nothing was published since it began
    /// (optimistic check of the parent, monolith CP4). On error the staging data is returned.
    pub fn promote(
        &mut self,
        staging: Staging<T>,
        seals: &[String],
    ) -> Result<Arc<Generation<T>>, (RunError, Staging<T>)> {
        let found = self.current.as_ref().map(|g| g.id);
        if found != staging.parent {
            return Err((
                RunError::ParentChanged {
                    expected: staging.parent,
                    found,
                },
                staging,
            ));
        }
        if !seals.is_empty() {
            return Err((RunError::Seals(seals.to_vec()), staging));
        }
        let g = Arc::new(Generation {
            id: staging.id,
            parent: staging.parent,
            data: staging.data,
        });
        self.current = Some(g.clone());
        Ok(g)
    }
}

/// How one mod was obtained in a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StagedRoute {
    Scan,
    Indexed,
    Reuse,
}

/// What a run staged for one mod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedMod {
    pub name: String,
    pub route: StagedRoute,
    pub missing: bool,
    /// A mandatory analyzer failed.
    pub error: Option<String>,
}

/// Facts the seals look at.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    /// Mods the canon expects (enabled or not) — staged results must cover exactly these.
    pub expected: BTreeSet<String>,
    pub staged: Vec<StagedMod>,
    pub chains: usize,
    pub contexts: usize,
    pub conflict_paths: usize,
}

/// Invariants before promotion; returns the violations (empty = sealed).
///
/// 1. Staged mods are exactly the expected ones, each once (v507 ledger).
///
/// A mod with an analyzer error (unreadable subfolder) does not break a seal: it is degraded and reported — the
/// monolith failed the whole run on one bad archive, NG does not (05-run §6).
/// 2. Conflict invariants (05-run §4.4 p.7): no chains ⇒ no contexts; contexts ≤ chains; chains ≤ conflict paths.
pub fn seal(s: &RunSummary) -> Vec<String> {
    let mut v = Vec::new();
    let mut seen = BTreeSet::new();
    for m in &s.staged {
        let key = m.name.to_lowercase();
        if !seen.insert(key) {
            v.push(format!("staged twice: {}", m.name));
        }
    }
    let expected: BTreeSet<String> = s.expected.iter().map(|n| n.to_lowercase()).collect();
    for e in expected.difference(&seen) {
        v.push(format!("not staged: {e}"));
    }
    for e in seen.difference(&expected) {
        v.push(format!("staged but not expected: {e}"));
    }
    if s.chains == 0 && s.contexts > 0 {
        v.push(format!("{} contexts without chains", s.contexts));
    }
    if s.contexts > s.chains {
        v.push(format!("contexts ({}) > chains ({})", s.contexts, s.chains));
    }
    if s.chains > s.conflict_paths {
        v.push(format!(
            "chains ({}) > conflict paths ({})",
            s.chains, s.conflict_paths
        ));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitions_follow_the_monolith_table() {
        let mut r = RunState::default();
        r.go(RunStatus::Running, "scan").unwrap();
        assert!(r.go(RunStatus::Completed, "skip").is_err());
        r.go(RunStatus::Staging, "built").unwrap();
        r.go(RunStatus::Committing, "seal").unwrap();
        r.go(RunStatus::Verifying, "").unwrap();
        r.go(RunStatus::Promoting, "").unwrap();
        r.go(RunStatus::Completed, "").unwrap();
        assert!(r.status.is_terminal());
        assert!(r.go(RunStatus::Running, "again").is_err());
        assert_eq!(r.history.len(), 6);
    }

    #[test]
    fn promote_checks_parent_and_seals() {
        let mut a = Authority::default();
        let s1 = a.begin("first");
        let s2 = a.begin("concurrent");
        let g1 = a.promote(s1, &[]).unwrap();
        assert_eq!(g1.parent, None);
        // s2 started before g1 was published
        let (e, s2) = a.promote(s2, &[]).unwrap_err();
        assert!(matches!(
            e,
            RunError::ParentChanged {
                expected: None,
                found: Some(_)
            }
        ));
        assert_eq!(s2.data, "concurrent");
        let s3 = a.begin("third");
        let (e, _) = a.promote(s3, &["not staged: x".into()]).unwrap_err();
        assert!(matches!(e, RunError::Seals(_)));
        assert_eq!(
            a.current().unwrap().data,
            "first",
            "failed promotions publish nothing"
        );
    }

    #[test]
    fn seals() {
        let m = |n: &str| StagedMod {
            name: n.into(),
            route: StagedRoute::Reuse,
            missing: false,
            error: None,
        };
        let mut s = RunSummary {
            expected: ["A".to_string(), "B".to_string()].into(),
            staged: vec![m("A"), m("b")],
            chains: 2,
            contexts: 1,
            conflict_paths: 10,
        };
        assert!(seal(&s).is_empty());
        s.staged.push(m("C"));
        s.staged.push(m("a"));
        s.staged[1].error = Some("unreadable".into()); // degraded, not a seal
        s.contexts = 3;
        let v = seal(&s);
        assert!(v.iter().any(|x| x.starts_with("staged twice: a")));
        assert!(!v.iter().any(|x| x.contains("unreadable")));
        assert!(v.iter().any(|x| x == "staged but not expected: c"));
        assert!(v.iter().any(|x| x.starts_with("contexts (3) > chains (2)")));
    }
}
