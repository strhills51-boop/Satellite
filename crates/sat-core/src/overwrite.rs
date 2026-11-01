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
//! The projection is a pure function of `(table, priorities)`, split in two (lesson of the old Satellite: the
//! order changes winners, never the structure of conflicts):
//! - [`ConflictIndex`] — facts only: contested paths (two or more distinct mods) and mod → contested paths;
//! - [`OverwriteProjection`] — stacks and winners of contested paths under priorities; single-mod paths are
//!   summed per mod once. [`OverwriteProjection::update`] recomputes only the paths the changed mods provide.
//!
//! Owns: the conflict index and the overwrite projection: stacks, chains, contexts, W/L/M/X statistics.

use std::sync::Arc;

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
/// High bit of [`ConflictIndex::path_kind`]: the path is contested, the rest is its slot.
const CONTESTED: u32 = 1 << 31;

/// Paths owned by exactly one mod, summed per mod (they cannot conflict, whatever the order).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Solo {
    paths: u32,
    bytes: u64,
    vram_kib: u64,
}

// @sat:overwrite.conflicts
/// Facts-only structure of conflicts: which paths are provided by two or more distinct mods (contested), and
/// which mods share each of them. Independent of order and enabled flags, so it is rebuilt only when the facts
/// change; priorities then decide stacks and winners for contested paths only (a few percent of all paths).
#[derive(Debug)]
pub struct ConflictIndex {
    /// Per path: `CONTESTED | slot`, or the owner mod of a single-mod path.
    path_kind: Vec<u32>,
    /// Slot → path, in path order.
    contested: Vec<PathId>,
    /// Mod → contested slots it provides (CSR).
    mod_slot_offsets: Vec<u32>,
    mod_slots: Vec<u32>,
    solo: Vec<Solo>,
}

/// Picks the provider of `m` that represents it on a single-mod path: loose before archive, then table order.
fn own_best(table: &ProviderTable, range: std::ops::Range<usize>) -> usize {
    range
        .clone()
        .find(|&i| !table.is_archive(i))
        .unwrap_or(range.start)
}

impl ConflictIndex {
    pub fn build(table: &ProviderTable) -> Self {
        let mods = table.mod_count();
        let paths = table.path_count();
        let mut path_kind = vec![NONE; paths];
        let mut contested = Vec::new();
        let mut solo = vec![Solo::default(); mods];
        let mut mod_counts = vec![0u32; mods + 1];
        let mut seen: Vec<ModId> = Vec::new();
        for (pi, kind) in path_kind.iter_mut().enumerate() {
            let p = PathId::from_idx(pi);
            let range = table.path_providers(p);
            seen.clear();
            for i in range.clone() {
                let m = table.owner(i);
                if !seen.contains(&m) {
                    seen.push(m);
                }
            }
            if seen.len() == 1 {
                let m = seen[0];
                let i = own_best(table, range);
                let sl = &mut solo[m.idx()];
                sl.paths += 1;
                sl.bytes += table.size(i);
                sl.vram_kib += u64::from(table.vram_kib(i));
                *kind = m.0;
            } else if seen.len() > 1 {
                *kind = CONTESTED | contested.len() as u32;
                contested.push(p);
                for m in &seen {
                    mod_counts[m.idx() + 1] += 1;
                }
            }
        }
        for i in 1..=mods {
            mod_counts[i] += mod_counts[i - 1];
        }
        let mut mod_slots = vec![0u32; mod_counts[mods] as usize];
        let mut fill = mod_counts[..mods].to_vec();
        for (slot, &p) in contested.iter().enumerate() {
            seen.clear();
            for i in table.path_providers(p) {
                let m = table.owner(i);
                if !seen.contains(&m) {
                    seen.push(m);
                    mod_slots[fill[m.idx()] as usize] = slot as u32;
                    fill[m.idx()] += 1;
                }
            }
        }
        Self {
            path_kind,
            contested,
            mod_slot_offsets: mod_counts,
            mod_slots,
            solo,
        }
    }

    pub fn contested_count(&self) -> usize {
        self.contested.len()
    }

    fn slots_of(&self, m: ModId) -> &[u32] {
        &self.mod_slots[self.mod_slot_offsets[m.idx()] as usize..self.mod_slot_offsets[m.idx() + 1] as usize]
    }
}

/// Per contested slot under some priorities: the stack of active distinct mods (winner first) and the winner's
/// bytes / VRAM. A stack of one is not a conflict (the winner counts it as unique); empty = no active provider.
#[derive(Debug, Clone, Default)]
struct Stacks {
    offsets: Vec<u32>,
    mods: Vec<ModId>,
    hash: Vec<u64>,
    bytes: Vec<u64>,
    vram: Vec<u32>,
}

/// Stack of one slot: loose before archive, then priority descending, then table order (deterministic).
fn stack_of(
    table: &ProviderTable,
    p: PathId,
    prio: &Priorities,
    cand: &mut Vec<(bool, i32, usize, ModId)>,
    out: &mut Vec<ModId>,
) -> (u64, u64, u32) {
    cand.clear();
    for i in table.path_providers(p) {
        let m = table.owner(i);
        let pr = prio.get(m);
        if pr >= 0 {
            cand.push((table.is_archive(i), -pr, i, m));
        }
    }
    if cand.is_empty() {
        return (0, 0, 0);
    }
    cand.sort_unstable();
    let wi = cand[0].2;
    let start = out.len();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &(_, _, _, m) in cand.iter() {
        if out[start..].contains(&m) {
            continue;
        }
        out.push(m);
        h ^= u64::from(m.0).wrapping_add(0x9e37_79b9_7f4a_7c15);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    (h, table.size(wi), table.vram_kib(wi))
}

/// Computes stacks for `slots` (sorted) in parallel; returns them in the same order as one CSR.
fn compute_stacks(table: &ProviderTable, index: &ConflictIndex, prio: &Priorities, slots: &[u32]) -> Stacks {
    let parts: Vec<Stacks> = slots
        .par_chunks(PART)
        .map(|chunk| {
            let mut st = Stacks {
                offsets: Vec::with_capacity(chunk.len()),
                ..Stacks::default()
            };
            let mut cand = Vec::new();
            for &slot in chunk {
                st.offsets.push(st.mods.len() as u32);
                let (h, b, v) = stack_of(
                    table,
                    index.contested[slot as usize],
                    prio,
                    &mut cand,
                    &mut st.mods,
                );
                st.hash.push(h);
                st.bytes.push(b);
                st.vram.push(v);
            }
            st
        })
        .collect();
    let mut all = Stacks {
        offsets: Vec::with_capacity(slots.len() + 1),
        ..Stacks::default()
    };
    for part in parts {
        let base = all.mods.len() as u32;
        all.offsets.extend(part.offsets.iter().map(|o| o + base));
        all.mods.extend(part.mods);
        all.hash.extend(part.hash);
        all.bytes.extend(part.bytes);
        all.vram.extend(part.vram);
    }
    all.offsets.push(all.mods.len() as u32);
    all
}

impl Stacks {
    fn get(&self, slot: usize) -> &[ModId] {
        &self.mods[self.offsets[slot] as usize..self.offsets[slot + 1] as usize]
    }
}

// @sat:overwrite.projection
/// The overwrite projection.
#[derive(Debug)]
pub struct OverwriteProjection {
    index: Arc<ConflictIndex>,
    priorities: Priorities,
    stacks: Stacks,
    // per contested slot
    slot_chain: Vec<u32>,
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

const PART: usize = 1 << 14;

impl OverwriteProjection {
    /// Full computation, building the conflict index on the way (one-shot callers).
    pub fn compute(table: &ProviderTable, priorities: Priorities) -> Self {
        Self::compute_with(Arc::new(ConflictIndex::build(table)), table, priorities)
    }

    /// Full computation over a prebuilt conflict index (`index` must be built from `table`).
    pub fn compute_with(index: Arc<ConflictIndex>, table: &ProviderTable, priorities: Priorities) -> Self {
        assert_eq!(
            priorities.0.len(),
            table.mod_count(),
            "priorities must cover every mod"
        );
        let all: Vec<u32> = (0..index.contested.len() as u32).collect();
        let stacks = compute_stacks(table, &index, &priorities, &all);
        Self::finish(index, table, priorities, stacks)
    }

    // @sat:overwrite.update
    /// Incremental recomputation after the priorities of `touched` mods changed (moved, enabled, disabled,
    /// added, removed). Other mods must keep their relative order and active state: then only contested paths
    /// that one of `touched` provides can change their stack. Stacks of all other paths are reused.
    pub fn update(&self, table: &ProviderTable, priorities: Priorities, touched: &[ModId]) -> Self {
        self.update_inner(table, priorities, touched, true)
    }

    fn update_inner(
        &self,
        table: &ProviderTable,
        priorities: Priorities,
        touched: &[ModId],
        fallback: bool,
    ) -> Self {
        assert_eq!(
            priorities.0.len(),
            table.mod_count(),
            "priorities must cover every mod"
        );
        let index = &self.index;
        let mut mark = vec![false; index.contested.len()];
        for &m in touched {
            for &s in index.slots_of(m) {
                mark[s as usize] = true;
            }
        }
        let affected: Vec<u32> = (0..mark.len() as u32).filter(|&s| mark[s as usize]).collect();
        if fallback && affected.len() * 2 > index.contested.len() {
            return Self::compute_with(index.clone(), table, priorities);
        }
        let fresh = compute_stacks(table, index, &priorities, &affected);
        let old = &self.stacks;
        let mut st = Stacks {
            offsets: Vec::with_capacity(mark.len() + 1),
            mods: Vec::with_capacity(old.mods.len()),
            hash: Vec::with_capacity(mark.len()),
            bytes: Vec::with_capacity(mark.len()),
            vram: Vec::with_capacity(mark.len()),
        };
        let mut next = 0;
        for slot in 0..mark.len() {
            st.offsets.push(st.mods.len() as u32);
            let (src, i) = if mark[slot] {
                next += 1;
                (&fresh, next - 1)
            } else {
                (old, slot)
            };
            st.mods.extend_from_slice(src.get(i));
            st.hash.push(src.hash[i]);
            st.bytes.push(src.bytes[i]);
            st.vram.push(src.vram[i]);
        }
        st.offsets.push(st.mods.len() as u32);
        Self::finish(index.clone(), table, priorities, st)
    }

    /// Interns stacks into chains, then W/L/M, contexts and totals (sequential, deterministic: slot order is
    /// path order).
    fn finish(
        index: Arc<ConflictIndex>,
        table: &ProviderTable,
        priorities: Priorities,
        stacks: Stacks,
    ) -> Self {
        let mods = table.mod_count();
        let slots = index.contested.len();
        let mut stats = vec![ModStat::default(); mods];
        let mut active_paths = 0;
        for (m, sl) in index.solo.iter().enumerate() {
            if priorities.is_active(ModId::from_idx(m)) {
                let st = &mut stats[m];
                st.unique += sl.paths;
                st.bytes_win += sl.bytes;
                st.vram_win_kib += sl.vram_kib;
                active_paths += sl.paths as usize;
            }
        }
        let mut slot_chain = vec![NONE; slots];
        let mut chains: Vec<Chain> = Vec::new();
        let mut chain_mod_offsets = vec![0u32];
        let mut chain_mods: Vec<ModId> = Vec::new();
        // hash -> first chain with that hash; collisions linked through `same_hash`
        let mut by_hash: FxHashMap<u64, u32> = FxHashMap::with_capacity_and_hasher(slots, Default::default());
        let mut same_hash: Vec<u32> = Vec::new();
        for slot in 0..slots {
            let seq = stacks.get(slot);
            if seq.is_empty() {
                continue;
            }
            active_paths += 1;
            let w = seq[0];
            stats[w.idx()].bytes_win += stacks.bytes[slot];
            stats[w.idx()].vram_win_kib += u64::from(stacks.vram[slot]);
            if seq.len() < 2 {
                stats[w.idx()].unique += 1;
                continue;
            }
            let h = stacks.hash[slot];
            let mut found = NONE;
            let mut c = by_hash.get(&h).copied().unwrap_or(NONE);
            while c != NONE {
                let ci = c as usize;
                if &chain_mods[chain_mod_offsets[ci] as usize..chain_mod_offsets[ci + 1] as usize] == seq {
                    found = c;
                    break;
                }
                c = same_hash[ci];
            }
            let id = if found != NONE {
                found
            } else {
                let c = chains.len() as u32;
                chains.push(Chain {
                    files: 0,
                    bytes: 0,
                    context: ContextId(0),
                });
                chain_mods.extend_from_slice(seq);
                chain_mod_offsets.push(chain_mods.len() as u32);
                let prev = by_hash.insert(h, c).unwrap_or(NONE);
                same_hash.push(prev);
                c
            };
            let ch = &mut chains[id as usize];
            ch.files += 1;
            ch.bytes += stacks.bytes[slot];
            slot_chain[slot] = id;
        }
        drop(by_hash);
        drop(same_hash);
        for (m, st) in stats.iter_mut().enumerate() {
            st.providers = table.mod_providers(ModId::from_idx(m)).len() as u32;
        }

        // chain -> paths (CSR)
        let mut chain_path_offsets = vec![0u32; chains.len() + 1];
        for (i, c) in chains.iter().enumerate() {
            chain_path_offsets[i + 1] = chain_path_offsets[i] + c.files;
        }
        let mut chain_paths = vec![PathId(0); chain_path_offsets[chains.len()] as usize];
        let mut fill = chain_path_offsets[..chains.len()].to_vec();
        for (slot, &c) in slot_chain.iter().enumerate() {
            if c != NONE {
                chain_paths[fill[c as usize] as usize] = index.contested[slot];
                fill[c as usize] += 1;
            }
        }
        // W/L/M, mod -> chains (CSR), contexts via union-find
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
            index,
            priorities,
            stacks,
            slot_chain,
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

    pub fn index(&self) -> &Arc<ConflictIndex> {
        &self.index
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
        let k = self.index.path_kind[p.idx()];
        if k == NONE || k & CONTESTED == 0 {
            return None;
        }
        match self.slot_chain[(k & !CONTESTED) as usize] {
            NONE => None,
            c => Some(ChainId(c)),
        }
    }
    pub fn path_winner(&self, p: PathId) -> Option<ModId> {
        match self.index.path_kind[p.idx()] {
            NONE => None,
            k if k & CONTESTED != 0 => self.stacks.get((k & !CONTESTED) as usize).first().copied(),
            m => self.priorities.is_active(ModId(m)).then_some(ModId(m)),
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

    fn same(a: &OverwriteProjection, b: &OverwriteProjection, t: &ProviderTable, what: &str) {
        assert_eq!(a.totals(), b.totals(), "{what}: totals");
        assert_eq!(a.stats(), b.stats(), "{what}: stats");
        assert_eq!(a.contexts(), b.contexts(), "{what}: contexts");
        assert_eq!(a.chain_count(), b.chain_count(), "{what}: chains");
        for c in 0..a.chain_count() {
            let c = ChainId::from_idx(c);
            assert_eq!(a.chain(c), b.chain(c), "{what}");
            assert_eq!(a.chain_mods(c), b.chain_mods(c), "{what}");
            assert_eq!(a.chain_paths(c), b.chain_paths(c), "{what}");
        }
        for p in 0..t.path_count() {
            let p = PathId::from_idx(p);
            assert_eq!(a.path_winner(p), b.path_winner(p), "{what}: winner");
            assert_eq!(a.path_chain(p), b.path_chain(p), "{what}: chain of path");
        }
    }

    // @sat:overwrite.tests.incremental
    /// Model test: after random moves and toggles, the incremental update equals a full computation.
    #[test]
    fn incremental_update_equals_full_compute() {
        let n = 60usize;
        let mods: Vec<_> = (0..n)
            .map(|i| {
                let files: Vec<String> = (0..40)
                    .map(|f| format!("d{}/f{}.dds", f % 5, (f * 13 + i * 7) % 120))
                    .collect();
                let refs: Vec<&str> = files.iter().map(String::as_str).collect();
                let m = mk(&format!("M{i}"), &refs);
                if i % 9 == 0 {
                    with_archive(m, &format!("M{i}.bsa"), &["d0/f1.dds", "d1/f2.dds", "x/arch.nif"])
                } else {
                    m
                }
            })
            .collect();
        let t = ProviderTable::build(&mods);
        // canon: an order of mods with enabled flags; priorities = rank if enabled
        let mut order: Vec<usize> = (0..n).collect();
        let mut enabled = vec![true; n];
        let prio = |order: &[usize], enabled: &[bool]| {
            let mut p = Priorities::all_inactive(n);
            for (rank, &m) in order.iter().enumerate() {
                if enabled[m] {
                    p.0[m] = rank as i32;
                }
            }
            p
        };
        let mut cur = OverwriteProjection::compute(&t, prio(&order, &enabled));
        let mut rng: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |k: usize| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % k as u64) as usize
        };
        for step in 0..300 {
            let mut touched = Vec::new();
            match next(3) {
                0 | 1 => {
                    let cnt = 1 + next(3);
                    for _ in 0..cnt {
                        let from = next(n);
                        let m = order.remove(from);
                        order.insert(next(n), m);
                        touched.push(ModId::from_idx(m));
                    }
                }
                _ => {
                    let m = next(n);
                    enabled[m] = !enabled[m];
                    touched.push(ModId::from_idx(m));
                }
            }
            let pr = prio(&order, &enabled);
            let inc = cur.update_inner(&t, pr.clone(), &touched, false);
            let full = OverwriteProjection::compute(&t, pr);
            same(&inc, &full, &t, &format!("step {step}"));
            cur = inc;
        }
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
