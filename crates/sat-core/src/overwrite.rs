//! Hot tier: the overwrite projection of a [`ProviderTable`] under given priorities. @sat:overwrite
//!
//! For every virtual path the active providers are ordered into a **stack**:
//! loose files beat archive entries (game rule for Bethesda titles), then higher priority wins.
//! A mod appears at most once per stack (its loose copy shadows its own archive copy).
//!
//! - **Exact chain** — the set of conflicting paths sharing one identical stack of mods.
//! - **Context** — a connected component of mods linked through chains.
//! - **W/L/M/X/+n** per mod — wins, losses, middle-of-stack, fully overridden, unique paths.
//!
//! The projection is a pure function of `(table, priorities)`; recomputing after a reorder costs no I/O.
//!
//! Owns: the overwrite projection: stacks, chains, contexts, W/L/M/X statistics.

use rayon::prelude::*;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::ids::{ChainId, ContextId, ModId, PathId};
use crate::table::ProviderTable;

/// Priority per mod (index = [`ModId`]); higher wins, `< 0` = inactive (disabled / not in the list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Priorities(pub Vec<i32>);

impl Priorities {
    pub const INACTIVE: i32 = -1;

    pub fn all_inactive(mods: usize) -> Self {
        Self(vec![Self::INACTIVE; mods])
    }
    #[inline]
    pub fn get(&self, m: ModId) -> i32 {
        self.0[m.idx()]
    }
    #[inline]
    pub fn is_active(&self, m: ModId) -> bool {
        self.0[m.idx()] >= 0
    }
}

/// Per-mod overwrite summary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModStat {
    /// Providers of this mod (loose + archive entries).
    pub providers: u32,
    /// Paths only this active mod provides (+n).
    pub unique: u32,
    /// Conflict paths this mod wins (W).
    pub win: u32,
    /// Conflict paths where it is overridden (L).
    pub lose: u32,
    /// Overridden, but itself overrides someone below (M).
    pub mid: u32,
    /// Active, has providers, wins nothing and has nothing unique — invisible in game (X).
    pub fully_overridden: bool,
    pub chains: u32,
    pub context: Option<ContextId>,
    /// VRAM of textures where this mod is the winner, KiB.
    pub vram_win_kib: u64,
    /// Bytes of files where this mod is the winner.
    pub bytes_win: u64,
}

/// One exact chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chain {
    pub files: u32,
    /// Winner bytes across the chain.
    pub bytes: u64,
    pub context: ContextId,
}

/// One context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    /// Members ordered by priority, lowest first (MO2 display order).
    pub mods: Vec<ModId>,
    pub chains: Vec<ChainId>,
    pub files: u32,
}

/// Totals for logs and budgets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverwriteStats {
    pub active_paths: usize,
    pub conflict_paths: usize,
    pub chains: usize,
    pub contexts: usize,
    pub fully_overridden: usize,
    pub vram_win_kib: u64,
}

const NONE: u32 = u32::MAX;

// @sat:overwrite.projection
/// The overwrite projection.
#[derive(Debug)]
pub struct OverwriteProjection {
    priorities: Priorities,
    // per path
    path_chain: Vec<u32>,
    path_winner: Vec<u32>,
    // chains
    chains: Vec<Chain>,
    chain_mod_offsets: Vec<u32>,
    chain_mods: Vec<ModId>,
    chain_path_offsets: Vec<u32>,
    chain_paths: Vec<PathId>,
    // mods
    stats: Vec<ModStat>,
    mod_chain_offsets: Vec<u32>,
    mod_chains: Vec<ChainId>,
    contexts: Vec<Context>,
    totals: OverwriteStats,
}

struct Part {
    first_path: usize,
    conflicts: Vec<(u32, u64, u32, u32, u64)>, // (path, hash, pool offset, len, winner bytes)
    pool: Vec<ModId>,
    unique: Vec<u32>,
    vram: Vec<u64>,
    bytes: Vec<u64>,
    active: usize,
}

const PART: usize = 1 << 14;

impl OverwriteProjection {
    pub fn compute(table: &ProviderTable, priorities: Priorities) -> Self {
        assert_eq!(
            priorities.0.len(),
            table.mod_count(),
            "priorities must cover every mod"
        );
        let mods = table.mod_count();
        let paths = table.path_count();
        let mut path_chain = vec![NONE; paths];
        let mut path_winner = vec![NONE; paths];
        let prio = &priorities;

        // 1. Parallel: per-path stacks.
        let parts: Vec<Part> = path_winner
            .par_chunks_mut(PART)
            .enumerate()
            .map(|(ci, winners)| {
                let first = ci * PART;
                let mut part = Part {
                    first_path: first,
                    conflicts: Vec::new(),
                    pool: Vec::new(),
                    unique: vec![0; mods],
                    vram: vec![0; mods],
                    bytes: vec![0; mods],
                    active: 0,
                };
                let mut cand: Vec<(bool, i32, ModId, usize)> = Vec::new();
                for (k, slot) in winners.iter_mut().enumerate() {
                    let p = PathId::from_idx(first + k);
                    let range = table.path_providers(p);
                    if range.len() == 1 {
                        // Fast path: most paths have exactly one provider.
                        let i = range.start;
                        let m = table.owner(i);
                        if prio.get(m) >= 0 {
                            part.active += 1;
                            *slot = m.0;
                            part.vram[m.idx()] += u64::from(table.vram_kib(i));
                            part.bytes[m.idx()] += table.size(i);
                            part.unique[m.idx()] += 1;
                        }
                        continue;
                    }
                    cand.clear();
                    for i in table.path_providers(p) {
                        let m = table.owner(i);
                        let pr = prio.get(m);
                        if pr >= 0 {
                            cand.push((table.is_archive(i), pr, m, i));
                        }
                    }
                    if cand.is_empty() {
                        continue;
                    }
                    part.active += 1;
                    if cand.len() > 1 {
                        // loose (false) before archive (true), then priority descending
                        cand.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
                    }
                    let (_, _, w, wi) = cand[0];
                    *slot = w.0;
                    part.vram[w.idx()] += u64::from(table.vram_kib(wi));
                    part.bytes[w.idx()] += table.size(wi);
                    let start = part.pool.len();
                    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
                    for &(_, _, m, _) in &cand {
                        if part.pool[start..].contains(&m) {
                            continue;
                        }
                        part.pool.push(m);
                        h ^= u64::from(m.0).wrapping_add(0x9e37_79b9_7f4a_7c15);
                        h = h.wrapping_mul(0x0100_0000_01b3);
                    }
                    let len = part.pool.len() - start;
                    if len < 2 {
                        part.pool.truncate(start);
                        part.unique[w.idx()] += 1;
                        continue;
                    }
                    part.conflicts
                        .push((p.0, h, start as u32, len as u32, table.size(wi)));
                }
                part
            })
            .collect();

        // 2. Sequential, deterministic (path order): intern stacks into chains.
        let mut stats = vec![ModStat::default(); mods];
        let mut chains: Vec<Chain> = Vec::new();
        let mut chain_mod_offsets = vec![0u32];
        let mut chain_mods: Vec<ModId> = Vec::new();
        let mut by_hash: FxHashMap<u64, Vec<u32>> = FxHashMap::default();
        let mut active_paths = 0;
        for part in &parts {
            debug_assert!(part.first_path % PART == 0);
            active_paths += part.active;
            for m in 0..mods {
                stats[m].unique += part.unique[m];
                stats[m].vram_win_kib += part.vram[m];
                stats[m].bytes_win += part.bytes[m];
            }
            for &(p, h, off, len, win_bytes) in &part.conflicts {
                let seq = &part.pool[off as usize..(off + len) as usize];
                let bucket = by_hash.entry(h).or_default();
                let id = match bucket.iter().copied().find(|&c| {
                    let c = c as usize;
                    &chain_mods[chain_mod_offsets[c] as usize..chain_mod_offsets[c + 1] as usize] == seq
                }) {
                    Some(c) => c,
                    None => {
                        let c = chains.len() as u32;
                        chains.push(Chain {
                            files: 0,
                            bytes: 0,
                            context: ContextId(0),
                        });
                        chain_mods.extend_from_slice(seq);
                        chain_mod_offsets.push(chain_mods.len() as u32);
                        bucket.push(c);
                        c
                    }
                };
                let ch = &mut chains[id as usize];
                ch.files += 1;
                ch.bytes += win_bytes;
                path_chain[p as usize] = id;
            }
        }
        drop(parts);
        for (m, st) in stats.iter_mut().enumerate() {
            st.providers = table.mod_providers(ModId::from_idx(m)).len() as u32;
        }

        // 3. chain -> paths (CSR)
        let mut chain_path_offsets = vec![0u32; chains.len() + 1];
        for (i, c) in chains.iter().enumerate() {
            chain_path_offsets[i + 1] = chain_path_offsets[i] + c.files;
        }
        let mut chain_paths = vec![PathId(0); chain_path_offsets[chains.len()] as usize];
        let mut fill = chain_path_offsets[..chains.len()].to_vec();
        for (p, &c) in path_chain.iter().enumerate() {
            if c != NONE {
                chain_paths[fill[c as usize] as usize] = PathId::from_idx(p);
                fill[c as usize] += 1;
            }
        }

        // 4. W/L/M, mod -> chains (CSR), contexts via union-find
        let mut parent: Vec<u32> = (0..mods as u32).collect();
        fn find(parent: &mut [u32], mut x: u32) -> u32 {
            while parent[x as usize] != x {
                parent[x as usize] = parent[parent[x as usize] as usize];
                x = parent[x as usize];
            }
            x
        }
        let mut mod_chain_offsets = vec![0u32; mods + 1];
        for c in 0..chains.len() {
            let ms = &chain_mods[chain_mod_offsets[c] as usize..chain_mod_offsets[c + 1] as usize];
            let files = chains[c].files;
            let last = ms.len() - 1;
            for (i, &m) in ms.iter().enumerate() {
                let st = &mut stats[m.idx()];
                st.chains += 1;
                mod_chain_offsets[m.idx() + 1] += 1;
                if i == 0 {
                    st.win += files;
                } else {
                    st.lose += files;
                    if i < last {
                        st.mid += files;
                    }
                    let (a, b) = (find(&mut parent, ms[0].0), find(&mut parent, m.0));
                    if a != b {
                        parent[b as usize] = a;
                    }
                }
            }
        }
        for i in 1..=mods {
            mod_chain_offsets[i] += mod_chain_offsets[i - 1];
        }
        let mut mod_chains = vec![ChainId(0); mod_chain_offsets[mods] as usize];
        let mut mfill = mod_chain_offsets[..mods].to_vec();
        for c in 0..chains.len() {
            for &m in &chain_mods[chain_mod_offsets[c] as usize..chain_mod_offsets[c + 1] as usize] {
                mod_chains[mfill[m.idx()] as usize] = ChainId::from_idx(c);
                mfill[m.idx()] += 1;
            }
        }

        let mut contexts: Vec<Context> = Vec::new();
        let mut ctx_of_root: FxHashMap<u32, u32> = FxHashMap::default();
        let mut fully = 0;
        for m in 0..mods {
            let mid = ModId::from_idx(m);
            let st = &mut stats[m];
            if priorities.is_active(mid) && st.providers > 0 && st.win == 0 && st.unique == 0 {
                st.fully_overridden = true;
                fully += 1;
            }
            if st.chains == 0 {
                continue;
            }
            let root = find(&mut parent, m as u32);
            let next = contexts.len() as u32;
            let id = *ctx_of_root.entry(root).or_insert(next);
            if id == next {
                contexts.push(Context {
                    mods: Vec::new(),
                    chains: Vec::new(),
                    files: 0,
                });
            }
            st.context = Some(ContextId(id));
            contexts[id as usize].mods.push(mid);
        }
        for c in 0..chains.len() {
            let first = chain_mods[chain_mod_offsets[c] as usize];
            let ctx = stats[first.idx()].context.expect("chain member has a context");
            chains[c].context = ctx;
            let cx = &mut contexts[ctx.idx()];
            cx.chains.push(ChainId::from_idx(c));
            cx.files += chains[c].files;
        }
        for cx in &mut contexts {
            cx.mods.sort_unstable_by_key(|&m| priorities.get(m));
        }

        let totals = OverwriteStats {
            active_paths,
            conflict_paths: chain_paths.len(),
            chains: chains.len(),
            contexts: contexts.len(),
            fully_overridden: fully,
            vram_win_kib: stats.iter().map(|s| s.vram_win_kib).sum(),
        };
        Self {
            priorities,
            path_chain,
            path_winner,
            chains,
            chain_mod_offsets,
            chain_mods,
            chain_path_offsets,
            chain_paths,
            stats,
            mod_chain_offsets,
            mod_chains,
            contexts,
            totals,
        }
    }

    pub fn priorities(&self) -> &Priorities {
        &self.priorities
    }
    pub fn totals(&self) -> OverwriteStats {
        self.totals
    }
    pub fn stat(&self, m: ModId) -> &ModStat {
        &self.stats[m.idx()]
    }
    pub fn stats(&self) -> &[ModStat] {
        &self.stats
    }
    pub fn chain_count(&self) -> usize {
        self.chains.len()
    }
    pub fn chain(&self, c: ChainId) -> &Chain {
        &self.chains[c.idx()]
    }
    /// Stack of the chain, winner first.
    pub fn chain_mods(&self, c: ChainId) -> &[ModId] {
        &self.chain_mods
            [self.chain_mod_offsets[c.idx()] as usize..self.chain_mod_offsets[c.idx() + 1] as usize]
    }
    pub fn chain_paths(&self, c: ChainId) -> &[PathId] {
        &self.chain_paths
            [self.chain_path_offsets[c.idx()] as usize..self.chain_path_offsets[c.idx() + 1] as usize]
    }
    pub fn mod_chains(&self, m: ModId) -> &[ChainId] {
        &self.mod_chains
            [self.mod_chain_offsets[m.idx()] as usize..self.mod_chain_offsets[m.idx() + 1] as usize]
    }
    pub fn contexts(&self) -> &[Context] {
        &self.contexts
    }
    pub fn path_chain(&self, p: PathId) -> Option<ChainId> {
        match self.path_chain[p.idx()] {
            NONE => None,
            c => Some(ChainId(c)),
        }
    }
    pub fn path_winner(&self, p: PathId) -> Option<ModId> {
        match self.path_winner[p.idx()] {
            NONE => None,
            m => Some(ModId(m)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::tests::{mk, with_archive};

    fn names(t: &ProviderTable, ms: &[ModId]) -> Vec<String> {
        ms.iter().map(|&m| t.mod_name(m).to_string()).collect()
    }

    #[test]
    fn stacks_chains_contexts_and_badges() {
        let t = ProviderTable::build(&[
            mk("A", &["textures/a.dds", "textures/b.dds", "meshes/x.nif"]),
            mk("B", &["Textures/A.dds", "textures/c.dds"]),
            mk("C", &["textures/a.dds", "textures/b.dds", "textures/c.dds"]),
            mk("D", &["other/only.txt"]),
            mk("E", &["meshes/x.nif"]),
        ]);
        // E disabled
        let p = OverwriteProjection::compute(&t, Priorities(vec![0, 1, 2, 3, -1]));
        // a.dds: C>B>A, b.dds: C>A, c.dds: C>B, x.nif: only A
        assert_eq!(p.totals().chains, 3);
        assert_eq!(p.totals().conflict_paths, 3);
        let (a, b, c) = (ModId(0), ModId(1), ModId(2));
        assert_eq!(p.stat(c).win, 3);
        assert_eq!(p.stat(a).lose, 2);
        assert_eq!(p.stat(b).mid, 1);
        assert_eq!(p.stat(a).unique, 1);
        assert!(!p.stat(a).fully_overridden);
        assert!(
            p.stat(b).fully_overridden,
            "B wins nothing and has nothing unique"
        );
        assert_eq!(p.contexts().len(), 1);
        assert_eq!(names(&t, &p.contexts()[0].mods), ["A", "B", "C"]);
        let pa = t.find_path("textures/a.dds").unwrap();
        let ch = p.path_chain(pa).unwrap();
        assert_eq!(names(&t, p.chain_mods(ch)), ["C", "B", "A"]);
        assert_eq!(p.path_winner(pa), Some(c));

        // enable E above everything: x.nif becomes a conflict, A loses its unique
        let p2 = OverwriteProjection::compute(&t, Priorities(vec![0, 1, 2, 3, 4]));
        assert_eq!(p2.stat(ModId(4)).win, 1);
        assert_eq!(p2.stat(a).unique, 0);
    }

    #[test]
    fn loose_beats_archive_regardless_of_priority() {
        let t = ProviderTable::build(&[
            mk("Low", &["meshes/x.nif"]),
            with_archive(mk("High", &[]), "High.bsa", &["meshes/x.nif"]),
        ]);
        let p = OverwriteProjection::compute(&t, Priorities(vec![0, 10]));
        let x = t.find_path("meshes/x.nif").unwrap();
        assert_eq!(p.path_winner(x), Some(ModId(0)));
        assert_eq!(names(&t, p.chain_mods(p.path_chain(x).unwrap())), ["Low", "High"]);
    }

    #[test]
    fn own_archive_copy_is_not_a_conflict() {
        let t = ProviderTable::build(&[with_archive(mk("M", &["a/b.dds"]), "M.bsa", &["a/b.dds"])]);
        let p = OverwriteProjection::compute(&t, Priorities(vec![0]));
        assert_eq!(p.totals().chains, 0);
        assert_eq!(p.stat(ModId(0)).unique, 1);
        assert_eq!(p.stat(ModId(0)).providers, 2);
    }

    #[test]
    fn deterministic_across_runs() {
        let mods: Vec<_> = (0..40)
            .map(|i| {
                let files: Vec<String> = (0..60)
                    .map(|f| format!("d{}/f{}.dds", f % 7, (f * 31 + i) % 90))
                    .collect();
                let refs: Vec<&str> = files.iter().map(String::as_str).collect();
                mk(&format!("M{i}"), &refs)
            })
            .collect();
        let t = ProviderTable::build(&mods);
        let pr = Priorities((0..40).collect());
        let a = OverwriteProjection::compute(&t, pr.clone());
        let b = OverwriteProjection::compute(&t, pr);
        assert_eq!(a.totals(), b.totals());
        assert_eq!(a.stats(), b.stats());
        for c in 0..a.chain_count() {
            assert_eq!(
                a.chain_mods(ChainId::from_idx(c)),
                b.chain_mods(ChainId::from_idx(c))
            );
        }
    }
}
