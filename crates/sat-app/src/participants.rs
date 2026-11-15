//! Participants of conflict chains as the monolith shows them: idle owners, owner stacks, W / L / M / X. @sat:app.participants
//!
//! Owns: splitting an exact chain into **owner stacks** — its paths grouped by who else provides them while
//! disabled (idle owners, `@sat:overwrite.owners`) — the cell of a mod in such a stack, and a member's counts
//! relative to a normalized chain: owned, wins, loses, missing, extra (`08-conflicts.md` §3.4, §4.3, §4.8).
//! Not here: exact and normalized chains (`@sat:overwrite`, `@sat:routes`), the table and cards that show them
//! (`@sat:app.context-table`, `@sat:app.cards`).
//!
//! The monolith's route holds every owner of a path, disabled ones too, so a disabled mod is a participant: it owns
//! the path, wins nothing, loses nothing (tooth `u`). NG keeps the conflict and its chain on the active stack (a
//! switched mod does not rename chains) and splits a chain's stack by idle owners only for showing: every owner
//! stack goes through the chain the same way, idle owners included, so a column and a tooth stay honest.
//!
//! Cells: `w` wins, `l` loses, `u` owns it but is disabled (no win, no loss), `-` does not have it. In a context
//! table the window downgrades cells outside the selected chain to "linked" (monolith v672); that is drawing, not
//! data.

use rustc_hash::FxHashMap;

use sat_core::ids::{ChainId, ModId, PathId};
use sat_core::overwrite::OverwriteProjection;
use sat_core::table::ProviderTable;

/// Paths of one exact chain that share their idle owners.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnerStack {
    pub chain: ChainId,
    /// Disabled owners, table order.
    pub idle: Vec<ModId>,
    pub paths: Vec<PathId>,
}

impl OwnerStack {
    pub fn files(&self) -> u32 {
        self.paths.len() as u32
    }

    /// The cell of mod `m`.
    pub fn cell(&self, ow: &OverwriteProjection, m: ModId) -> char {
        match ow.chain_mods(self.chain).iter().position(|&x| x == m) {
            Some(0) => 'w',
            Some(_) => 'l',
            None if self.idle.contains(&m) => 'u',
            None => '-',
        }
    }

    /// Everyone in the stack: the active stack (winner first), then idle owners.
    pub fn members<'a>(&'a self, ow: &'a OverwriteProjection) -> impl Iterator<Item = ModId> + 'a {
        ow.chain_mods(self.chain)
            .iter()
            .copied()
            .chain(self.idle.iter().copied())
    }

    /// Paths as strings, sorted.
    pub fn path_strings(&self, table: &ProviderTable) -> Vec<String> {
        let mut v: Vec<String> = self.paths.iter().map(|&p| table.path_string(p)).collect();
        v.sort_unstable();
        v
    }
}

/// Splits exact chain `chain` with paths `paths` by idle owners.
pub(crate) fn split(
    ow: &OverwriteProjection,
    table: &ProviderTable,
    chain: ChainId,
    paths: &[PathId],
) -> Vec<OwnerStack> {
    let mut by: FxHashMap<Vec<ModId>, Vec<PathId>> = Default::default();
    for &p in paths {
        by.entry(ow.idle_owners(table, p)).or_default().push(p);
    }
    by.into_iter()
        .map(|(idle, mut paths)| {
            paths.sort_unstable();
            OwnerStack { chain, idle, paths }
        })
        .collect()
}

/// The owner stacks of a normalized chain, larger first: its columns and its teeth. `paths_of` may hold the paths of
/// its exact chains (a context computes them in one pass); a missing entry is computed.
pub(crate) fn head_stacks(
    ow: &OverwriteProjection,
    table: &ProviderTable,
    head: &crate::chains::Head,
    paths_of: Option<&FxHashMap<ChainId, Vec<PathId>>>,
) -> Vec<OwnerStack> {
    let mut v: Vec<OwnerStack> = Vec::new();
    for &c in &head.chains {
        match paths_of.and_then(|m| m.get(&c)) {
            Some(ps) => v.extend(split(ow, table, c, ps)),
            None => v.extend(split(ow, table, c, &ow.chain_paths(c))),
        }
    }
    v.sort_by(|a, b| {
        b.paths
            .len()
            .cmp(&a.paths.len())
            .then(a.chain.cmp(&b.chain))
            .then(a.idle.cmp(&b.idle))
    });
    v
}

/// A member's counts relative to a normalized chain (the monolith's chain participant).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Tally {
    /// Paths of the chain it provides (active or not).
    pub owned: u32,
    pub wins: u32,
    pub loses: u32,
    /// Paths of the chain it does not provide.
    pub missing: u32,
    /// Its contested paths outside the chain.
    pub extra: u32,
}

/// Counts of member `m` over `stacks` of a chain of `total` paths.
pub(crate) fn tally(ow: &OverwriteProjection, stacks: &[OwnerStack], m: ModId, total: u32) -> Tally {
    let mut t = Tally::default();
    for s in stacks {
        let n = s.files();
        match s.cell(ow, m) {
            'w' => {
                t.wins += n;
                t.owned += n;
            }
            'l' => {
                t.loses += n;
                t.owned += n;
            }
            'u' => t.owned += n,
            _ => {}
        }
    }
    t.missing = total.saturating_sub(t.owned);
    t.extra = (ow.contested_of(m) as u32).saturating_sub(t.owned);
    t
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sat_core::canon::{Entry, EntryKind, Modlist};
    use sat_core::facts::{LooseDir, LooseFile, ModFacts};
    use sat_core::order::OrderBook;
    use sat_core::session::Session;
    use sat_core::table::ProviderTable;

    pub(crate) fn session(mods: &[(&str, bool, &[&str])]) -> Session {
        let facts: Vec<ModFacts> = mods
            .iter()
            .map(|(n, _, files)| ModFacts {
                name: n.to_string(),
                dirs: vec![LooseDir {
                    rel: "meshes".into(),
                    files: files
                        .iter()
                        .map(|f| LooseFile {
                            name: f.to_string(),
                            size: 1,
                            vram: 0,
                        })
                        .collect(),
                }],
                ..Default::default()
            })
            .collect();
        let entries = mods
            .iter()
            .map(|(n, on, _)| Entry {
                name: n.to_string(),
                kind: EntryKind::Mod,
                enabled: *on,
            })
            .collect();
        Session::new(
            OrderBook::from_modlist(&Modlist::new(entries)),
            Arc::new(ProviderTable::build(&facts)),
        )
    }

    #[test]
    fn a_disabled_owner_splits_the_chain_and_owns_without_winning() {
        // x, y: A < C (one exact chain); B is disabled and has x only
        let mut s = session(&[
            ("A", true, &["x.nif", "y.nif"]),
            ("B", false, &["x.nif"]),
            ("C", true, &["x.nif", "y.nif"]),
        ]);
        let ow = s.overwrite();
        let table = s.table();
        let id =
            |n: &str| sat_core::ids::ModId::from_idx(table.mod_names().iter().position(|x| x == n).unwrap());
        assert_eq!(ow.chain_count(), 1, "the chain stays on the active stack");
        let c = ow.chain_ids()[0];
        let st = super::split(&ow, table, c, &ow.chain_paths(c));
        assert_eq!(st.len(), 2, "x has an idle owner, y has not");
        let x = st.iter().find(|s| !s.idle.is_empty()).unwrap();
        assert_eq!(x.idle, [id("B")]);
        assert_eq!(
            (x.cell(&ow, id("C")), x.cell(&ow, id("A")), x.cell(&ow, id("B"))),
            ('w', 'l', 'u')
        );
        let t = super::tally(&ow, &st, id("B"), 2);
        assert_eq!((t.owned, t.wins, t.loses, t.missing, t.extra), (1, 0, 0, 1, 0));
        let t = super::tally(&ow, &st, id("C"), 2);
        assert_eq!((t.owned, t.wins, t.loses, t.missing), (2, 2, 0, 0));
    }
}
