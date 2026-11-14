//! Conflict domains in one shape: centrality, semantics, chains, contexts, counts (ADR 0017, ADR 0018). @sat:conflict
//!
//! Owns: what a conflict domain says about itself (`DomainInfo`: family, centrality — where the order of the
//! participants comes from —, interaction semantics, where it comes from), and its output in one shape every
//! projection reads: participants, targets, chains (one stack of participants over its targets, a cell per
//! participant × target), contexts (chains tied by shared participants) and counts. A `Builder` turns contests —
//! one target with its participants and their cells — into that shape.
//! Not here: any one domain (files `@sat:overwrite`, records `@sat:records`, specs `@sat:conflict.spec`, the built-in
//! kinds `@sat:conflict.kinds`), and the projections themselves (cards, table — `@sat:app.domains`).
//!
//! A domain without a winner (constraint, additive) has none here either: cells say what each participant is, the
//! stack is just its participants. Equal ranks under a winner semantics are a tie — unresolved, not a win.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

/// What kind of conflict (manifest 19.8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    /// One target, competing participants; a winner may exist.
    Collision,
    /// Different parts of one target; the right result is a composition.
    Merge,
    /// Participants act on each other (frameworks, runtime rules).
    Interaction,
    /// A requirement or a limit is broken; nothing wins.
    Constraint,
}

/// Where the order of a target's participants comes from (the owner's "централь").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Centrality {
    /// The manager's mod priority.
    ModOrder,
    /// The plugin load order.
    PluginOrder,
    /// A number in a participant's own file (a JSON pointer or an INI key).
    Field,
    /// The participant's name, alphabetically.
    Name,
    /// No order.
    None,
}

/// How participants of one target interact (ADR 0017 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Semantics {
    /// The last in the order takes the target.
    LastWins,
    /// The highest rank takes it.
    HighestWins,
    /// The lowest rank takes it.
    LowestWins,
    /// The highest rank takes it while its conditions hold, the next one otherwise.
    Conditional,
    /// Different parts compose; the winner carries only its own.
    Mergeable,
    /// Everything adds up.
    Additive,
    /// Only one may be there at all.
    Exclusive,
    /// A requirement or a limit.
    Constraint,
}

impl Semantics {
    /// Whether a target has one winner under this semantics.
    pub fn has_winner(self) -> bool {
        matches!(
            self,
            Self::LastWins | Self::HighestWins | Self::LowestWins | Self::Conditional
        )
    }
}

/// Where a domain comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// An adapter in code.
    Builtin,
    /// A spec of the game pack (`@sat:game`).
    Pack,
    /// A spec the user added.
    User,
}

/// How bad a chain is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warn,
    Error,
}

/// What a domain says about itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainInfo {
    pub id: String,
    pub title: String,
    /// One line: what the domain finds.
    pub about: String,
    pub family: Family,
    pub centrality: Centrality,
    pub semantics: Semantics,
    pub origin: Origin,
}

/// A cell: what participant does at a target.
pub mod cell {
    /// Takes the target.
    pub const WIN: u8 = b'w';
    /// Loses it.
    pub const LOSE: u8 = b'l';
    /// Loses it with the winner's very content (nothing lost).
    pub const SAME: u8 = b's';
    /// The target's own source (a record's plugin).
    pub const ORIGIN: u8 = b'o';
    /// Ties for the top.
    pub const TIE: u8 = b't';
    /// Breaks the requirement or the limit.
    pub const BREAKS: u8 = b'x';
    /// Takes part without an order (additive, constraint's other side).
    pub const MEMBER: u8 = b'm';
    /// Not there.
    pub const NONE: u8 = b'-';
}

/// A participant: a mod, a plugin, a submod, a rule file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Participant {
    pub label: String,
    /// The mod it comes from, by name (a plugin's or a submod's owner; a mod itself).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Row order in tables (lower first): mod priority, load index, name order.
    pub rank: i64,
}

/// One stack of participants over its targets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chain {
    /// Participants, the lowest first, the winner (if any) last.
    pub stack: Vec<u32>,
    pub targets: Vec<u32>,
    /// Row-major: `cells[m * targets.len() + t]` for member `m` of the stack and target `t`.
    pub cells: Vec<u8>,
    /// What kind of case it is, in words (`ничья`, `нет мастера`, `вслепую`, …).
    pub kind: String,
    pub severity: Severity,
    /// The case wants a decision (a tie, a real field conflict, a broken requirement).
    pub unresolved: bool,
}

impl Chain {
    pub fn cell(&self, member: usize, target: usize) -> u8 {
        self.cells[member * self.targets.len() + target]
    }

    /// Cells of one member, one per target.
    pub fn row(&self, member: usize) -> &[u8] {
        let n = self.targets.len();
        &self.cells[member * n..(member + 1) * n]
    }
}

/// Chains tied by shared participants (a record's own source does not tie).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Context {
    /// Chain indices, the largest first.
    pub chains: Vec<u32>,
    /// Participants, by rank.
    pub participants: Vec<u32>,
    pub targets: u32,
    pub unresolved: u32,
    pub severity: Severity,
}

/// Totals of a domain.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Counts {
    /// Targets with a case.
    pub targets: u32,
    /// Targets whose case wants a decision.
    pub unresolved: u32,
    /// Targets by kind.
    pub kinds: Vec<(String, u32)>,
}

/// A domain's output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainReport {
    pub info: DomainInfo,
    pub participants: Vec<Participant>,
    pub targets: Vec<String>,
    /// Severity first, unresolved first, larger first.
    pub chains: Vec<Chain>,
    /// Larger first.
    pub contexts: Vec<Context>,
    pub counts: Counts,
    /// What the domain could not read (files, plugins), said once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl DomainReport {
    pub fn empty(info: DomainInfo) -> Self {
        Builder::new(info).finish()
    }

    /// The chain's name: its kind and its participants, lowest → highest.
    pub fn chain_name(&self, c: &Chain) -> String {
        let names: Vec<&str> = c
            .stack
            .iter()
            .map(|&p| self.participants[p as usize].label.as_str())
            .collect();
        if c.kind.is_empty() {
            names.join(" -> ")
        } else {
            format!("{} · {}", c.kind, names.join(" -> "))
        }
    }
}

type ChainKey = (Vec<u32>, String, Severity, bool);

// @sat:conflict.builder
#[derive(Debug)]
/// Folds contests into chains: contests with the same stack, kind, severity and resolution share a chain.
pub struct Builder {
    info: DomainInfo,
    participants: Vec<Participant>,
    by_label: FxHashMap<String, u32>,
    targets: Vec<String>,
    chains: Vec<Chain>,
    by_key: FxHashMap<ChainKey, u32>,
    notes: Vec<String>,
}

impl Builder {
    pub fn new(info: DomainInfo) -> Self {
        Self {
            info,
            participants: Vec::new(),
            by_label: FxHashMap::default(),
            targets: Vec::new(),
            chains: Vec::new(),
            by_key: FxHashMap::default(),
            notes: Vec::new(),
        }
    }

    /// The participant labelled `label` (made on first use; a later owner or rank does not change it).
    pub fn participant(&mut self, label: &str, owner: Option<&str>, rank: i64) -> u32 {
        if let Some(&p) = self.by_label.get(label) {
            return p;
        }
        let p = self.participants.len() as u32;
        self.participants.push(Participant {
            label: label.to_string(),
            owner: owner.map(str::to_string),
            rank,
        });
        self.by_label.insert(label.to_string(), p);
        p
    }

    pub fn note(&mut self, s: impl Into<String>) {
        self.notes.push(s.into());
    }

    // @sat:conflict.contest
    /// One target and its participants in stack order (lowest first) with their cells.
    pub fn contest(
        &mut self,
        target: &str,
        members: &[(u32, u8)],
        kind: &str,
        severity: Severity,
        unresolved: bool,
    ) {
        if members.is_empty() {
            return;
        }
        let t = self.targets.len() as u32;
        self.targets.push(target.to_string());
        let stack: Vec<u32> = members.iter().map(|m| m.0).collect();
        let key = (stack.clone(), kind.to_string(), severity, unresolved);
        let c = match self.by_key.get(&key) {
            Some(&c) => c as usize,
            None => {
                self.chains.push(Chain {
                    stack,
                    targets: Vec::new(),
                    cells: Vec::new(),
                    kind: kind.to_string(),
                    severity,
                    unresolved,
                });
                self.by_key.insert(key, (self.chains.len() - 1) as u32);
                self.chains.len() - 1
            }
        };
        let ch = &mut self.chains[c];
        // cells are row-major per member: widen every row by one
        let n = ch.targets.len();
        let mut cells = Vec::with_capacity(ch.cells.len() + members.len());
        for (m, member) in members.iter().enumerate() {
            cells.extend_from_slice(&ch.cells[m * n..(m + 1) * n]);
            cells.push(member.1);
        }
        ch.cells = cells;
        ch.targets.push(t);
    }

    pub fn finish(self) -> DomainReport {
        let Builder {
            info,
            participants,
            targets,
            mut chains,
            notes,
            ..
        } = self;
        chains.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then(b.unresolved.cmp(&a.unresolved))
                .then(b.targets.len().cmp(&a.targets.len()))
                .then(a.stack.cmp(&b.stack))
        });
        // counts
        let mut kinds: FxHashMap<&str, u32> = FxHashMap::default();
        let mut counts = Counts::default();
        for c in &chains {
            let n = c.targets.len() as u32;
            counts.targets += n;
            if c.unresolved {
                counts.unresolved += n;
            }
            *kinds.entry(c.kind.as_str()).or_default() += n;
        }
        let mut kinds: Vec<(String, u32)> = kinds.into_iter().map(|(k, n)| (k.to_string(), n)).collect();
        kinds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        counts.kinds = kinds;
        let contexts = contexts(&participants, &chains);
        DomainReport {
            info,
            participants,
            targets,
            chains,
            contexts,
            counts,
            notes,
        }
    }
}

fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        parent[x as usize] = parent[parent[x as usize] as usize];
        x = parent[x as usize];
    }
    x
}

/// Contexts of chains (after a domain reorders its chains).
pub fn contexts_of(participants: &[Participant], chains: &[Chain]) -> Vec<Context> {
    contexts(participants, chains)
}

// @sat:conflict.contexts
/// Chains tied by a shared participant that is not just a target's source (`o` in every cell of its row).
fn contexts(participants: &[Participant], chains: &[Chain]) -> Vec<Context> {
    let n = participants.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    let ties = |c: &Chain, m: usize| c.row(m).iter().any(|&x| x != cell::ORIGIN && x != cell::NONE);
    for c in chains {
        let mut first: Option<u32> = None;
        for (m, &p) in c.stack.iter().enumerate() {
            if !ties(c, m) {
                continue;
            }
            match first {
                None => first = Some(p),
                Some(f) => {
                    let (a, b) = (find(&mut parent, f), find(&mut parent, p));
                    if a != b {
                        parent[b as usize] = a;
                    }
                }
            }
        }
    }
    let mut by_root: FxHashMap<u32, usize> = FxHashMap::default();
    let mut out: Vec<Context> = Vec::new();
    for (i, c) in chains.iter().enumerate() {
        // a chain where nobody ties (all origins) forms its own context from its first member
        let anchor = (0..c.stack.len())
            .find(|&m| ties(c, m))
            .map_or(c.stack[0], |m| c.stack[m]);
        let root = find(&mut parent, anchor);
        let k = *by_root.entry(root).or_insert_with(|| {
            out.push(Context {
                chains: Vec::new(),
                participants: Vec::new(),
                targets: 0,
                unresolved: 0,
                severity: Severity::Info,
            });
            out.len() - 1
        });
        let cx = &mut out[k];
        cx.chains.push(i as u32);
        cx.targets += c.targets.len() as u32;
        if c.unresolved {
            cx.unresolved += c.targets.len() as u32;
        }
        cx.severity = cx.severity.max(c.severity);
        cx.participants.extend(c.stack.iter().copied());
    }
    for cx in &mut out {
        cx.participants.sort_unstable();
        cx.participants.dedup();
        cx.participants
            .sort_by_key(|&p| (participants[p as usize].rank, p));
    }
    out.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then(b.unresolved.cmp(&a.unresolved))
            .then(b.targets.cmp(&a.targets))
    });
    out
}

// @sat:conflict.owners
/// Per owner (lower case, as `owner_of` names it): targets it takes part in across `reports` (a cell that is not a
/// target's own source or "not there"), and those wanting a decision.
pub fn owner_cases(
    reports: &[&DomainReport],
    owner_of: impl Fn(&Participant) -> Option<String>,
) -> FxHashMap<String, (u32, u32)> {
    let mut out: FxHashMap<String, (u32, u32)> = FxHashMap::default();
    for r in reports {
        let owners: Vec<Option<String>> = r
            .participants
            .iter()
            .map(|p| owner_of(p).map(|o| o.to_lowercase()))
            .collect();
        for c in &r.chains {
            for (m, &p) in c.stack.iter().enumerate() {
                let Some(o) = &owners[p as usize] else { continue };
                let n = c
                    .row(m)
                    .iter()
                    .filter(|&&x| x != cell::ORIGIN && x != cell::NONE)
                    .count() as u32;
                if n == 0 {
                    continue;
                }
                let e = out.entry(o.clone()).or_default();
                e.0 += n;
                if c.unresolved {
                    e.1 += n;
                }
            }
        }
    }
    out
}

/// Cells of a winner semantics from ranks (stack order, lowest first): the top rank wins, equal tops tie, the rest
/// lose. Returns the cells and whether the top is a tie.
pub fn ranked_cells(ranks: &[f64], higher_wins: bool) -> (Vec<u8>, bool) {
    let top = if higher_wins {
        ranks.iter().copied().fold(f64::NEG_INFINITY, f64::max)
    } else {
        ranks.iter().copied().fold(f64::INFINITY, f64::min)
    };
    let tops = ranks.iter().filter(|&&r| r == top).count();
    let cells = ranks
        .iter()
        .map(|&r| match (r == top, tops > 1) {
            (true, true) => cell::TIE,
            (true, false) => cell::WIN,
            _ => cell::LOSE,
        })
        .collect();
    (cells, tops > 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> DomainInfo {
        DomainInfo {
            id: "t".into(),
            title: "T".into(),
            about: String::new(),
            family: Family::Collision,
            centrality: Centrality::Field,
            semantics: Semantics::HighestWins,
            origin: Origin::Builtin,
        }
    }

    #[test]
    fn contests_fold_into_chains_and_contexts_by_shared_participants() {
        let mut b = Builder::new(info());
        let (a, x, y, z) = (
            b.participant("A", Some("mod A"), 1),
            b.participant("X", None, 2),
            b.participant("Y", None, 3),
            b.participant("Z", None, 4),
        );
        b.contest(
            "t1",
            &[(a, cell::LOSE), (x, cell::WIN)],
            "",
            Severity::Info,
            false,
        );
        b.contest(
            "t2",
            &[(a, cell::SAME), (x, cell::WIN)],
            "",
            Severity::Info,
            false,
        );
        b.contest(
            "t3",
            &[(x, cell::TIE), (y, cell::TIE)],
            "ничья",
            Severity::Warn,
            true,
        );
        // z's chain shares nobody but an origin
        b.contest(
            "t4",
            &[(a, cell::ORIGIN), (z, cell::WIN)],
            "",
            Severity::Info,
            false,
        );
        let r = b.finish();
        assert_eq!(r.chains.len(), 3);
        assert_eq!(r.chains[0].kind, "ничья", "warn and unresolved first");
        let ax = r.chains.iter().find(|c| c.stack == [a, x]).unwrap();
        assert_eq!(ax.row(0), b"ls");
        assert_eq!(ax.row(1), b"ww");
        assert_eq!(r.counts.targets, 4);
        assert_eq!(r.counts.unresolved, 1);
        assert_eq!(
            r.contexts.len(),
            2,
            "A–X–Y tie together; Z only meets A as an origin"
        );
        assert_eq!(r.contexts[0].participants, [a, x, y]);
        assert_eq!(r.chain_name(&r.chains[0]), "ничья · X -> Y");
    }

    #[test]
    fn ranks_give_a_winner_or_a_tie() {
        assert_eq!(ranked_cells(&[1.0, 3.0, 2.0], true), (b"lwl".to_vec(), false));
        assert_eq!(ranked_cells(&[3.0, 3.0, 2.0], true), (b"ttl".to_vec(), true));
        assert_eq!(ranked_cells(&[1.0, 3.0], false), (b"wl".to_vec(), false));
    }
}
