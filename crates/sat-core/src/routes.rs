//! Normalized chains: exact chains merged into route groups (monolith normalized layer v499–v501). @sat:routes
//!
//! Owns: grouping the live exact chains of an [`OverwriteProjection`] into normalized chains, the super-route of
//! each group, and chain → group / mod → groups lookups.
//! Not here: exact chains and stacks (`@sat:overwrite.projection`), names and colours (`@sat:identity`), decisions
//! (`@sat:decisions` — a normalized chain is settled when every exact chain in it is).
//!
//! An exact chain is one stack, so a build has many near-duplicates: `C → B → A` on some textures and `C → A` on
//! others. The monolith shows them as one normalized chain whose route `C → B → A` keeps every source route as a
//! subsequence. Its rules (`08-conflicts.md` §4.7), kept here:
//! - one winner per normalized chain (no winner mixing);
//! - the union of the source routes' edges is acyclic, and every source route is a subsequence of the super-route;
//! - at most [`NODE_CAP`] mods and [`CHAIN_CAP`] source chains per group;
//! - the exact layer is not changed: every exact chain is in exactly one group, files and stacks are kept.
//!
//! The monolith minimises repeated mod appearances with a beam search; under one priority scale every stack is a
//! subsequence of the list order, so all chains of one winner merge safely and the greedy pass below already
//! reaches that minimum. The acyclicity check matters when loose files beat archives against priority.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::ids::{ChainId, ModId};
use crate::overwrite::OverwriteProjection;

/// Most mods in one normalized chain (monolith v499 `route-node-cap`).
pub const NODE_CAP: usize = 256;
/// Most exact chains in one normalized chain (monolith v499 `route-variant-cap`).
pub const CHAIN_CAP: usize = 128;

/// One normalized chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteGroup {
    /// Super-route, winner first (the same direction as an exact stack).
    pub route: Vec<ModId>,
    /// Source exact chains, larger first.
    pub chains: Vec<ChainId>,
    pub files: u32,
    pub bytes: u64,
}

// @sat:routes.groups
/// Normalized chains of one projection.
#[derive(Debug, Clone, Default)]
pub struct RouteGroups {
    groups: Vec<RouteGroup>,
    of_chain: FxHashMap<ChainId, u32>,
}

struct Draft {
    nodes: FxHashSet<ModId>,
    edges: FxHashSet<(ModId, ModId)>,
    chains: Vec<ChainId>,
    /// Every edge goes from higher to lower priority: such a graph cannot have a cycle, so a route that is ordered
    /// too joins without a graph walk (the common case: only loose-over-archive breaks the order).
    ordered: bool,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            nodes: FxHashSet::default(),
            edges: FxHashSet::default(),
            chains: Vec::new(),
            ordered: true,
        }
    }
}

/// Whether the edges with `add` stay acyclic (Kahn).
fn acyclic(nodes: &FxHashSet<ModId>, edges: &FxHashSet<(ModId, ModId)>, add: &[(ModId, ModId)]) -> bool {
    let mut indeg: FxHashMap<ModId, u32> = nodes.iter().map(|&n| (n, 0)).collect();
    let mut out: FxHashMap<ModId, Vec<ModId>> = FxHashMap::default();
    let mut seen: FxHashSet<(ModId, ModId)> = FxHashSet::default();
    for &(a, b) in edges.iter().chain(add) {
        if seen.insert((a, b)) {
            indeg.entry(a).or_insert(0);
            *indeg.entry(b).or_insert(0) += 1;
            out.entry(a).or_default().push(b);
        }
    }
    let mut ready: Vec<ModId> = indeg.iter().filter(|(_, d)| **d == 0).map(|(&n, _)| n).collect();
    let mut done = 0;
    while let Some(n) = ready.pop() {
        done += 1;
        for &m in out.get(&n).map_or(&[][..], |v| v) {
            let d = indeg.get_mut(&m).expect("node");
            *d -= 1;
            if *d == 0 {
                ready.push(m);
            }
        }
    }
    done == indeg.len()
}

/// Topological order of a group, ties by priority (higher first): the super-route, winner first.
fn super_route(d: &Draft, prio: impl Fn(ModId) -> i32) -> Vec<ModId> {
    let mut indeg: FxHashMap<ModId, u32> = d.nodes.iter().map(|&n| (n, 0)).collect();
    let mut out: FxHashMap<ModId, Vec<ModId>> = FxHashMap::default();
    for &(a, b) in &d.edges {
        *indeg.get_mut(&b).expect("node") += 1;
        out.entry(a).or_default().push(b);
    }
    let key = |m: ModId| (prio(m), std::cmp::Reverse(m.0));
    let mut ready: Vec<ModId> = indeg.iter().filter(|(_, d)| **d == 0).map(|(&n, _)| n).collect();
    let mut route = Vec::with_capacity(d.nodes.len());
    while !ready.is_empty() {
        // highest priority last, so pop takes it
        ready.sort_unstable_by_key(|&m| key(m));
        let n = ready.pop().expect("non-empty");
        route.push(n);
        for &m in out.get(&n).map_or(&[][..], |v| v) {
            let d = indeg.get_mut(&m).expect("node");
            *d -= 1;
            if *d == 0 {
                ready.push(m);
            }
        }
    }
    route
}

impl RouteGroups {
    pub fn build(ow: &OverwriteProjection) -> Self {
        let mut by_winner: FxHashMap<ModId, Vec<ChainId>> = FxHashMap::default();
        let mut winners: Vec<ModId> = Vec::new();
        for c in ow.chain_ids() {
            let w = ow.chain_mods(c)[0];
            by_winner
                .entry(w)
                .or_insert_with(|| {
                    winners.push(w);
                    Vec::new()
                })
                .push(c);
        }
        let mut groups = Vec::new();
        let mut of_chain = FxHashMap::default();
        for w in winners {
            let mut chains = by_winner.remove(&w).expect("bucket");
            chains.sort_by_key(|&c| {
                (
                    std::cmp::Reverse((ow.chain_mods(c).len(), ow.chain(c).files)),
                    c.0,
                )
            });
            let mut drafts: Vec<Draft> = Vec::new();
            for c in chains {
                let stack = ow.chain_mods(c);
                let edges: Vec<(ModId, ModId)> = stack.windows(2).map(|p| (p[0], p[1])).collect();
                let prio = ow.priorities();
                let ordered = edges.iter().all(|&(a, b)| prio.get(a) > prio.get(b));
                let fits = |d: &Draft| {
                    d.chains.len() < CHAIN_CAP
                        && d.nodes.len() + stack.iter().filter(|m| !d.nodes.contains(m)).count() <= NODE_CAP
                        && ((d.ordered && ordered) || acyclic(&d.nodes, &d.edges, &edges))
                };
                let i = match drafts.iter().position(fits) {
                    Some(i) => i,
                    None => {
                        drafts.push(Draft::default());
                        drafts.len() - 1
                    }
                };
                let d = &mut drafts[i];
                d.nodes.extend(stack.iter().copied());
                d.edges.extend(edges);
                d.ordered &= ordered;
                d.chains.push(c);
            }
            for d in drafts {
                let id = groups.len() as u32;
                for &c in &d.chains {
                    of_chain.insert(c, id);
                }
                groups.push(RouteGroup {
                    route: super_route(&d, |m| ow.priorities().get(m)),
                    files: d.chains.iter().map(|&c| ow.chain(c).files).sum(),
                    bytes: d.chains.iter().map(|&c| ow.chain(c).bytes).sum(),
                    chains: d.chains,
                });
            }
        }
        Self { groups, of_chain }
    }

    pub fn len(&self) -> usize {
        self.groups.len()
    }
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
    pub fn groups(&self) -> &[RouteGroup] {
        &self.groups
    }
    pub fn group(&self, g: u32) -> &RouteGroup {
        &self.groups[g as usize]
    }
    /// The normalized chain an exact chain belongs to.
    pub fn of_chain(&self, c: ChainId) -> Option<u32> {
        self.of_chain.get(&c).copied()
    }
    // @sat:routes.of-mod
    /// Normalized chains a mod takes part in (through its exact chains), larger first.
    pub fn of_mod(&self, ow: &OverwriteProjection, m: ModId) -> Vec<u32> {
        let mut gs: Vec<u32> = ow
            .mod_chains(m)
            .into_iter()
            .filter_map(|c| self.of_chain(c))
            .collect();
        gs.sort_unstable();
        gs.dedup();
        gs.sort_by_key(|&g| (std::cmp::Reverse(self.groups[g as usize].files), g));
        gs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overwrite::Priorities;
    use crate::table::ProviderTable;
    use crate::table::tests::{mk, with_archive};

    fn names(t: &ProviderTable, ms: &[ModId]) -> Vec<String> {
        ms.iter().map(|&m| t.mod_name(m).to_string()).collect()
    }

    #[test]
    fn chains_of_one_winner_merge_into_one_route() {
        // order A < B < C < D: x → C,B,A; y → C,A; z → C,B; w → D,A (another winner)
        let t = ProviderTable::build(&[
            mk("A", &["x.dds", "y.dds", "w.dds"]),
            mk("B", &["x.dds", "z.dds"]),
            mk("C", &["x.dds", "y.dds", "z.dds"]),
            mk("D", &["w.dds"]),
        ]);
        let ow = OverwriteProjection::compute(&t, Priorities((0..4).collect()));
        assert_eq!(ow.chain_count(), 4);
        let r = RouteGroups::build(&ow);
        assert_eq!(r.len(), 2, "one per winner");
        let c = r.groups().iter().find(|g| g.chains.len() == 3).unwrap();
        assert_eq!(names(&t, &c.route), ["C", "B", "A"]);
        assert_eq!(c.files, 3);
        // every exact chain is in exactly one group and is a subsequence of its route
        for id in ow.chain_ids() {
            let g = r.group(r.of_chain(id).unwrap());
            let mut it = g.route.iter();
            assert!(
                ow.chain_mods(id).iter().all(|m| it.any(|x| x == m)),
                "source route kept"
            );
        }
        let a = t
            .mod_names()
            .iter()
            .position(|n| n == "A")
            .map(ModId::from_idx)
            .unwrap();
        assert_eq!(r.of_mod(&ow, a).len(), 2);
    }

    #[test]
    fn a_cycle_from_loose_over_archive_splits_the_group() {
        // B is above A by priority, but A's loose file beats B's archive copy on y:
        // x → C,B,A (all loose); y → C,A,B (A loose, B archive) → B→A and A→B cannot share a route
        let t = ProviderTable::build(&[
            mk("A", &["x.dds", "y.dds"]),
            with_archive(mk("B", &["x.dds"]), "B.bsa", &["y.dds"]),
            mk("C", &["x.dds", "y.dds"]),
        ]);
        let ow = OverwriteProjection::compute(&t, Priorities((0..3).collect()));
        assert_eq!(ow.chain_count(), 2);
        let r = RouteGroups::build(&ow);
        assert_eq!(r.len(), 2, "same winner, but the routes disagree on A and B");
    }
}
