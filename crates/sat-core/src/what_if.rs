//! "What if it were on": the theoretical part a disabled mod would play at its place in the list. @sat:what-if
//!
//! Owns: for each disabled mod, against the build as it is (the enabled mods only, each disabled mod alone) — how
//! many enabled mods it would meet on a path, how many paths it would take from the current winner (and how many
//! bytes), how many it would lose, how many of its copies are the same bytes as the winner's (no change to see), and
//! the mods it would take the most paths from. The game's rule decides a copy: a loose file beats every archive copy,
//! then the place in the list.
//!
//! Not in any real metric: the build's conflicts, contexts and reports count enabled mods only. Not here: whether
//! its plugins would load (`@sat:plugins.master-gap`), whether it is an older version of an enabled mod (the CLI
//! looks at Nexus ids).

use rustc_hash::FxHashMap;

use crate::ids::ModId;
use crate::overwrite::Priorities;
use crate::table::ProviderTable;

/// What a disabled mod would do if it were the only one turned on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhatIf {
    pub m: ModId,
    /// Its paths (one per path, loose or in its archives) and their bytes.
    pub files: u32,
    pub bytes: u64,
    /// Paths it shares with enabled mods.
    pub shared: u32,
    /// Enabled mods it meets on those paths.
    pub met: u32,
    /// Paths where its copy would become the one the game takes (and differs from the current one, when known).
    pub would_win: u32,
    pub would_win_bytes: u64,
    /// Paths where an enabled mod's copy would still be taken.
    pub would_lose: u32,
    /// Paths where its copy is the same bytes as the current winner's: it would change nothing there.
    pub same: u32,
    /// The enabled mods it would take the most paths from: (mod, paths), most first.
    pub victims: Vec<(ModId, u32)>,
}

// @sat:what-if.compute
/// The theoretical part of every disabled mod (a mod that meets no enabled one has a row too). `rank` is each mod's place in the list as if every mod were enabled
/// (higher wins among loose copies); `prio` is the build as it is. `same(a, b)` says whether two copies hold the same
/// bytes (`None` — unknown).
pub fn what_if(
    table: &ProviderTable,
    prio: &Priorities,
    rank: &Priorities,
    same: impl Fn(usize, usize) -> Option<bool>,
) -> Vec<WhatIf> {
    let mut out = Vec::new();
    for mi in 0..table.mod_count() {
        let m = ModId::from_idx(mi);
        if prio.is_active(m) || !rank.is_active(m) {
            continue;
        }
        let my_rank = rank.get(m);
        let mut w = WhatIf {
            m,
            files: 0,
            bytes: 0,
            shared: 0,
            met: 0,
            would_win: 0,
            would_win_bytes: 0,
            would_lose: 0,
            same: 0,
            victims: Vec::new(),
        };
        let mut met: FxHashMap<u32, u32> = FxHashMap::default();
        let mut victims: FxHashMap<u32, u32> = FxHashMap::default();
        let mut last_path = None;
        for &i in table.mod_providers(m) {
            let i = i as usize;
            let p = table.path_of_provider(i);
            // one copy per path (a mod with the file loose and in an archive counts the loose one)
            if last_path == Some(p) {
                continue;
            }
            last_path = Some(p);
            w.files += 1;
            w.bytes += table.size(i);
            let active: Vec<usize> = table
                .path_providers(p)
                .filter(|&j| prio.is_active(table.owner(j)))
                .collect();
            if active.is_empty() {
                continue;
            }
            w.shared += 1;
            for &j in &active {
                *met.entry(table.owner(j).0).or_default() += 1;
            }
            // the copy the game takes now: a loose one by place, else an archive one by place
            let key = |j: usize| (!table.is_archive(j), prio.get(table.owner(j)));
            let Some(&winner) = active.iter().max_by_key(|&&j| key(j)) else {
                continue;
            };
            // my best copy of this path (loose before archive)
            let mine = table
                .path_providers(p)
                .filter(|&j| table.owner(j) == m)
                .max_by_key(|&j| !table.is_archive(j))
                .unwrap_or(i);
            let wins = match (table.is_archive(mine), table.is_archive(winner)) {
                (false, true) => true,
                (true, false) => false,
                _ => my_rank > rank.get(table.owner(winner)),
            };
            if same(mine, winner) == Some(true) {
                w.same += 1;
            } else if wins {
                w.would_win += 1;
                w.would_win_bytes += table.size(mine);
                *victims.entry(table.owner(winner).0).or_default() += 1;
            } else {
                w.would_lose += 1;
            }
        }
        w.met = met.len() as u32;
        let mut v: Vec<(ModId, u32)> = victims.into_iter().map(|(k, n)| (ModId(k), n)).collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.1));
        v.truncate(5);
        w.victims = v;
        out.push(w);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{LooseDir, LooseFile, ModFacts};

    fn mk(name: &str, files: &[(&str, u64)]) -> ModFacts {
        ModFacts {
            name: name.into(),
            dirs: vec![LooseDir {
                rel: "textures".into(),
                files: files
                    .iter()
                    .map(|(n, s)| LooseFile {
                        name: n.to_string(),
                        size: *s,
                        vram: 0,
                    })
                    .collect(),
            }],
            ..ModFacts::default()
        }
    }

    #[test]
    fn a_disabled_mod_would_take_what_it_outranks_and_change_nothing_where_it_is_the_same() {
        // A (on, rank 1) and C (on, rank 3); B (off, rank 2) and D (off, rank 4)
        let facts = [
            mk("A", &[("x.dds", 1), ("y.dds", 1)]),
            mk("B", &[("x.dds", 2), ("y.dds", 2)]),
            mk("C", &[("y.dds", 3)]),
            mk("D", &[("x.dds", 9), ("z.dds", 1)]),
        ];
        let table = ProviderTable::build(&facts);
        let prio = Priorities(vec![1, -1, 3, -1]);
        let rank = Priorities(vec![1, 2, 3, 4]);
        // D's x.dds is the same bytes as A's
        let same = |a: usize, b: usize| {
            let names = [table.mod_name(table.owner(a)), table.mod_name(table.owner(b))];
            Some(names == ["D", "A"])
        };
        let r = what_if(&table, &prio, &rank, same);
        let b = r.iter().find(|w| table.mod_name(w.m) == "B").unwrap();
        // B outranks A on x.dds, but not C on y.dds
        assert_eq!(
            (b.shared, b.met, b.would_win, b.would_lose, b.same),
            (2, 2, 1, 1, 0)
        );
        assert_eq!(b.victims, vec![(ModId::from_idx(0), 1)]);
        let d = r.iter().find(|w| table.mod_name(w.m) == "D").unwrap();
        // D meets A on x.dds with the same bytes; z.dds is its own
        assert_eq!(
            (d.files, d.bytes, d.shared, d.would_win, d.same),
            (2, 10, 1, 0, 1)
        );
    }
}
