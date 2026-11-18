//! A move in the mod order that changes no file's winner. @sat:order.safe-move
//!
//! Owns: which paths would get another winner when some mods take another place in the list — the old and the new
//! priorities compared on the moved mods' loose copies only. The mod order decides among loose copies; archive
//! contents go by the plugin order (`@sat:conflict.game-copy`), which a mod move does not touch (the archive file
//! itself is a loose file and is checked like any other). A new winner with the same bytes as the old one changes
//! nothing the game sees and is not counted.
//!
//! Not here: where to move (the CLI reads a list, `@sat:cli.safe-moves`), applying the move (`@sat:order.book`).

use crate::ids::{ModId, PathId};
use crate::overwrite::Priorities;
use crate::table::ProviderTable;

// @sat:order.safe-move.check
/// The paths of the `moved` mods whose winner differs between `old` and `new` (and whose bytes differ, or are not
/// known to be the same — `same(a, b)`), up to `limit` of them; empty — the move is safe.
pub fn changed_winners(
    table: &ProviderTable,
    old: &Priorities,
    new: &Priorities,
    moved: &[ModId],
    same: impl Fn(usize, usize) -> Option<bool>,
    limit: usize,
) -> Vec<PathId> {
    let winner = |prio: &Priorities, p: PathId| -> Option<usize> {
        table
            .path_providers(p)
            .filter(|&j| !table.is_archive(j) && prio.is_active(table.owner(j)))
            .max_by_key(|&j| prio.get(table.owner(j)))
    };
    let mut out = Vec::new();
    let mut seen = rustc_hash::FxHashSet::default();
    for &m in moved {
        for &i in table.mod_providers(m) {
            let i = i as usize;
            if table.is_archive(i) {
                continue;
            }
            let p = table.path_of_provider(i);
            if !seen.insert(p) {
                continue;
            }
            let (a, b) = (winner(old, p), winner(new, p));
            let differs = match (a, b) {
                (Some(a), Some(b)) => a != b && same(a, b) != Some(true),
                (None, None) => false,
                _ => true,
            };
            if differs {
                out.push(p);
                if out.len() >= limit {
                    return out;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{LooseDir, LooseFile, ModFacts};

    fn mk(name: &str, files: &[&str]) -> ModFacts {
        ModFacts {
            name: name.into(),
            dirs: vec![LooseDir {
                rel: "textures".into(),
                files: files
                    .iter()
                    .map(|n| LooseFile {
                        name: n.to_string(),
                        size: 1,
                        vram: 0,
                    })
                    .collect(),
            }],
            ..ModFacts::default()
        }
    }

    #[test]
    fn a_mod_may_jump_over_mods_it_shares_nothing_with_but_not_over_its_rival() {
        let facts = [mk("A", &["x.dds"]), mk("B", &["y.dds"]), mk("C", &["x.dds"])];
        let table = ProviderTable::build(&facts);
        let a = ModId::from_idx(0);
        // A over B only: A shares nothing with B
        let old = Priorities(vec![1, 2, 3]);
        let over_b = Priorities(vec![2, 1, 3]);
        assert!(changed_winners(&table, &old, &over_b, &[a], |_, _| None, 10).is_empty());
        // A over C: C's x.dds stops winning
        let over_c = Priorities(vec![4, 2, 3]);
        assert_eq!(
            changed_winners(&table, &old, &over_c, &[a], |_, _| None, 10).len(),
            1
        );
        // ... unless the two copies are the same bytes
        assert!(changed_winners(&table, &old, &over_c, &[a], |_, _| Some(true), 10).is_empty());
    }
}
