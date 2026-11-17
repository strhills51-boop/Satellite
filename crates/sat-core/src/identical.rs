//! Content of contested copies: is a losing copy the winner's bytes, a different file, or not known. @sat:identical
//!
//! Owns: content hashes aligned with the provider table (the monolith's per-path route metadata, v554: origin, byte
//! length, content hash), equality of two copies of a path, the content verdict of an exact chain, what each mod
//! loses as copies equal to the winner, and which files are worth reading.
//! Not here: reading and hashing files (`@sat:scan.content-hash`), the chains themselves (`@sat:overwrite.projection`).
//!
//! A verdict is a property of a conflict, never a filter: every conflict stays a conflict and keeps being counted.
//! An identical copy still costs disk (and, in an archive, loading); a copy that was not read is "unknown", not
//! "different". Sizes speak first: loose files of different sizes differ without being read. Archive entries are
//! hashed by their unpacked bytes, so a packed copy compares with a loose one; a BA2 texture (no DDS header in the
//! archive) stays unknown.

use serde::{Deserialize, Serialize};

use crate::ids::{ChainId, ModId, PathId};
use crate::overwrite::{OverwriteProjection, Priorities};
use crate::table::ProviderTable;

/// Content hashes by provider index (`None` — not read).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentHashes(pub Vec<Option<u64>>);

impl ContentHashes {
    /// Hashes that belong to this table (same number of providers), else none.
    pub fn for_table(self, table: &ProviderTable) -> Self {
        if self.0.len() == table.provider_count() {
            self
        } else {
            Self::default()
        }
    }

    fn get(&self, i: usize) -> Option<u64> {
        self.0.get(i).copied().flatten()
    }
}

// @sat:identical.same
/// Whether two copies of a path hold the same bytes: `Some(false)` for loose files of different sizes or different
/// hashes, `Some(true)` for equal hashes (an archive entry's hash is of its unpacked bytes), `None` when it is not
/// known (a copy not read, an entry that cannot be unpacked for comparison).
pub fn same(table: &ProviderTable, hashes: &ContentHashes, a: usize, b: usize) -> Option<bool> {
    // loose files: sizes speak without reading; archive sizes may be packed, only hashes speak
    if !table.is_archive(a) && !table.is_archive(b) && table.size(a) != table.size(b) {
        return Some(false);
    }
    Some(hashes.get(a)? == hashes.get(b)?)
}

/// The copy that reaches the game: of the highest-priority active owner, its loose file before its archive entry.
fn winner(table: &ProviderTable, prio: &Priorities, p: PathId) -> Option<usize> {
    table
        .path_providers(p)
        .filter(|&i| prio.is_active(table.owner(i)))
        .max_by_key(|&i| (prio.get(table.owner(i)), !table.is_archive(i)))
}

/// What the copies of a path or a chain are, against the winner's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Every compared copy is the winner's bytes.
    Identical,
    /// Every path has a copy that differs.
    Different,
    /// Some paths are identical, others differ.
    Partly,
    /// Some copies could not be compared (archive entries, files not read).
    Unknown,
}

// @sat:identical.chain
/// The content of an exact chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainContent {
    pub verdict: Verdict,
    /// Losing copies equal to the winner, and their bytes.
    pub equal_files: u32,
    pub equal_bytes: u64,
    /// Paths with at least one copy that could not be compared.
    pub unknown_paths: u32,
}

/// Compares every losing copy of the chain's paths with the winner's.
pub fn chain_content(
    table: &ProviderTable,
    hashes: &ContentHashes,
    ow: &OverwriteProjection,
    c: ChainId,
) -> ChainContent {
    let prio = ow.priorities();
    let (mut equal_files, mut equal_bytes, mut unknown_paths) = (0, 0, 0);
    let (mut identical, mut different) = (0u32, 0u32);
    for p in ow.chain_paths(c) {
        let Some(w) = winner(table, prio, p) else { continue };
        let (mut all_same, mut any_diff, mut unknown) = (true, false, false);
        for i in table
            .path_providers(p)
            .filter(|&i| i != w && table.owner(i) != table.owner(w))
        {
            match same(table, hashes, w, i) {
                Some(true) => {
                    equal_files += 1;
                    equal_bytes += table.size(i);
                }
                Some(false) => {
                    all_same = false;
                    any_diff = true;
                }
                None => {
                    all_same = false;
                    unknown = true;
                }
            }
        }
        if unknown {
            unknown_paths += 1;
        }
        if any_diff {
            different += 1;
        } else if all_same {
            identical += 1;
        }
    }
    let verdict = match (identical, different, unknown_paths) {
        (_, _, u) if u > 0 && different == 0 => Verdict::Unknown,
        (0, _, _) => Verdict::Different,
        (_, 0, _) => Verdict::Identical,
        _ => Verdict::Partly,
    };
    ChainContent {
        verdict,
        equal_files,
        equal_bytes,
        unknown_paths,
    }
}

// @sat:identical.real-contexts
/// The overwrite contexts with the paths whose copies are all the same bytes left out: a path like that has a winner
/// whose bytes every loser also holds, so the order among its owners changes nothing the game sees. What stays are
/// the conflicts that matter, often in smaller groups. A path not compared (archive entries, files not read) counts as
/// a real conflict. Each group: active mods, lowest priority first.
pub fn real_contexts(
    table: &ProviderTable,
    hashes: &ContentHashes,
    ow: &OverwriteProjection,
) -> Vec<Vec<ModId>> {
    contexts_of(table, hashes, ow, false)
}

// @sat:identical.order-contexts
/// [`real_contexts`] of the loose copies only: where the mod order decides. A loose file beats every archive copy
/// whatever the order, and archives go by their plugins' order, so a path with one loose copy (or none) asks
/// nothing of the mod order; a texture pack that repacks the game's archives ties no mods together. What stays are
/// the groups whose order in the list is a real choice.
pub fn order_contexts(
    table: &ProviderTable,
    hashes: &ContentHashes,
    ow: &OverwriteProjection,
) -> Vec<Vec<ModId>> {
    contexts_of(table, hashes, ow, true)
}

fn contexts_of(
    table: &ProviderTable,
    hashes: &ContentHashes,
    ow: &OverwriteProjection,
    loose_only: bool,
) -> Vec<Vec<ModId>> {
    let prio = ow.priorities();
    let n = table.mod_count();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    let mut touched = vec![false; n];
    for cx in ow.contexts() {
        for &c in &cx.chains {
            for p in ow.chain_paths(c) {
                let ps: Vec<usize> = table
                    .path_providers(p)
                    .filter(|&i| prio.is_active(table.owner(i)) && !(loose_only && table.is_archive(i)))
                    .collect();
                let Some(&first) = ps.first() else { continue };
                if loose_only && ps.iter().all(|&i| table.owner(i) == table.owner(first)) {
                    continue;
                }
                if ps.iter().all(|&i| same(table, hashes, first, i) == Some(true)) {
                    continue;
                }
                let owners: Vec<usize> = ps.iter().map(|&i| table.owner(i).idx()).collect();
                for &o in &owners {
                    touched[o] = true;
                }
                for w in owners.windows(2) {
                    let (a, b) = (find(&mut parent, w[0]), find(&mut parent, w[1]));
                    if a != b {
                        parent[a] = b;
                    }
                }
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<ModId>> = Default::default();
    for m in 0..n {
        if touched[m] {
            let r = find(&mut parent, m);
            groups.entry(r).or_default().push(ModId::from_idx(m));
        }
    }
    let mut out: Vec<Vec<ModId>> = groups.into_values().filter(|g| g.len() > 1).collect();
    for g in &mut out {
        g.sort_by_key(|&m| prio.get(m));
    }
    out.sort_by_key(|g| std::cmp::Reverse(g.len()));
    out
}

// @sat:identical.mods
/// What a mod loses as copies equal to the winner (files the game takes from someone else, byte for byte the same).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModContent {
    pub equal_files: u32,
    pub equal_bytes: u64,
    /// Losing copies that could not be compared.
    pub unknown_files: u32,
}

/// Per mod (index = `ModId`), over every contested path.
pub fn mod_content(table: &ProviderTable, hashes: &ContentHashes, prio: &Priorities) -> Vec<ModContent> {
    let mut out = vec![ModContent::default(); table.mod_count()];
    for p in 0..table.path_count() {
        let p = PathId::from_idx(p);
        if table.path_providers(p).len() < 2 {
            continue;
        }
        let Some(w) = winner(table, prio, p) else { continue };
        for i in table
            .path_providers(p)
            .filter(|&i| table.owner(i) != table.owner(w))
        {
            let m = &mut out[table.owner(i).idx()];
            match same(table, hashes, w, i) {
                Some(true) => {
                    m.equal_files += 1;
                    m.equal_bytes += table.size(i);
                }
                Some(false) => {}
                None => m.unknown_files += 1,
            }
        }
    }
    out
}

// @sat:identical.needs
/// Copies worth reading, on contested paths (any mod, on or off — a verdict must not change when a mod is
/// switched): a loose file that shares its size with another loose copy of another mod, at least `min_size` bytes;
/// an archive entry whenever another mod has a copy (its packed size says nothing).
pub fn needs_hash(table: &ProviderTable, min_size: u64) -> Vec<usize> {
    let mut out = Vec::new();
    for p in 0..table.path_count() {
        let r = table.path_providers(PathId::from_idx(p));
        if r.len() < 2 {
            continue;
        }
        let all: Vec<usize> = r.collect();
        for &i in &all {
            let other = |j: &&usize| **j != i && table.owner(**j) != table.owner(i);
            let read = if table.is_archive(i) {
                all.iter().any(|j| other(&j))
            } else {
                table.size(i) >= min_size
                    && all
                        .iter()
                        .filter(other)
                        .any(|&j| table.is_archive(j) || table.size(j) == table.size(i))
            };
            if read {
                out.push(i);
            }
        }
    }
    out
}

/// Mods with equal copies, most bytes first.
pub fn top(content: &[ModContent]) -> Vec<(ModId, ModContent)> {
    let mut v: Vec<(ModId, ModContent)> = content
        .iter()
        .enumerate()
        .filter(|(_, c)| c.equal_files > 0)
        .map(|(m, c)| (ModId::from_idx(m), *c))
        .collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1.equal_bytes));
    v
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
    fn copies_are_equal_different_or_unknown_and_nothing_is_dropped() {
        // A < B < C by priority; D is off
        let facts = [
            mod_with("A", &[("same.dds", 100), ("diff.dds", 100), ("sized.dds", 5)]),
            mod_with("B", &[("same.dds", 100), ("unread.dds", 7)]),
            mod_with(
                "C",
                &[
                    ("same.dds", 100),
                    ("diff.dds", 100),
                    ("sized.dds", 6),
                    ("unread.dds", 7),
                ],
            ),
            mod_with("D", &[("same.dds", 100)]),
        ];
        let table = ProviderTable::build(&facts);
        let prio = Priorities(vec![0, 1, 2, -1]);
        let need = needs_hash(&table, 10);
        // same.dds ×4 (D too: off mods count), diff.dds ×2; sized.dds differs by size, unread.dds is below 10 bytes
        assert_eq!(need.len(), 6);
        let path = |i: usize| table.path_string(table.path_of_provider(i));
        let name = |i: usize| table.mod_name(table.owner(i)).to_string();
        let hashes = ContentHashes(
            (0..table.provider_count())
                .map(|i| match (path(i).as_str(), name(i).as_str()) {
                    (p, _) if p.ends_with("unread.dds") => None,
                    (p, "A") if p.ends_with("diff.dds") => Some(2),
                    _ => Some(1),
                })
                .collect(),
        )
        .for_table(&table);
        let mc = mod_content(&table, &hashes, &prio);
        assert_eq!(
            (mc[0].equal_files, mc[0].equal_bytes),
            (1, 100),
            "A: same.dds equal, diff and sized differ"
        );
        assert_eq!(
            (mc[1].equal_files, mc[1].unknown_files),
            (1, 1),
            "B: same.dds equal; unread.dds not read"
        );
        assert_eq!(mc[2], ModContent::default(), "C wins everything");
        assert_eq!(mc[3].equal_files, 1, "D is off, its copy is still a copy");
        assert_eq!(top(&mc)[0].1.equal_bytes, 100);

        let ow = OverwriteProjection::compute(&table, prio);
        let verdicts: Vec<Verdict> = ow
            .chain_ids()
            .into_iter()
            .map(|c| chain_content(&table, &hashes, &ow, c).verdict)
            .collect();
        assert!(verdicts.contains(&Verdict::Unknown), "{verdicts:?}");
        assert!(
            verdicts.contains(&Verdict::Partly) || verdicts.contains(&Verdict::Different),
            "{verdicts:?}"
        );
        // hashes of another table are not taken
        assert_eq!(
            ContentHashes(vec![Some(1)]).for_table(&table),
            ContentHashes::default()
        );
    }

    #[test]
    fn an_archive_copy_ties_no_mods_in_the_mod_order() {
        use crate::facts::ArchiveFacts;
        use sat_formats::{ArchiveEntry, ArchiveKind, ArchiveManifest};
        // Pack repacks the game's archive with a.dds and b.dds; A and B have them loose, differing
        let mut pack = mod_with("Pack", &[]);
        pack.archives.push(ArchiveFacts {
            file_name: "Skyrim - Textures0.bsa".into(),
            size: 10,
            manifest: Some(ArchiveManifest {
                kind: ArchiveKind::Bsa { version: 105 },
                entries: ["textures/a.dds", "textures/b.dds"]
                    .map(|p| ArchiveEntry {
                        path: p.into(),
                        size: 5,
                        vram: 0,
                    })
                    .to_vec(),
            }),
            error: None,
        });
        let facts = [
            mod_with("A", &[("a.dds", 9)]),
            mod_with("B", &[("b.dds", 9)]),
            pack,
        ];
        let table = ProviderTable::build(&facts);
        let hashes =
            ContentHashes((0..table.provider_count()).map(|i| Some(i as u64)).collect()).for_table(&table);
        let ow = OverwriteProjection::compute(&table, Priorities(vec![0, 1, 2]));
        // by every differing copy, the pack ties A and B into one group
        assert_eq!(real_contexts(&table, &hashes, &ow).first().map(Vec::len), Some(3));
        // by loose copies only, nothing is left to order: each path has one loose copy
        assert!(order_contexts(&table, &hashes, &ow).is_empty());
    }
}
