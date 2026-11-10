//! Identical overrides: a losing file whose content is the winner's — the conflict changes nothing in the game,
//! the losing copy is only disk. @sat:identical
//!
//! Owns: which files to compare (`candidates`: loose providers of a contested path with the winner's size) and
//! what equal contents add up to (`tally`: per mod, files and bytes the game never gets anything from).
//! Not here: reading and hashing files (`@sat:scan.content-hash`), the overwrite projection (`@sat:overwrite.projection`).
//!
//! Only loose files of active mods take part: comparing with archive entries needs unpacking, and a disabled mod's
//! copy is not in the game anyway. The size goes first — files of other sizes are never read.

use crate::ids::{ModId, PathId};
use crate::overwrite::Priorities;
use crate::table::ProviderTable;

/// A contested path to compare: the winner's provider and the losers' providers of the same size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathId,
    pub winner: usize,
    pub losers: Vec<usize>,
}

// @sat:identical.candidates
/// Paths whose winner is a loose file and that have loose losers of the same size, among active mods. The winner is
/// the active loose provider of the highest priority (by mod, not by logical group: a group's members keep their
/// own files on disk).
pub fn candidates(table: &ProviderTable, prio: &Priorities, min_size: u64) -> Vec<Candidate> {
    let mut out = Vec::new();
    for p in 0..table.path_count() {
        let p = PathId::from_idx(p);
        let range = table.path_providers(p);
        if range.len() < 2 {
            continue;
        }
        let loose = || {
            range
                .clone()
                .filter(|&i| !table.is_archive(i) && prio.is_active(table.owner(i)))
        };
        let Some(winner) = loose().max_by_key(|&i| prio.get(table.owner(i))) else {
            continue;
        };
        let size = table.size(winner);
        if size < min_size {
            continue;
        }
        let w = table.owner(winner);
        let losers: Vec<usize> = loose()
            .filter(|&i| i != winner && table.owner(i) != w && table.size(i) == size)
            .collect();
        if !losers.is_empty() {
            out.push(Candidate {
                path: p,
                winner,
                losers,
            });
        }
    }
    out
}

/// Files and bytes of a mod that are identical copies of what wins over them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Redundant {
    pub files: u64,
    pub bytes: u64,
}

/// What comparing the candidates found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tally {
    /// By mod (index = `ModId`).
    pub by_mod: Vec<Redundant>,
    /// Contested paths where every compared loser equals the winner.
    pub harmless_paths: u64,
    pub compared: u64,
    /// Files that could not be read.
    pub unread: u64,
}

impl Tally {
    pub fn total(&self) -> Redundant {
        self.by_mod.iter().fold(Redundant::default(), |a, r| Redundant {
            files: a.files + r.files,
            bytes: a.bytes + r.bytes,
        })
    }

    /// Mods with redundant copies, most bytes first.
    pub fn top(&self) -> Vec<(ModId, Redundant)> {
        let mut v: Vec<(ModId, Redundant)> = self
            .by_mod
            .iter()
            .enumerate()
            .filter(|(_, r)| r.files > 0)
            .map(|(m, r)| (ModId::from_idx(m), *r))
            .collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.1.bytes));
        v
    }
}

// @sat:identical.tally
/// Adds up equal contents; `hash(i)` — the content hash of provider `i` (`None`: could not be read).
pub fn tally(table: &ProviderTable, cands: &[Candidate], hash: impl Fn(usize) -> Option<u64>) -> Tally {
    let mut t = Tally {
        by_mod: vec![Redundant::default(); table.mod_count()],
        ..Tally::default()
    };
    for c in cands {
        let Some(w) = hash(c.winner) else {
            t.unread += 1;
            continue;
        };
        let mut all = true;
        for &l in &c.losers {
            t.compared += 1;
            match hash(l) {
                Some(h) if h == w => {
                    let r = &mut t.by_mod[table.owner(l).idx()];
                    r.files += 1;
                    r.bytes += table.size(l);
                }
                Some(_) => all = false,
                None => {
                    t.unread += 1;
                    all = false;
                }
            }
        }
        if all && table.path_providers(c.path).len() == c.losers.len() + 1 {
            t.harmless_paths += 1;
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{LooseDir, LooseFile, ModFacts};

    fn mod_with(name: &str, files: &[(&str, u64)]) -> ModFacts {
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
    fn equal_losers_are_redundant_others_are_not() {
        // A < B < C by priority; D is off
        let facts = [
            mod_with("A", &[("same.dds", 100), ("diff.dds", 100), ("small.dds", 5)]),
            mod_with("B", &[("same.dds", 100), ("other.dds", 7)]),
            mod_with(
                "C",
                &[
                    ("same.dds", 100),
                    ("diff.dds", 100),
                    ("other.dds", 9),
                    ("small.dds", 5),
                ],
            ),
            mod_with("D", &[("same.dds", 100)]),
        ];
        let table = ProviderTable::build(&facts);
        let cands = candidates(&table, &Priorities(vec![0, 1, 2, -1]), 10);
        // same.dds (A, B lose to C; D is off), diff.dds (A loses to C); other.dds sizes differ; small.dds too small
        assert_eq!(cands.len(), 2);
        assert_eq!(cands.iter().map(|c| c.losers.len()).sum::<usize>(), 3);
        let name = |i: usize| table.mod_name(table.owner(i)).to_string();
        let path = |i: usize| table.path_string(table.path_of_provider(i));
        // contents: every same.dds equal; diff.dds differs
        let t = tally(&table, &cands, |i| {
            Some(if path(i).ends_with("diff.dds") && name(i) == "A" {
                2
            } else {
                1
            })
        });
        assert_eq!(t.by_mod[0], Redundant { files: 1, bytes: 100 });
        assert_eq!(t.by_mod[1], Redundant { files: 1, bytes: 100 });
        assert_eq!(t.total(), Redundant { files: 2, bytes: 200 });
        assert_eq!(t.top()[0].1.bytes, 100);
        // same.dds also has D (off) among its providers: not counted as fully harmless
        assert_eq!((t.harmless_paths, t.compared, t.unread), (0, 3, 0));
        // an unreadable winner skips the path
        let t = tally(&table, &cands, |i| (name(i) != "C").then_some(1));
        assert_eq!((t.total().files, t.unread), (0, 2));
    }
}
